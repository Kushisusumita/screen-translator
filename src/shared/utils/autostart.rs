use crate::shared::error::AppError;
use crate::shared::i18n::t;

#[cfg(windows)]
const AUTOSTART_VALUE: &str = "SakuraScreenTranslator";
/// Value name written by versions before the rename, cleaned up so a user who
/// upgrades does not end up with the app starting twice.
#[cfg(windows)]
const LEGACY_VALUE: &str = "ScreenTranslator";

/// Quotes the executable path for the `Run` key.
///
/// The value is handed to `CreateProcess`, which — for an unquoted path — tries
/// `C:\Program.exe`, then `C:\Program Files\Sakura.exe`, and so on. Without the
/// quotes, autostart from any path containing a space is ambiguous at best and
/// hijackable at worst.
///
/// The other platforms have no equivalent: a LaunchAgent takes an argument
/// vector and an XDG entry is parsed by the desktop, not by a shell.
#[cfg(any(windows, test))]
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
        .map_err(|e| {
            AppError::Other(
                t("Could not open the autostart registry key: {error}")
                    .replace("{error}", &e.to_string()),
            )
        })?;

    let _ = key.delete_value(LEGACY_VALUE);

    if enabled {
        key.set_value(AUTOSTART_VALUE, &quote_command(exe_path))
            .map_err(|e| {
                AppError::Other(
                    t("Could not write the autostart entry: {error}")
                        .replace("{error}", &e.to_string()),
                )
            })?;
    } else {
        // Absent is the desired state, so "not found" is success.
        let _ = key.delete_value(AUTOSTART_VALUE);
    }
    Ok(())
}

/// macOS: a per-user LaunchAgent. `launchd` reads the directory at login, so
/// writing the file is the whole operation — no `launchctl load` needed for the
/// next session.
#[cfg(target_os = "macos")]
pub fn set_autostart(enabled: bool, exe_path: &str) -> Result<(), AppError> {
    let path = launch_agent_path()?;

    if !enabled {
        // Absent is the desired state, so "not found" is success.
        if path.exists() {
            std::fs::remove_file(&path)
                .map_err(|e| {
                    AppError::Other(
                        t("Could not remove the autostart entry: {error}")
                            .replace("{error}", &e.to_string()),
                    )
                })?;
        }
        return Ok(());
    }

    let dir = path
        .parent()
        .ok_or_else(|| AppError::Other(t("Could not find the LaunchAgents folder").to_string()))?;
    std::fs::create_dir_all(dir)
        .map_err(|e| {
            AppError::Other(
                t("Could not create the LaunchAgents folder: {error}")
                    .replace("{error}", &e.to_string()),
            )
        })?;

    let plist = format!(
        r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
    <key>Label</key>
    <string>{label}</string>
    <key>ProgramArguments</key>
    <array>
        <string>{exe}</string>
    </array>
    <key>RunAtLoad</key>
    <true/>
    <key>KeepAlive</key>
    <false/>
</dict>
</plist>
"#,
        label = LAUNCH_AGENT_LABEL,
        exe = xml_escape(exe_path.trim().trim_matches('"')),
    );

    std::fs::write(&path, plist)
        .map_err(|e| {
            AppError::Other(
                t("Could not write the autostart entry: {error}")
                    .replace("{error}", &e.to_string()),
            )
        })?;
    Ok(())
}

#[cfg(target_os = "macos")]
const LAUNCH_AGENT_LABEL: &str = "com.sakura.screen-translator";

#[cfg(target_os = "macos")]
fn launch_agent_path() -> Result<std::path::PathBuf, AppError> {
    let home = dirs::home_dir()
        .ok_or_else(|| AppError::Other(t("Could not find the home folder").to_string()))?;
    Ok(home
        .join("Library/LaunchAgents")
        .join(format!("{LAUNCH_AGENT_LABEL}.plist")))
}

/// A path can legally contain `&` or `<`, which would otherwise produce a plist
/// `launchd` refuses to parse.
#[cfg(target_os = "macos")]
fn xml_escape(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
}

/// Linux: an XDG autostart entry. Every desktop environment that implements the
/// spec — GNOME, KDE, XFCE — starts it at login.
#[cfg(all(unix, not(target_os = "macos")))]
pub fn set_autostart(enabled: bool, exe_path: &str) -> Result<(), AppError> {
    let dir = dirs::config_dir()
        .ok_or_else(|| AppError::Other(t("Could not find the settings folder").to_string()))?
        .join("autostart");
    let path = dir.join("sakura-screen-translator.desktop");

    if !enabled {
        if path.exists() {
            std::fs::remove_file(&path)
                .map_err(|e| {
                    AppError::Other(
                        t("Could not remove the autostart entry: {error}")
                            .replace("{error}", &e.to_string()),
                    )
                })?;
        }
        return Ok(());
    }

    std::fs::create_dir_all(&dir)
        .map_err(|e| {
            AppError::Other(
                t("Could not create the autostart folder: {error}")
                    .replace("{error}", &e.to_string()),
            )
        })?;

    let entry = format!(
        "[Desktop Entry]\n\
         Type=Application\n\
         Name=Sakura Screen Translator\n\
         Exec={exe}\n\
         Terminal=false\n\
         X-GNOME-Autostart-enabled=true\n",
        exe = exe_path.trim().trim_matches('"'),
    );

    std::fs::write(&path, entry)
        .map_err(|e| {
            AppError::Other(
                t("Could not write the autostart entry: {error}")
                    .replace("{error}", &e.to_string()),
            )
        })?;
    Ok(())
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
