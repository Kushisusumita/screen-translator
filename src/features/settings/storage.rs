use crate::entities::settings::Settings;
use crate::shared::error::AppError;
use std::path::PathBuf;

pub fn config_path() -> PathBuf {
    let base = dirs::config_dir()
        .unwrap_or_else(|| PathBuf::from("."));
    base.join("screen-translator").join("config.toml")
}

pub fn load_settings() -> Settings {
    match try_load_settings() {
        Ok(s) => s,
        Err(e) => {
            tracing::warn!("Failed to load settings: {}. Using defaults.", e);
            Settings::default()
        }
    }
}

fn try_load_settings() -> Result<Settings, AppError> {
    let path = config_path();
    if !path.exists() {
        return Ok(Settings::default());
    }
    let content = std::fs::read_to_string(&path)?;
    let settings: Settings = toml::from_str(&content)?;
    Ok(settings)
}

pub fn save_settings(settings: &Settings) -> Result<(), AppError> {
    let path = config_path();
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let content = toml::to_string_pretty(settings)?;
    std::fs::write(&path, content)?;
    Ok(())
}
