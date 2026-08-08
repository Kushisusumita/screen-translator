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

/// Sanity bounds for the download. The real binary is a few megabytes; anything
/// far outside that is not what we asked for.
const MIN_SIZE: usize = 256 * 1024;
const MAX_SIZE: usize = 200 * 1024 * 1024;

const OLD_SUFFIX: &str = ".old";

pub async fn download_and_apply(url: &str) -> Result<(), String> {
    if !url_is_allowed(url) {
        return Err("ссылка на обновление ведёт не на GitHub — установка отменена".into());
    }

    let current =
        std::env::current_exe().map_err(|e| format!("не найден путь к программе: {e}"))?;
    let dir = current
        .parent()
        .ok_or_else(|| "не определить папку установки".to_string())?
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
        .map_err(|e| format!("не удалось скачать: {e}"))?;

    if !resp.status().is_success() {
        return Err(format!("сервер ответил HTTP {}", resp.status()));
    }

    let bytes = resp
        .bytes()
        .await
        .map_err(|e| format!("обрыв загрузки: {e}"))?;

    validate_payload(&bytes)?;

    let staged = dir.join(format!(
        "{}.new",
        current.file_name().unwrap_or_default().to_string_lossy()
    ));
    std::fs::write(&staged, &bytes).map_err(|e| format!("не удалось записать файл: {e}"))?;

    let retired = PathBuf::from(format!("{}{OLD_SUFFIX}", current.to_string_lossy()));
    let _ = std::fs::remove_file(&retired);

    // Renaming a running executable is allowed on Windows; the open handle
    // follows the file.
    if let Err(e) = std::fs::rename(&current, &retired) {
        let _ = std::fs::remove_file(&staged);
        return Err(format!(
            "не удалось освободить файл программы: {e}. \
             Возможно, нужны права администратора."
        ));
    }

    if let Err(e) = std::fs::rename(&staged, &current) {
        // Put the working build back before giving up.
        let _ = std::fs::rename(&retired, &current);
        let _ = std::fs::remove_file(&staged);
        return Err(format!("не удалось установить обновление: {e}"));
    }

    info!("Update installed, restarting");
    match std::process::Command::new(&current).spawn() {
        Ok(_) => {
            std::process::exit(0);
        }
        Err(e) => Err(format!(
            "обновление установлено, но перезапуск не удался: {e}. Запустите программу вручную."
        )),
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

/// Confirms the bytes are a Windows executable of a plausible size.
///
/// Without this, any `200` response — a proxy error page, a login redirect —
/// would be written over the installed program.
fn validate_payload(bytes: &[u8]) -> Result<(), String> {
    validate_size(bytes.len())?;
    if cfg!(windows) && !bytes.starts_with(b"MZ") {
        return Err("скачанный файл не является программой Windows".into());
    }
    Ok(())
}

fn validate_size(len: usize) -> Result<(), String> {
    if len < MIN_SIZE {
        return Err(format!(
            "скачано всего {} КБ — это не программа",
            len / 1024
        ));
    }
    if len > MAX_SIZE {
        return Err("файл обновления неправдоподобно велик".into());
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
        Err(e) => Err(format!(
            "нет прав на запись в {}: {e}. Переустановите программу в папку пользователя \
             или запустите обновление от администратора.",
            dir.display()
        )),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fake_exe(len: usize) -> Vec<u8> {
        let mut v = vec![0u8; len];
        v[0] = b'M';
        v[1] = b'Z';
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
        assert!(err.contains("не является программой"), "{err}");
    }

    #[test]
    fn a_truncated_download_is_rejected() {
        assert!(validate_payload(&fake_exe(1024)).is_err());
    }

    #[test]
    fn an_absurdly_large_payload_is_rejected() {
        // Size is checked on the length alone, so this needs no 200 MB buffer.
        let err = validate_size(MAX_SIZE + 1).unwrap_err();
        assert!(err.contains("велик"), "{err}");
        assert!(validate_size(MIN_SIZE).is_ok());
    }

    #[test]
    fn an_empty_body_is_rejected() {
        assert!(validate_payload(&[]).is_err());
    }
}
