//! Cross-platform autostart toggle.
//!
//! - Windows: writes `HKCU\Software\Microsoft\Windows\CurrentVersion\Run`.
//! - macOS / Linux: not yet implemented; returns Ok for status queries and
//!   an explicit error when toggled, so the UI can surface "unsupported".

#[cfg(target_os = "windows")]
mod platform {
    use winreg::RegKey;
    use winreg::enums::HKEY_CURRENT_USER;

    const RUN_KEY: &str = r"Software\Microsoft\Windows\CurrentVersion\Run";
    const VALUE_NAME: &str = "open-promux";

    pub fn is_enabled() -> Result<bool, String> {
        let hkcu = RegKey::predef(HKEY_CURRENT_USER);
        match hkcu.open_subkey(RUN_KEY) {
            Ok(key) => Ok(key.get_value::<String, _>(VALUE_NAME).is_ok()),
            Err(_) => Ok(false),
        }
    }

    pub fn set_enabled(enabled: bool) -> Result<(), String> {
        let hkcu = RegKey::predef(HKEY_CURRENT_USER);
        let (key, _) = hkcu
            .create_subkey(RUN_KEY)
            .map_err(|e| format!("open run key failed: {e}"))?;
        if enabled {
            let exe = std::env::current_exe().map_err(|e| format!("current_exe failed: {e}"))?;
            let path = format!("\"{}\"", exe.display());
            key.set_value(VALUE_NAME, &path)
                .map_err(|e| format!("write run key failed: {e}"))?;
        } else {
            let _ = key.delete_value(VALUE_NAME);
        }
        Ok(())
    }
}

#[cfg(not(target_os = "windows"))]
mod platform {
    pub fn is_enabled() -> Result<bool, String> {
        Ok(false)
    }

    pub fn set_enabled(_enabled: bool) -> Result<(), String> {
        Err("autostart toggle is not implemented on this platform yet".into())
    }
}

pub use platform::{is_enabled, set_enabled};
