//! In-process log bus used by the desktop UI to display real-time logs.
//!
//! Combines a bounded ring buffer (so a freshly-opened UI can fetch the most
//! recent backlog) with a [`tokio::sync::broadcast`] channel (for live
//! streaming once the UI subscribes). Embedders install [`LogBusLayer`] into
//! a `tracing_subscriber::Registry` to capture every event the proxy emits.

use std::{
    collections::VecDeque,
    fmt::Write as FmtWrite,
    sync::{Arc, Mutex},
    time::{SystemTime, UNIX_EPOCH},
};

use serde::Serialize;
use tokio::sync::broadcast;
use tracing::field::{Field, Visit};
use tracing_subscriber::{Layer, layer::Context};

/// Maximum number of buffered log lines retained for backfill.
const RING_CAPACITY: usize = 2048;

/// Broadcast channel buffer; older items are dropped if a subscriber lags.
const BROADCAST_CAPACITY: usize = 512;

/// A single captured log event, formatted for display in the desktop UI.
#[derive(Debug, Clone, Serialize)]
pub struct LogLine {
    /// Monotonically increasing sequence id assigned by the bus.
    pub seq: u64,
    /// Wall-clock timestamp in milliseconds since the Unix epoch.
    pub ts_millis: i64,
    /// Log level: TRACE / DEBUG / INFO / WARN / ERROR.
    pub level: &'static str,
    /// `tracing` event target (typically the module path).
    pub target: String,
    /// Concatenated `message` field plus any structured fields.
    pub message: String,
}

/// Shared handle that the desktop UI uses to fetch backlog and subscribe.
#[derive(Clone)]
pub struct LogBus {
    inner: Arc<Inner>,
}

struct Inner {
    ring: Mutex<RingBuffer>,
    sender: broadcast::Sender<LogLine>,
}

struct RingBuffer {
    items: VecDeque<LogLine>,
    next_seq: u64,
}

impl LogBus {
    /// Create a new bus with default ring/broadcast capacities.
    pub fn new() -> Self {
        let (sender, _rx) = broadcast::channel(BROADCAST_CAPACITY);
        Self {
            inner: Arc::new(Inner {
                ring: Mutex::new(RingBuffer {
                    items: VecDeque::with_capacity(RING_CAPACITY),
                    next_seq: 1,
                }),
                sender,
            }),
        }
    }

    /// Snapshot the most recent retained lines (oldest first).
    pub fn snapshot(&self) -> Vec<LogLine> {
        let ring = self.inner.ring.lock().expect("log bus ring poisoned");
        ring.items.iter().cloned().collect()
    }

    /// Drop every retained line.
    pub fn clear(&self) {
        let mut ring = self.inner.ring.lock().expect("log bus ring poisoned");
        ring.items.clear();
    }

    /// Subscribe to the live broadcast channel.
    pub fn subscribe(&self) -> broadcast::Receiver<LogLine> {
        self.inner.sender.subscribe()
    }

    /// Build a [`LogBusLayer`] that pushes events into this bus.
    pub fn layer(&self) -> LogBusLayer {
        LogBusLayer {
            inner: Arc::clone(&self.inner),
        }
    }
}

impl Default for LogBus {
    fn default() -> Self {
        Self::new()
    }
}

/// `tracing_subscriber` layer that records events into a [`LogBus`].
pub struct LogBusLayer {
    inner: Arc<Inner>,
}

impl<S> Layer<S> for LogBusLayer
where
    S: tracing::Subscriber,
{
    fn on_event(&self, event: &tracing::Event<'_>, _: Context<'_, S>) {
        let metadata = event.metadata();
        let level = level_str(metadata.level());

        let mut visitor = MessageVisitor::default();
        event.record(&mut visitor);
        let MessageVisitor {
            mut message,
            fields,
        } = visitor;
        for (key, value) in fields {
            let separator = if message.is_empty() { "" } else { " " };
            let _ = write!(&mut message, "{separator}{key}={value}");
        }

        let mut ring = self.inner.ring.lock().expect("log bus ring poisoned");
        let seq = ring.next_seq;
        ring.next_seq = ring.next_seq.wrapping_add(1);

        let ts_millis = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_millis() as i64)
            .unwrap_or_default();

        let line = LogLine {
            seq,
            ts_millis,
            level,
            target: metadata.target().to_string(),
            message,
        };

        if ring.items.len() == RING_CAPACITY {
            ring.items.pop_front();
        }
        ring.items.push_back(line.clone());
        drop(ring);

        let _ = self.inner.sender.send(line);
    }
}

fn level_str(level: &tracing::Level) -> &'static str {
    match *level {
        tracing::Level::TRACE => "TRACE",
        tracing::Level::DEBUG => "DEBUG",
        tracing::Level::INFO => "INFO",
        tracing::Level::WARN => "WARN",
        tracing::Level::ERROR => "ERROR",
    }
}

#[derive(Default)]
struct MessageVisitor {
    message: String,
    fields: Vec<(&'static str, String)>,
}

impl Visit for MessageVisitor {
    fn record_str(&mut self, field: &Field, value: &str) {
        if field.name() == "message" {
            self.message.push_str(value);
        } else {
            self.fields.push((field.name(), value.to_string()));
        }
    }

    fn record_debug(&mut self, field: &Field, value: &dyn std::fmt::Debug) {
        if field.name() == "message" {
            let _ = write!(&mut self.message, "{value:?}");
        } else {
            self.fields.push((field.name(), format!("{value:?}")));
        }
    }

    fn record_i64(&mut self, field: &Field, value: i64) {
        self.fields.push((field.name(), value.to_string()));
    }

    fn record_u64(&mut self, field: &Field, value: u64) {
        self.fields.push((field.name(), value.to_string()));
    }

    fn record_bool(&mut self, field: &Field, value: bool) {
        self.fields.push((field.name(), value.to_string()));
    }

    fn record_f64(&mut self, field: &Field, value: f64) {
        self.fields.push((field.name(), value.to_string()));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tracing_subscriber::{Registry, layer::SubscriberExt};

    #[test]
    fn log_bus_should_capture_messages_and_fields_into_ring() {
        let bus = LogBus::new();
        let subscriber = Registry::default().with(bus.layer());

        tracing::subscriber::with_default(subscriber, || {
            tracing::info!(client = "open-promux", "request handled");
            tracing::warn!("upstream slow");
        });

        let snapshot = bus.snapshot();
        assert_eq!(snapshot.len(), 2);

        let first = &snapshot[0];
        assert_eq!(first.level, "INFO");
        assert!(first.message.contains("request handled"));
        assert!(first.message.contains("client=open-promux"));

        let second = &snapshot[1];
        assert_eq!(second.level, "WARN");
        assert!(second.message.contains("upstream slow"));
        assert!(second.seq > first.seq);
    }

    #[test]
    fn log_bus_subscribers_receive_live_events() {
        let bus = LogBus::new();
        let mut rx = bus.subscribe();
        let subscriber = Registry::default().with(bus.layer());

        tracing::subscriber::with_default(subscriber, || {
            tracing::error!("boom");
        });

        let event = rx.try_recv().expect("subscriber gets the event");
        assert_eq!(event.level, "ERROR");
        assert_eq!(event.message, "boom");
    }

    #[test]
    fn log_bus_clear_should_drop_retained_lines() {
        let bus = LogBus::new();
        let subscriber = Registry::default().with(bus.layer());

        tracing::subscriber::with_default(subscriber, || {
            tracing::info!("first");
            tracing::info!("second");
        });

        assert_eq!(bus.snapshot().len(), 2);
        bus.clear();
        assert!(bus.snapshot().is_empty());
    }
}
