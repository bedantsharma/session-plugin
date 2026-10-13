//! User settings in `<home>/config.toml`. Missing keys fall back to defaults.

use std::path::Path;

use serde::{Deserialize, Serialize};

use crate::error::{Error, Result};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Config {
    /// Agent-minutes per story point, for display only (spec §2).
    pub points_minutes: u32,
    /// Minutes of silence after which a session counts as crashed (spec §8.2).
    pub crash_timeout_min: u32,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            points_minutes: 15,
            crash_timeout_min: 30,
        }
    }
}

impl Config {
    /// Read the config file; a missing file means all defaults.
    pub fn load(path: &Path) -> Result<Self> {
        match std::fs::read_to_string(path) {
            Ok(text) => {
                toml::from_str(&text).map_err(|e| Error::Config(format!("{}: {e}", path.display())))
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Self::default()),
            Err(e) => Err(e.into()),
        }
    }

    /// Write a default config file unless one exists. Returns whether it wrote one.
    pub fn write_default_if_missing(path: &Path) -> Result<bool> {
        if path.exists() {
            return Ok(false);
        }
        let body = format!("# bridge-screen settings\n{}", Self::default().to_toml()?);
        std::fs::write(path, body)?;
        Ok(true)
    }

    pub fn to_toml(&self) -> Result<String> {
        toml::to_string(self).map_err(|e| Error::Config(e.to_string()))
    }
}

#[cfg(test)]
mod tests {
    use super::Config;
    use crate::error::Error;

    #[test]
    fn missing_file_gives_defaults() {
        let dir = tempfile::tempdir().unwrap();
        let cfg = Config::load(&dir.path().join("config.toml")).unwrap();
        assert_eq!(
            cfg,
            Config {
                points_minutes: 15,
                crash_timeout_min: 30
            }
        );
    }

    #[test]
    fn partial_file_fills_defaults() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        std::fs::write(&path, "points_minutes = 20\n").unwrap();
        let cfg = Config::load(&path).unwrap();
        assert_eq!(
            cfg,
            Config {
                points_minutes: 20,
                crash_timeout_min: 30
            }
        );
    }

    #[test]
    fn invalid_file_is_a_config_error() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        std::fs::write(&path, "points_minutes = \"lots\"\n").unwrap();
        assert!(matches!(Config::load(&path), Err(Error::Config(_))));
    }

    #[test]
    fn write_default_only_once() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        assert!(Config::write_default_if_missing(&path).unwrap());
        std::fs::write(&path, "points_minutes = 9\n").unwrap();
        assert!(!Config::write_default_if_missing(&path).unwrap());
        assert_eq!(Config::load(&path).unwrap().points_minutes, 9);
    }

    #[test]
    fn to_toml_round_trips() {
        let cfg = Config {
            points_minutes: 12,
            crash_timeout_min: 45,
        };
        let text = cfg.to_toml().unwrap();
        assert_eq!(toml::from_str::<Config>(&text).unwrap(), cfg);
    }
}
