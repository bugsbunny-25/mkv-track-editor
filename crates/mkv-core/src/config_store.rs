//! Loads/saves the TOML config file. Shared by the desktop app (OS config
//! dir) and the server (`MKV_CONFIG_DIR`), so both behave identically.

use crate::error::CoreError;
use mkv_profiles::AppConfig;
use std::path::{Path, PathBuf};

pub const CONFIG_FILE_NAME: &str = "config.toml";

pub fn config_file(dir: &Path) -> PathBuf {
    dir.join(CONFIG_FILE_NAME)
}

/// Loads the config, writing the defaults first if no file exists yet (so
/// users have something to find and edit). An existing but invalid file is
/// an error — it's never silently replaced.
pub async fn load_or_init(path: &Path) -> Result<AppConfig, CoreError> {
    match tokio::fs::read_to_string(path).await {
        Ok(text) => AppConfig::from_toml(&text)
            .map_err(|e| CoreError::Config(format!("{}: {e}", path.display()))),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            let config = AppConfig::default();
            save(path, &config).await?;
            Ok(config)
        }
        Err(e) => Err(CoreError::Io(e)),
    }
}

/// Validates, then writes atomically (temp file + rename) so a crash
/// mid-write can't leave a truncated config behind.
pub async fn save(path: &Path, config: &AppConfig) -> Result<(), CoreError> {
    config.validate().map_err(|e| CoreError::Config(e.to_string()))?;
    let text = config.to_toml().map_err(|e| CoreError::Config(e.to_string()))?;
    if let Some(dir) = path.parent() {
        tokio::fs::create_dir_all(dir).await?;
    }
    let tmp = path.with_extension("toml.tmp");
    tokio::fs::write(&tmp, text).await?;
    tokio::fs::rename(&tmp, path).await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn creates_defaults_then_round_trips() {
        let dir = tempfile::tempdir().unwrap();
        let path = config_file(&dir.path().join("nested"));
        let config = load_or_init(&path).await.unwrap();
        assert_eq!(config, AppConfig::default());
        assert!(path.exists(), "defaults are written for discoverability");

        let mut changed = config.clone();
        changed.naming.custom_formats.clear();
        save(&path, &changed).await.unwrap();
        assert_eq!(load_or_init(&path).await.unwrap(), changed);
    }

    #[tokio::test]
    async fn invalid_file_is_an_error_and_is_not_overwritten() {
        let dir = tempfile::tempdir().unwrap();
        let path = config_file(dir.path());
        std::fs::write(&path, "[naming]\nmovie_format = \"{Movie Title\"\n").unwrap();
        let err = load_or_init(&path).await.unwrap_err();
        assert!(err.to_string().contains("unclosed"), "{err}");
        assert!(std::fs::read_to_string(&path).unwrap().contains("{Movie Title\""));
    }

    #[tokio::test]
    async fn refuses_to_save_invalid_config() {
        let dir = tempfile::tempdir().unwrap();
        let mut config = AppConfig::default();
        config.profiles.clear();
        assert!(save(&config_file(dir.path()), &config).await.is_err());
        assert!(!config_file(dir.path()).exists());
    }
}
