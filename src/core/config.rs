use crate::db::engine::dispatcher::types::DbConfig;
use anyhow::{Context, Result};
use std::fs;
use std::path::Path;

pub struct ConfigLoader;

impl ConfigLoader {
    pub fn load_from_file<P: AsRef<Path>>(path: P) -> Result<DbConfig> {
        let content = fs::read_to_string(path.as_ref()).context("Failed to read config file")?;

        let config: DbConfig =
            serde_yaml::from_str(&content).context("Failed to parse YAML config")?;

        Self::validate_config(&config)?;
        Ok(config)
    }

    pub fn save_to_file<P: AsRef<Path>>(path: P, config: &DbConfig) -> Result<()> {
        let yaml = serde_yaml::to_string(config).context("Failed to serialize config to YAML")?;

        let temp_path = path.as_ref().with_extension("yaml.tmp");
        fs::write(&temp_path, yaml).context("Failed to write temp config file")?;

        fs::rename(&temp_path, path.as_ref()).context("Failed to rename temp config file")?;

        Ok(())
    }

    pub fn validate_config(config: &DbConfig) -> Result<()> {
        if config.data_dir.is_empty() {
            anyhow::bail!("Config must specify a data_dir");
        }

        if config.repos.is_empty() {
            anyhow::bail!("Config must contain at least one repository");
        }

        Ok(())
    }
}
