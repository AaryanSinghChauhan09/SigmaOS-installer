use serde::{Deserialize, Serialize};
use std::fs;
use anyhow::Result;

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct Config {
    pub enabled: bool,
    pub debug_mode: bool,
    pub log_level: String,
}

impl Default for Config {
    fn default() -> Self {
        Config {
            enabled: true,
            debug_mode: false,
            log_level: "info".to_string(),
        }
    }
}

impl Config {
    pub fn new(log_level: &str) -> Self {
        Config {
            enabled: true,
            debug_mode: false,
            log_level: log_level.to_string(),
        }
    }

    pub fn from_file(path: &str) -> Result<Self> {
        let content = fs::read_to_string(path)?;
        let config: Config = toml::from_str(&content)?;
        Ok(config)
    }

    pub fn to_file(&self, path: &str) -> Result<()> {
        let content = toml::to_string_pretty(self)?;
        fs::write(path, content)?;
        Ok(())
    }

    pub fn process(&self) -> Result<()> {
        if !self.enabled {
            return Ok(());
        }
        log::info!("Processing with log level: {}", self.log_level);
        Ok(())
    }
}
