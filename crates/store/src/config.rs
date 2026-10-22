//! User settings, stored as TOML in the platform config directory
//! (`~/Library/Application Support/app.writer.writer/config.toml` on macOS).

use std::path::{Path, PathBuf};

use directories::ProjectDirs;
use serde::{Deserialize, Serialize};
use writer_llm::ProviderKind;

use crate::{Result, StoreError};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Config {
    pub provider: ProviderKind,
    /// Overrides the provider's default check model.
    pub check_model: Option<String>,
    /// Overrides the provider's default rewrite model.
    pub rewrite_model: Option<String>,
    /// LLM calls stop once this month's spend reaches the cap. 0 disables the cap.
    pub monthly_cap_usd: f64,
    pub locale: String,
}

impl Default for Config {
    fn default() -> Self {
        Config {
            provider: ProviderKind::Claude,
            check_model: None,
            rewrite_model: None,
            monthly_cap_usd: 10.0,
            locale: "en-US".to_owned(),
        }
    }
}

impl Config {
    pub fn check_model(&self) -> String {
        self.check_model
            .clone()
            .unwrap_or_else(|| self.provider.default_check_model().to_owned())
    }

    pub fn rewrite_model(&self) -> String {
        self.rewrite_model
            .clone()
            .unwrap_or_else(|| self.provider.default_rewrite_model().to_owned())
    }

    /// Reads the file, or returns the defaults if it does not exist yet.
    pub fn load(path: &Path) -> Result<Config> {
        match std::fs::read_to_string(path) {
            Ok(text) => toml::from_str(&text).map_err(|e| StoreError::Config {
                path: path.to_owned(),
                message: e.to_string(),
            }),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Config::default()),
            Err(source) => Err(StoreError::Io {
                path: path.to_owned(),
                source,
            }),
        }
    }

    pub fn save(&self, path: &Path) -> Result<()> {
        let io_err = |source| StoreError::Io {
            path: path.to_owned(),
            source,
        };
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir).map_err(io_err)?;
        }
        let text = toml::to_string_pretty(self).map_err(|e| StoreError::Config {
            path: path.to_owned(),
            message: e.to_string(),
        })?;
        std::fs::write(path, text).map_err(io_err)
    }
}

/// Where the config file and database live on this machine.
pub struct Paths {
    pub config_file: PathBuf,
    pub database: PathBuf,
}

impl Paths {
    pub fn detect() -> Result<Paths> {
        let dirs = ProjectDirs::from("app", "writer", "writer").ok_or(StoreError::NoHome)?;
        Ok(Paths {
            config_file: dirs.config_dir().join("config.toml"),
            database: dirs.data_dir().join("writer.db"),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_follow_the_provider() {
        let mut config = Config::default();
        assert_eq!(config.check_model(), "claude-haiku-5-5");
        config.provider = ProviderKind::OpenAi;
        assert_eq!(config.check_model(), "gpt-6-luna");
        assert_eq!(config.rewrite_model(), "gpt-6.1-sol");
        config.rewrite_model = Some("gpt-6-astra".to_owned());
        assert_eq!(config.rewrite_model(), "gpt-6-astra");
    }

    #[test]
    fn save_then_load_round_trips() {
        let dir = std::env::temp_dir().join(format!("writer-config-test-{}", std::process::id()));
        let path = dir.join("config.toml");
        let config = Config {
            provider: ProviderKind::OpenAi,
            monthly_cap_usd: 5.0,
            ..Config::default()
        };
        config.save(&path).unwrap();
        assert_eq!(Config::load(&path).unwrap(), config);
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn missing_file_gives_defaults_and_partial_file_fills_the_rest() {
        let missing = std::env::temp_dir().join("writer-definitely-missing/config.toml");
        assert_eq!(Config::load(&missing).unwrap(), Config::default());

        let partial: Config = toml::from_str("provider = \"openai\"").unwrap();
        assert_eq!(partial.provider, ProviderKind::OpenAi);
        assert_eq!(partial.monthly_cap_usd, 10.0);
    }
}
