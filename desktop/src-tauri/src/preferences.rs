//! Persistent desktop-only preferences (UI language, etc.).
//!
//! Stored next to the proxy `config.toml` in the platform config directory.
//! Kept in a separate file so the proxy CLI never has to know about UI
//! state, and so wiping the desktop UI's settings doesn't affect proxy
//! behaviour.

use std::path::PathBuf;

use serde::{Deserialize, Serialize};

pub const PREFERENCES_FILE_NAME: &str = "desktop_preferences.toml";

#[derive(Debug, Clone, Default, Deserialize, Serialize)]
pub struct DesktopPreferences {
    /// BCP-47 language tag selected in the UI (`"en"`, `"zh"`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub language: Option<String>,
}

/// Locate the preferences file. Always returns a path; the file may not yet
/// exist (callers handle "missing" as defaults).
pub fn preferences_path(config_path: &std::path::Path) -> PathBuf {
    let dir = config_path.parent().map(PathBuf::from).unwrap_or_default();
    dir.join(PREFERENCES_FILE_NAME)
}

pub fn load(path: &std::path::Path) -> DesktopPreferences {
    let Ok(content) = std::fs::read_to_string(path) else {
        return DesktopPreferences::default();
    };
    toml::from_str(&content).unwrap_or_default()
}

pub fn save(path: &std::path::Path, prefs: &DesktopPreferences) -> Result<(), String> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|e| format!("create {} failed: {e}", parent.display()))?;
    }
    let toml_text =
        toml::to_string_pretty(prefs).map_err(|e| format!("serialize preferences failed: {e}"))?;
    std::fs::write(path, toml_text).map_err(|e| format!("write {} failed: {e}", path.display()))
}
