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
//! Windows lets you rename a *running* executable, which removes the need for a
//! helper script entirely. The new build is placed next to the old one, the old
//! one is renamed out of the way, the new one takes its name, and the process
//! restarts. Every step is reversible until the last, and the leftover is
//! cleaned up on the next launch.

use std::path::{Path, PathBuf};

use tracing::{info, warn};

use super::checker::url_is_allowed;
use crate::shared::i18n::t;

/// Sanity bounds for the download. The real binary is a few megabytes; anything
/// far outside that is not what we asked for.
const MIN_SIZE: usize = 256 * 1024;
const MAX_SIZE: usize = 200 * 1024 * 1024;

const OLD_SUFFIX: &str = ".old";

pub async fn download_and_apply(url: &str) -> Result<(), String> {
    if !url_is_allowed(url) {
        return Err(
            t("The update link does not point to GitHub — the install was cancelled").into(),
        );
    }

    let current = std::env::current_exe().map_err(|e| {
        t("Could not find the program path: {error}").replace("{error}", &e.to_string())
    })?;
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
        .map_err(|e| e.to_string())?;

    info!(url, "Downloading update");
    let resp = client
        .get(url)
        .send()
        .await
        .map_err(|e| t("Download failed: {error}").replace("{error}", &e.to_string()))?;

    if !resp.status().is_success() {
        return Err(
            t("The server returned HTTP {status}").replace("{status}", &resp.status().to_string())
        );
    }

    let bytes = resp.bytes().await.map_err(|e| {
        t("The download was interrupted: {error}").replace("{error}", &e.to_string())
    })?;

    validate_payload(&bytes)?;

    let staged = dir.join(format!(
        "{}.new",
        current.file_name().unwrap_or_default().to_string_lossy()
    ));
    std::fs::write(&staged, &bytes)
        .map_err(|e| t("Could not write the file: {error}").replace("{error}", &e.to_string()))?;

    let retired = PathBuf::from(format!("{}{OLD_SUFFIX}", current.to_string_lossy()));
    let _ = std::fs::remove_file(&retired);

    // Renaming a running executable is allowed on Windows; the open handle
    // follows the file.
    if let Err(e) = std::fs::rename(&current, &retired) {
        let _ = std::fs::remove_file(&staged);
        return Err(t(
            "Could not free the program file: {error}. You may need administrator rights.",
        )
        .replace("{error}", &e.to_string()));
    }

    if let Err(e) = std::fs::rename(&staged, &current) {
        // Put the working build back before giving up.
        let _ = std::fs::rename(&retired, &current);
        let _ = std::fs::remove_file(&staged);
        return Err(t("Could not install the update: {error}").replace("{error}", &e.to_string()));
    }

    info!("Update installed, restarting");
    match std::process::Command::new(&current).spawn() {
        Ok(_) => {
            std::process::exit(0);
        }
        Err(e) => Err(t(
            "The update was installed but the restart failed: {error}. Start the program manually.",
        )
        .replace("{error}", &e.to_string())),
    }
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
