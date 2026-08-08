use crate::shared::error::AppError;

const AUTOSTART_VALUE: &str = "SakuraScreenTranslator";
/// Value name written by versions before the rename, cleaned up so a user who
/// upgrades does not end up with the app starting twice.
const LEGACY_VALUE: &str = "ScreenTranslator";

/// Quotes the executable path for the `Run` key.
///
/// The value is handed to `CreateProcess`, which — for an unquoted path — tries
/// `C:\Program.exe`, then `C:\Program Files\Sakura.exe`, and so on. Without the
/// quotes, autostart from any path containing a space is ambiguous at best and
/// hijackable at worst.
pub fn quote_command(exe_path: &str) -> String {
    let trimmed = exe_path.trim().trim_matches('"');
    format!("\"{trimmed}\"")
}

#[cfg(windows)]
pub fn set_autostart(enabled: bool, exe_path: &str) -> Result<(), AppError> {
    use winreg::enums::{HKEY_CURRENT_USER, KEY_SET_VALUE};
    use winreg::RegKey;

    let hkcu = RegKey::predef(HKEY_CURRENT_USER);
    let (key, _) = hkcu
        .create_subkey_with_flags(
            r"Software\Microsoft\Windows\CurrentVersion\Run",
            KEY_SET_VALUE,
        )
        .map_err(|e| AppError::Other(format!("не открыть ключ автозапуска: {e}")))?;

    let _ = key.delete_value(LEGACY_VALUE);

    if enabled {
        key.set_value(AUTOSTART_VALUE, &quote_command(exe_path))
            .map_err(|e| AppError::Other(format!("не записать автозапуск: {e}")))?;
    } else {
        // Absent is the desired state, so "not found" is success.
        let _ = key.delete_value(AUTOSTART_VALUE);
    }
    Ok(())
}

#[cfg(not(windows))]
pub fn set_autostart(_enabled: bool, _exe_path: &str) -> Result<(), AppError> {
    // macOS wants a LaunchAgent plist in ~/Library/LaunchAgents.
    Err(AppError::Other(
        "автозапуск на этой платформе пока не реализован".into(),
    ))
}

pub fn get_current_exe_path() -> String {
    std::env::current_exe()
        .map(|p| p.to_string_lossy().to_string())
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::quote_command;

    #[test]
    fn a_path_with_spaces_is_quoted() {
        assert_eq!(
            quote_command(r"C:\Program Files\Sakura\app.exe"),
            "\"C:\\Program Files\\Sakura\\app.exe\""
        );
    }

    #[test]
    fn an_already_quoted_path_is_not_double_quoted() {
        assert_eq!(
            quote_command("\"C:\\a b\\app.exe\""),
            "\"C:\\a b\\app.exe\""
        );
    }

    #[test]
    fn surrounding_whitespace_is_dropped() {
        assert_eq!(quote_command("  C:\\app.exe  "), "\"C:\\app.exe\"");
    }
}
