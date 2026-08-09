//! Self-update.
//!
//! The original wrote a batch script into `%TEMP%`, interpolated two paths into
//! it unescaped, spawned `cmd`, and called `process::exit(0)`. That has three
//! problems worth naming: the downloaded bytes were never checked to be an
//! executable at all (a captive-portal HTML page returned with `200` would have
//! been copied over the installed binary, bricking it); anything in `%TEMP%` is
//! writable by every process running as the user, so the script could be
//! swapped between being written and being run; and a `%` anywhere in the
//! install path breaks batch expansion.
//!
//! Renaming a *running* executable is allowed on every platform this ships to
//! — Windows keeps the open handle with the file, and on Unix the process
//! holds the inode — which removes the need for a helper script entirely. The
//! new build is placed next to the old one, the old one is renamed out of the
//! way, the new one takes its name, and the caller restarts. Every step is
//! reversible until the last, and the leftover is cleaned up on the next
//! launch.

use std::path::{Path, PathBuf};

use futures::StreamExt;
use tracing::{info, warn};

use super::checker::url_is_allowed;
use crate::shared::i18n::t;

/// Sanity bounds for the download. The real binary is a few megabytes; anything
/// far outside that is not what we asked for.
const MIN_SIZE: usize = 256 * 1024;
const MAX_SIZE: usize = 200 * 1024 * 1024;

const OLD_SUFFIX: &str = ".old";

/// Downloads the new build and puts it in place, reporting progress as it goes.
///
/// `progress` is called with (received, total) as the body arrives; total is 0
/// when the server does not say. Streaming rather than `bytes()` is what makes
/// a progress bar possible at all — the old code waited for the whole ten
/// megabytes in one call, so the window said "downloading" and nothing else
/// until it was over.
///
/// Returns the path of the installed executable. Restarting is the caller's
/// job: this runs on a worker thread, and quitting from here skipped saving the
/// settings and left the tray icon behind.
pub async fn download_and_apply(
    url: &str,
    progress: impl Fn(u64, u64),
) -> Result<PathBuf, String> {
    if !url_is_allowed(url) {
        return Err(t("The update link does not point to GitHub — the install was cancelled").into());
    }

    let current = std::env::current_exe()
        .map_err(|e| t("Could not find the program path: {error}").replace("{error}", &e.to_string()))?;
    let dir = current
        .parent()
        .ok_or_else(|| t("Could not determine the install folder").to_string())?
        .to_path_buf();

    // Fail before downloading if the install directory is read-only, rather
    // than after.
    check_writable(&dir)?;

    let client = reqwest::Client::builder()
        .user_agent(concat!("screen-translator/", env!("CARGO_PKG_VERSION")))
        .timeout(std::time::Duration::from_secs(180))
        .build()
        .map_err(|e| {
            warn!(error = %e, "Could not build the download client");
            t("Could not reach the update server").to_string()
        })?;

    info!(url, "Downloading update");
    let resp = client.get(url).send().await.map_err(|e| {
        warn!(error = %e, "Update download failed");
        t("Could not reach the update server").to_string()
    })?;

    if !resp.status().is_success() {
        warn!(status = resp.status().as_u16(), "Update download refused");
        return Err(t("Could not download the update right now").to_string());
    }

    let total = resp.content_length().unwrap_or(0);
    validate_size_hint(total)?;

    let mut bytes: Vec<u8> = Vec::with_capacity(total as usize);
    let mut stream = resp.bytes_stream();
    progress(0, total);
    while let Some(chunk) = stream.next().await {
        let chunk = chunk.map_err(|e| {
            warn!(error = %e, "Update download interrupted");
            t("The download was interrupted").to_string()
        })?;
        bytes.extend_from_slice(&chunk);
        // A server that lies about the length, or none at all, must not let the
        // download grow without bound.
        if bytes.len() > MAX_SIZE {
            return Err(t("The update file is implausibly large").into());
        }
        progress(bytes.len() as u64, total);
    }

    validate_payload(&bytes)?;

    let staged = dir.join(format!(
        "{}.new",
        current.file_name().unwrap_or_default().to_string_lossy()
    ));
    std::fs::write(&staged, &bytes)
        .map_err(|e| t("Could not write the file: {error}").replace("{error}", &e.to_string()))?;

    let retired = PathBuf::from(format!("{}{OLD_SUFFIX}", current.to_string_lossy()));
    let _ = std::fs::remove_file(&retired);

    // Allowed while the program is running: Windows lets the open handle
    // follow the file, and on Unix the running process keeps the inode.
    if let Err(e) = std::fs::rename(&current, &retired) {
        let _ = std::fs::remove_file(&staged);
        return Err(
            t("Could not free the program file: {error}. You may need administrator rights.")
                .replace("{error}", &e.to_string()),
        );
    }

    if let Err(e) = std::fs::rename(&staged, &current) {
        // Put the working build back before giving up.
        let _ = std::fs::rename(&retired, &current);
        let _ = std::fs::remove_file(&staged);
        return Err(t("Could not install the update: {error}").replace("{error}", &e.to_string()));
    }

    // Keep the executable bit on the platforms that have one.
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(&current, std::fs::Permissions::from_mode(0o755));
    }

    info!("Update installed");
    Ok(current)
}

/// Rejects an implausible download before a byte of it is transferred.
fn validate_size_hint(total: u64) -> Result<(), String> {
    if total > MAX_SIZE as u64 {
        return Err(t("The update file is implausibly large").into());
    }
    Ok(())
}

/// Removes the previous build left behind by an update. Called at startup, when
/// the file is no longer in use.
pub fn cleanup_previous_version() {
    let Ok(current) = std::env::current_exe() else {
        return;
    };
    let retired = PathBuf::from(format!("{}{OLD_SUFFIX}", current.to_string_lossy()));
    if retired.exists() {
        match std::fs::remove_file(&retired) {
            Ok(()) => info!(path = %retired.display(), "Removed the previous version"),
            Err(e) => warn!(error = %e, "Could not remove the previous version yet"),
        }
    }
}

/// What an executable for this platform starts with. The check used to run on
/// Windows only, which left every other platform writing whatever came back
/// over the installed program.
#[cfg(windows)]
const EXECUTABLE_MAGICS: &[&[u8]] = &[b"MZ"];
#[cfg(target_os = "macos")]
const EXECUTABLE_MAGICS: &[&[u8]] = &[
    // Mach-O, 64-bit, little-endian — and the universal ("fat") wrapper that a
    // build for both Apple silicon and Intel produces.
    &[0xCF, 0xFA, 0xED, 0xFE],
    &[0xCA, 0xFE, 0xBA, 0xBE],
];
#[cfg(all(unix, not(target_os = "macos")))]
const EXECUTABLE_MAGICS: &[&[u8]] = &[&[0x7F, b'E', b'L', b'F']];

/// Confirms the bytes are an executable for this platform, of a plausible size.
///
/// Without this, any `200` response — a proxy error page, a login redirect —
/// would be written over the installed program.
fn validate_payload(bytes: &[u8]) -> Result<(), String> {
    validate_size(bytes.len())?;
    if !EXECUTABLE_MAGICS.iter().any(|m| bytes.starts_with(m)) {
        return Err(t("The downloaded file is not a program").into());
    }
    Ok(())
}

fn validate_size(len: usize) -> Result<(), String> {
    if len < MIN_SIZE {
        return Err(t("Only {size} KB was downloaded — that is not a program")
            .replace("{size}", &(len / 1024).to_string()));
    }
    if len > MAX_SIZE {
        return Err(t("The update file is implausibly large").into());
    }
    Ok(())
}

fn check_writable(dir: &Path) -> Result<(), String> {
    let probe = dir.join(".sakura-write-test");
    match std::fs::write(&probe, b"x") {
        Ok(()) => {
            let _ = std::fs::remove_file(&probe);
            Ok(())
        }
        Err(e) => Err(t("No write permission for {path}: {error}. Reinstall the program into a user folder or run the update as administrator.")
            .replace("{path}", &dir.display().to_string())
            .replace("{error}", &e.to_string())),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A payload that looks like an executable for whichever platform the tests
    /// are running on.
    fn fake_exe(len: usize) -> Vec<u8> {
        let magic = EXECUTABLE_MAGICS[0];
        let mut v = vec![0u8; len];
        v[..magic.len()].copy_from_slice(magic);
        v
    }

    #[test]
    fn a_real_looking_binary_passes() {
        assert!(validate_payload(&fake_exe(MIN_SIZE + 1)).is_ok());
    }

    #[test]
    fn an_html_error_page_is_rejected() {
        let page = b"<!DOCTYPE html><html><body>Sign in</body></html>".repeat(20_000);
        let err = validate_payload(&page).unwrap_err();
        assert!(
            err.contains(t("The downloaded file is not a program")),
            "{err}"
        );
    }

    #[test]
    fn a_truncated_download_is_rejected() {
        assert!(validate_payload(&fake_exe(1024)).is_err());
    }

    #[test]
    fn an_absurdly_large_payload_is_rejected() {
        // Size is checked on the length alone, so this needs no 200 MB buffer.
        let err = validate_size(MAX_SIZE + 1).unwrap_err();
        assert!(
            err.contains(t("The update file is implausibly large")),
            "{err}"
        );
        assert!(validate_size(MIN_SIZE).is_ok());
    }

    #[test]
    fn an_empty_body_is_rejected() {
        assert!(validate_payload(&[]).is_err());
    }
}
