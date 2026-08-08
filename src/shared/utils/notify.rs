//! Desktop notifications, each platform's own.
//!
//! Not a drawn-in-app banner: a toast in the Action Center on Windows, a
//! Notification Centre entry on macOS, and whatever the desktop's notification
//! daemon does on Linux. The app lives in the tray, so a message the user only
//! sees by opening its settings window is a message they do not see.
//!
//! Failure is never reported upwards. A notification that did not appear —
//! because the daemon is missing, or notifications are muted — is not a reason
//! to interrupt anything the caller was doing.

/// Shows a notification, or quietly does nothing if the desktop has no way to.
///
/// Both strings are treated as data: they are passed as arguments or through
/// escaping, never interpolated into a shell command or a script.
pub fn show(title: &str, body: &str) {
    #[cfg(windows)]
    windows_toast(title, body);
    #[cfg(target_os = "macos")]
    macos_notification(title, body);
    #[cfg(all(unix, not(target_os = "macos")))]
    freedesktop_notification(title, body);
}

#[cfg(windows)]
fn windows_toast(title: &str, body: &str) {
    use tauri_winrt_notification::{Duration, Sound, Toast};

    // The app is not installed through a package manifest, so it has no AUMID
    // of its own; PowerShell's is the documented stand-in and is what makes the
    // toast appear at all on Windows 10 and 11.
    let result = Toast::new(Toast::POWERSHELL_APP_ID)
        .title(title)
        .text1(body)
        .sound(Some(Sound::Default))
        .duration(Duration::Short)
        .show();

    if let Err(e) = result {
        tracing::debug!(error = %e, "Could not show a toast");
    }
}

/// macOS has no CLI for the modern notification API, and `UNUserNotificationCenter`
/// refuses to work for a process without an application bundle — which this is,
/// when run from `cargo run` or as a bare binary. `osascript` is the way that
/// works in both cases.
#[cfg(target_os = "macos")]
fn macos_notification(title: &str, body: &str) {
    let script = format!(
        "display notification {} with title {}",
        applescript_string(body),
        applescript_string(title)
    );
    spawn_detached("osascript", &["-e", &script]);
}

/// AppleScript string literal: quotes and backslashes escaped, control
/// characters dropped. Without this a translation containing a quote — which
/// arbitrary text off the screen certainly can — would end the literal and the
/// rest would be parsed as script.
#[cfg(target_os = "macos")]
fn applescript_string(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 2);
    out.push('"');
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            c if c.is_control() => out.push(' '),
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

/// `notify-send` is part of libnotify, which every desktop that shows
/// notifications at all already has.
#[cfg(all(unix, not(target_os = "macos")))]
fn freedesktop_notification(title: &str, body: &str) {
    // Arguments, not a command line, so nothing in the text is interpreted.
    spawn_detached(
        "notify-send",
        &["--app-name=Sakura Screen Translator", title, body],
    );
}

/// Starts the helper and forgets about it: waiting would block the caller for
/// as long as the notification is on screen.
#[cfg(unix)]
fn spawn_detached(program: &str, args: &[&str]) {
    use std::process::Stdio;

    match std::process::Command::new(program)
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
    {
        Ok(mut child) => {
            // Reaped on a thread so the process does not sit as a zombie for
            // the lifetime of the app.
            std::thread::spawn(move || {
                let _ = child.wait();
            });
        }
        Err(e) => tracing::debug!(error = %e, program, "Could not show a notification"),
    }
}

#[cfg(all(test, target_os = "macos"))]
mod tests {
    use super::applescript_string;

    #[test]
    fn a_quote_cannot_end_the_literal() {
        assert_eq!(
            applescript_string(r#"say "hi""#),
            r#""say \"hi\"""#
        );
    }

    #[test]
    fn a_backslash_is_escaped_before_the_quotes_are() {
        assert_eq!(applescript_string(r"back\slash"), r#""back\\slash""#);
    }

    #[test]
    fn newlines_do_not_break_the_script() {
        assert_eq!(applescript_string("two\nlines"), "\"two lines\"");
    }

    #[test]
    fn ordinary_text_is_left_alone() {
        assert_eq!(applescript_string("Версия 1.2.3"), "\"Версия 1.2.3\"");
    }
}
