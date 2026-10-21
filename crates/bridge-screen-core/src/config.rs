//! User settings in `<home>/config.toml`. Missing keys fall back to defaults.

use std::collections::BTreeMap;
use std::path::Path;

use serde::{Deserialize, Serialize};

use crate::error::{Error, Result};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Config {
    /// Agent-minutes per story point, for display only (spec §2).
    pub points_minutes: u32,
    /// Minutes of silence after which a session counts as crashed (spec §8.2).
    pub crash_timeout_min: u32,
    /// Hours a crashed session must have been silent before an agent in another worktree
    /// may act on its leftovers; until then they are report-only (the owner may still
    /// resolve them). A "crashed" session may just have an owner who stepped away.
    pub crash_takeover_hours: u32,
    /// Drift scores below this are on-task without asking (spec §7.2). Tuned per repo.
    pub drift_lower: f64,
    /// Drift scores above this are flagged as possible drift. Tuned per repo.
    pub drift_upper: f64,
    /// Away time is always tolerated up to this many minutes (spec §7.3).
    pub tolerance_min_minutes: u32,
    /// …or up to this percentage of the active task's estimate, if larger. Tuned per repo.
    pub tolerance_pct: u32,
    /// Seconds the L2 re-run of the test command may take (spec §8.5); a repo's own
    /// `test_timeout_s` (`bridge-screen config set test-timeout`) overrides it.
    pub verify_timeout_s: u64,
    /// The dashboard's port on 127.0.0.1 (`bridge-screen ui`); 0 = any free port.
    pub ui_port: u16,
    /// USD per million tokens, per model id or id prefix (spec §9.2). Entries in the file
    /// override the built-in defaults key by key; models with no entry are unpriced.
    pub prices: BTreeMap<String, Price>,
}

/// A model's prices in USD per million tokens.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct Price {
    pub input: f64,
    pub output: f64,
    pub cache_read: f64,
    pub cache_write_5m: f64,
    pub cache_write_1h: f64,
}

/// The price for `model`: its exact id, else the longest configured prefix of it
/// (`claude-opus-5` prices `claude-opus-5-1`), else none.
pub fn price_for<'a>(prices: &'a BTreeMap<String, Price>, model: &str) -> Option<&'a Price> {
    if let Some(p) = prices.get(model) {
        return Some(p);
    }
    prices
        .iter()
        .filter(|(k, _)| !k.is_empty() && model.starts_with(k.as_str()))
        .max_by_key(|(k, _)| k.len())
        .map(|(_, p)| p)
}

/// The built-in price table.
pub fn default_prices() -> BTreeMap<String, Price> {
    BTreeMap::new()
}

impl Default for Config {
    fn default() -> Self {
        Self {
            points_minutes: 15,
            crash_timeout_min: 30,
            crash_takeover_hours: 24,
            drift_lower: 0.3,
            drift_upper: 0.7,
            tolerance_min_minutes: 15,
            tolerance_pct: 20,
            verify_timeout_s: 120,
            ui_port: 0,
            prices: default_prices(),
        }
    }
}

impl Config {
    /// Read the config file; a missing file means all defaults.
    pub fn load(path: &Path) -> Result<Self> {
        match std::fs::read_to_string(path) {
            Ok(text) => {
                let mut cfg: Self = toml::from_str(&text)
                    .map_err(|e| Error::Config(format!("{}: {e}", path.display())))?;
                // The owner's prices override the defaults key by key.
                for (model, price) in default_prices() {
                    cfg.prices.entry(model).or_insert(price);
                }
                Ok(cfg)
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
                crash_timeout_min: 30,
                crash_takeover_hours: 24,
                drift_lower: 0.3,
                drift_upper: 0.7,
                tolerance_min_minutes: 15,
                tolerance_pct: 20,
                verify_timeout_s: 120,
                ui_port: 0,
                prices: super::default_prices(),
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
                ..Config::default()
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
            crash_takeover_hours: 6,
            drift_lower: 0.25,
            drift_upper: 0.8,
            tolerance_min_minutes: 10,
            tolerance_pct: 30,
            verify_timeout_s: 300,
            ui_port: 8787,
            prices: super::default_prices(),
        };
        let text = cfg.to_toml().unwrap();
        assert_eq!(toml::from_str::<Config>(&text).unwrap(), cfg);
    }
}
