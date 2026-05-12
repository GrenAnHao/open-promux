//! Desktop application state shared across Tauri commands.
//!
//! Wraps the embeddable [`open_promux::ServerHandle`] together with a
//! [`open_promux::LogBus`] and the on-disk configuration path. All members are
//! kept behind async locks so commands can be invoked freely from JavaScript.

use std::path::{Path, PathBuf};

use open_promux::{LogBus, ServerHandle, config::DEFAULT_CONFIG_TEMPLATE};
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

/// Write [`DEFAULT_CONFIG_TEMPLATE`] to `path` if no file exists there yet.
///
/// Mirrors cc-switch's "first-run bootstrap" pattern: a fresh install gets a
/// sane, commented starting point instead of a blank form. Existing configs
/// are left untouched, even if they are syntactically empty.
///
/// Failures during directory creation or file write are returned to the
/// caller; the desktop entry point logs and continues so the UI can still
/// render the (empty) form.
pub fn ensure_default_config(path: &Path) -> std::io::Result<bool> {
    if path.exists() {
        return Ok(false);
    }
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::write(path, DEFAULT_CONFIG_TEMPLATE)?;
    Ok(true)
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn ensure_default_config_writes_template_when_missing() {
        let dir = tempdir().expect("tempdir");
        let path = dir.path().join("nested").join("config.toml");
        let written = ensure_default_config(&path).expect("write template");
        assert!(written, "first call should report write");
        let contents = std::fs::read_to_string(&path).expect("read back");
        assert_eq!(contents, DEFAULT_CONFIG_TEMPLATE);
    }

    #[test]
    fn ensure_default_config_does_not_overwrite_existing_file() {
        let dir = tempdir().expect("tempdir");
        let path = dir.path().join("config.toml");
        std::fs::write(&path, "host = \"10.0.0.1\"\nport = 9999\n").expect("seed existing config");

        let written = ensure_default_config(&path).expect("call helper");
        assert!(!written, "second call should skip");

        let contents = std::fs::read_to_string(&path).expect("read existing");
        assert!(contents.contains("10.0.0.1"));
    }
}
