//! Reading a translation aloud.
//!
//! Uses the speech synthesiser already present on the OS, so there is no extra
//! dependency and nothing is sent over the network.
//!
//! The text is piped in on **stdin**, never interpolated into the command line.
//! A translation is arbitrary text taken off the user's screen; putting it in a
//! shell command would be a command-injection hole with the attacker being
//! whatever webpage happened to be open.

use std::io::Write;
use std::process::Stdio;

// Only the paths that wait on a child process report how it went; macOS's
// `say` is fire-and-forget.
#[cfg(any(windows, all(unix, not(target_os = "macos"))))]
use tracing::{info, warn};

use crate::shared::error::AppError;
use crate::shared::i18n::t;

#[cfg(windows)]
const SCRIPT: &str = "\
Add-Type -AssemblyName System.Speech;
$text = [Console]::In.ReadToEnd();
if ($text.Trim().Length -gt 0) {
  $s = New-Object System.Speech.Synthesis.SpeechSynthesizer;
  $s.Speak($text);
}";

pub fn speak(text: &str) -> Result<(), AppError> {
    if text.trim().is_empty() {
        return Ok(());
    }

    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;

        let mut child = std::process::Command::new("powershell")
            .args(["-NoProfile", "-NonInteractive", "-Command", SCRIPT])
            .creation_flags(CREATE_NO_WINDOW)
            .stdin(Stdio::piped())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .map_err(|e| {
                AppError::Other(
                    t("Could not start speech synthesis: {error}")
                        .replace("{error}", &e.to_string()),
                )
            })?;

        if let Some(mut stdin) = child.stdin.take() {
            stdin
                .write_all(text.as_bytes())
                .map_err(|e| {
                    AppError::Other(
                        t("Could not send the text: {error}").replace("{error}", &e.to_string()),
                    )
                })?;
        }

        // Detached on purpose: speaking a paragraph takes seconds and must not
        // block the UI thread.
        std::thread::spawn(move || match child.wait() {
            Ok(status) if !status.success() => warn!(?status, "Speech synthesis exited badly"),
            Err(e) => warn!(error = %e, "Speech synthesis could not be awaited"),
            _ => info!("Speech finished"),
        });

        Ok(())
    }

    #[cfg(target_os = "macos")]
    {
        let mut child = std::process::Command::new("say")
            .stdin(Stdio::piped())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .map_err(|e| {
                AppError::Other(
                    t("Could not start speech synthesis: {error}")
                        .replace("{error}", &e.to_string()),
                )
            })?;
        if let Some(mut stdin) = child.stdin.take() {
            let _ = stdin.write_all(text.as_bytes());
        }
        std::thread::spawn(move || {
            let _ = child.wait();
        });
        Ok(())
    }

    // Linux and the other unixes: speech-dispatcher is what desktop screen
    // readers already talk to, so a system that speaks at all has it.
    #[cfg(all(unix, not(target_os = "macos")))]
    {
        let mut child = std::process::Command::new("spd-say")
            // Wait for the previous utterance instead of cutting it off, and
            // read stdin so the text never reaches the command line.
            .args(["--wait", "-e"])
            .stdin(Stdio::piped())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .map_err(|e| {
                AppError::Other(
                    t("Could not start spd-say: {error}").replace("{error}", &e.to_string()),
                )
            })?;

        if let Some(mut stdin) = child.stdin.take() {
            stdin
                .write_all(text.as_bytes())
                .map_err(|e| {
                    AppError::Other(
                        t("Could not send the text: {error}").replace("{error}", &e.to_string()),
                    )
                })?;
        }

        std::thread::spawn(move || match child.wait() {
            Ok(status) if !status.success() => warn!(?status, "Speech synthesis exited badly"),
            Err(e) => warn!(error = %e, "Speech synthesis could not be awaited"),
            _ => info!("Speech finished"),
        });

        Ok(())
    }
}
