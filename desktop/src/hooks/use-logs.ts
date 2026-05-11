import { useCallback, useEffect, useRef, useState } from "react";
import { listen, type UnlistenFn } from "@tauri-apps/api/event";

import { api } from "@/lib/api";
import type { LogLine } from "@/lib/types";

const MAX_LINES = 2048;

const LOG_EVENT = "log://line";

/**
 * Loads the historical buffer once and then subscribes to the Tauri
 * `log://line` stream emitted by the Rust log bridge.
 *
 * Incoming lines are batched into a ref-based queue and flushed at most
 * once per animation frame. Without this, a chatty server (hundreds of
 * lines per second) would cause one `setState` per line and re-render the
 * whole log viewer for every event – which dominates the main thread well
 * before the renderer itself becomes the bottleneck.
 */
export function useLogs() {
  const [lines, setLines] = useState<LogLine[]>([]);
  const seqSeen = useRef<Set<number>>(new Set());
  const pending = useRef<LogLine[]>([]);
  const rafHandle = useRef<number | null>(null);

  const flush = useCallback(() => {
    rafHandle.current = null;
    if (pending.current.length === 0) return;
    const incoming = pending.current;
    pending.current = [];
    setLines((prev) => {
      const merged =
        prev.length + incoming.length > MAX_LINES
          ? prev.concat(incoming).slice(-MAX_LINES)
          : prev.concat(incoming);
      // Rebuild dedupe set from the trimmed window so it can't grow
      // unbounded for long-running sessions.
      if (merged.length !== prev.length + incoming.length) {
        seqSeen.current = new Set(merged.map((l) => l.seq));
      }
      return merged;
    });
  }, []);

  useEffect(() => {
    let unlisten: UnlistenFn | undefined;
    let cancelled = false;

    const append = (line: LogLine) => {
      if (seqSeen.current.has(line.seq)) return;
      seqSeen.current.add(line.seq);
      pending.current.push(line);
      if (rafHandle.current === null) {
        rafHandle.current = requestAnimationFrame(flush);
      }
    };

    (async () => {
      try {
        const snapshot = await api.getLogsSnapshot();
        if (!cancelled) {
          const window = snapshot.slice(-MAX_LINES);
          seqSeen.current = new Set(window.map((l) => l.seq));
          setLines(window);
        }
      } catch {
        // ignore: snapshot is best-effort
      }
      if (!cancelled) {
        unlisten = await listen<LogLine>(LOG_EVENT, (event) => {
          append(event.payload);
        });
      }
    })();

    return () => {
      cancelled = true;
      if (unlisten) unlisten();
      if (rafHandle.current !== null) {
        cancelAnimationFrame(rafHandle.current);
        rafHandle.current = null;
      }
      pending.current = [];
    };
  }, [flush]);

  const clear = async () => {
    await api.clearLogs();
    seqSeen.current = new Set();
    pending.current = [];
    if (rafHandle.current !== null) {
      cancelAnimationFrame(rafHandle.current);
      rafHandle.current = null;
    }
    setLines([]);
  };

  return { lines, clear };
}
