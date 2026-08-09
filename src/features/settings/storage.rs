//! Reading and writing `config.toml`.
//!
//! Two failure modes are handled explicitly because both silently destroyed
//! user settings before:
//!
//! * the file was written with a plain `fs::write`, so a crash or the updater's
//!   `process::exit` partway through left a truncated file that parsed as
//!   nothing and reset every setting. Writes now go to a temp file and are moved
//!   into place, which is atomic on both NTFS and APFS;
//! * a config that failed to parse was replaced by defaults and then
//!   overwritten, losing whatever was in it. Now it is copied aside first.

use std::path::{Path, PathBuf};

use tracing::{error, info, warn};

use crate::entities::settings::{LogSettings, Settings};
use crate::shared::error::AppError;

pub fn config_dir() -> PathBuf {
    match dirs::config_dir() {
        Some(base) => base.join("screen-translator"),
        None => {
            // Better than silently scattering config into whatever directory the
            // app happened to be launched from.
            warn!("No config directory from the OS; falling back to the executable's folder");
            std::env::current_exe()
                .ok()
                .and_then(|p| p.parent().map(Path::to_path_buf))
                .unwrap_or_else(|| PathBuf::from("."))
                .join("config")
        }
    }
}

pub fn config_path() -> PathBuf {
    config_dir().join("config.toml")
}

/// Reads just the logging section.
///
/// Logging has to be running before the full settings load, so that a migration
/// or a broken-config warning has somewhere to go. Parsing the file twice costs
/// nothing and keeps that ordering honest.
pub fn load_log_settings() -> LogSettings {
    #[derive(serde::Deserialize, Default)]
    struct LogsOnly {
        #[serde(default)]
        logs: LogSettings,
    }

    std::fs::read_to_string(config_path())
        .ok()
        .and_then(|text| toml::from_str::<LogsOnly>(&text).ok())
        .unwrap_or_default()
        .logs
}

pub fn load_settings() -> Settings {
    let path = config_path();
    if !path.exists() {
        info!(path = %path.display(), "No config yet, starting from defaults");
        return Settings::default();
    }

    let content = match std::fs::read_to_string(&path) {
        Ok(c) => c,
        Err(e) => {
            error!(error = %e, path = %path.display(), "Cannot read config");
            return Settings::default();
        }
    };

    match toml::from_str::<Settings>(&content) {
        Ok(mut settings) => {
            // An old file parses cleanly into all-defaults, so the migration has
            // to be attempted on success, not only on failure.
            let legacy: super::migrate::LegacySettings =
                toml::from_str(&content).unwrap_or_default();
            if super::migrate::apply(&mut settings, &legacy) {
                let backup = path.with_extension("toml.v0");
                match std::fs::write(&backup, &content) {
                    Ok(()) => info!(backup = %backup.display(), "Kept a copy of the old config"),
                    Err(e) => warn!(error = %e, "Could not back up the old config"),
                }
            }
            settings
        }
        Err(e) => {
            let backup = path.with_extension("toml.broken");
            match std::fs::write(&backup, &content) {
                Ok(_) => warn!(
                    error = %e,
                    backup = %backup.display(),
                    "Config did not parse; the original was kept and defaults are in use"
                ),
                Err(be) => error!(
                    error = %e,
                    backup_error = %be,
                    "Config did not parse and could not be backed up"
                ),
            }
            Settings::default()
        }
    }
}

pub fn save_settings(settings: &Settings) -> Result<(), AppError> {
    let path = config_path();
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }

    let content = toml::to_string_pretty(settings)?;

    // Same directory as the target so the rename stays on one volume.
    let tmp = path.with_extension("toml.tmp");
    std::fs::write(&tmp, content.as_bytes())?;

    // Windows rename fails if the destination exists, so replace explicitly.
    match std::fs::rename(&tmp, &path) {
        Ok(()) => Ok(()),
        Err(_) => {
            std::fs::copy(&tmp, &path)?;
            let _ = std::fs::remove_file(&tmp);
            Ok(())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_config_path_sits_under_the_config_dir() {
        let p = config_path();
        assert!(p.ends_with("config.toml"));
        assert!(p.parent().unwrap().ends_with("screen-translator"));
    }

    #[test]
    fn defaults_survive_a_round_trip_through_toml() {
        let s = Settings::default();
        let text = toml::to_string_pretty(&s).expect("serialise");
        let back: Settings = toml::from_str(&text).expect("deserialise");
        assert_eq!(back.target_lang, s.target_lang);
        assert_eq!(back.result_view, s.result_view);
        assert_eq!(back.hotkeys.region.key, s.hotkeys.region.key);
    }
}
