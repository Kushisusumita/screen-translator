//! Where the app keeps its files.
//!
//! One answer, asked for in one place. The settings loader and the logger used
//! to work the directory out separately, which meant a change to one of them
//! quietly left the other writing somewhere else.

use std::path::{Path, PathBuf};

/// Overrides the data directory. Handy for a portable copy on a USB stick, for
/// keeping a second profile, and for running a development build without
/// touching the settings of the installed one.
pub const DATA_DIR_VAR: &str = "SAKURA_CONFIG_DIR";

/// The folder holding `config.toml` and `logs/`.
pub fn data_dir() -> PathBuf {
    if let Some(custom) = std::env::var_os(DATA_DIR_VAR) {
        if !custom.is_empty() {
            return PathBuf::from(custom);
        }
    }

    match dirs::config_dir() {
        Some(base) => base.join("screen-translator"),
        // Better than scattering files into whatever directory the app
        // happened to be launched from.
        None => std::env::current_exe()
            .ok()
            .and_then(|p| p.parent().map(Path::to_path_buf))
            .unwrap_or_else(|| PathBuf::from("."))
            .join("config"),
    }
}

pub fn config_path() -> PathBuf {
    data_dir().join("config.toml")
}

pub fn log_dir() -> PathBuf {
    data_dir().join("logs")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_config_and_the_logs_share_a_folder() {
        let config = config_path();
        let logs = log_dir();
        assert_eq!(config.parent(), logs.parent());
        assert!(config.ends_with("config.toml"));
        assert!(logs.ends_with("logs"));
    }
}
