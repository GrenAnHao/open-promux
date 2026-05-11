//! Desktop application state shared across Tauri commands.
//!
//! Wraps the embeddable [`open_promux::ServerHandle`] together with a
//! [`open_promux::LogBus`] and the on-disk configuration path. All members are
//! kept behind async locks so commands can be invoked freely from JavaScript.

use std::path::PathBuf;

use open_promux::{LogBus, ServerHandle};
use tokio::sync::Mutex;

/// Default file name searched in the platform config directory.
pub const CONFIG_FILE_NAME: &str = "config.toml";

/// Single source of truth for the desktop runtime.
pub struct DesktopState {
    /// Broadcast/ring-buffer for proxy logs. Cloneable handle.
    pub log_bus: LogBus,
    /// Path used by load/save commands. Resolved at startup.
    pub config_path: Mutex<PathBuf>,
    /// Currently running embedded server, if any.
    pub server: Mutex<Option<ServerHandle>>,
}

impl DesktopState {
    pub fn new(log_bus: LogBus, config_path: PathBuf) -> Self {
        Self {
            log_bus,
            config_path: Mutex::new(config_path),
            server: Mutex::new(None),
        }
    }
}

/// Resolve the default config path: `<config_dir>/open-promux/config.toml`,
/// falling back to a sibling of the executable when the platform-specific
/// directory cannot be discovered.
pub fn default_config_path() -> PathBuf {
    if let Some(dir) = dirs::config_dir() {
        return dir.join("open-promux").join(CONFIG_FILE_NAME);
    }
    if let Ok(exe) = std::env::current_exe()
        && let Some(parent) = exe.parent()
    {
        return parent.join(CONFIG_FILE_NAME);
    }
    PathBuf::from(CONFIG_FILE_NAME)
}
