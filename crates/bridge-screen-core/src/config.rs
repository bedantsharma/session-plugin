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
    #[serde(skip_serializing_if = "BTreeMap::is_empty")]
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
/// (`claude-opus-5` prices `claude-opus-5-1`), else none. A fast-mode model
/// (`claude-opus-5-5 (fast)`) matches only fast entries, by the same rule on the base id,
/// so it is never priced at the standard rate.
pub fn price_for<'a>(prices: &'a BTreeMap<String, Price>, model: &str) -> Option<&'a Price> {
    if let Some(p) = prices.get(model) {
        return Some(p);
    }
    let fast = crate::telemetry::FAST;
    let (base, is_fast) = match model.strip_suffix(fast) {
        Some(base) => (base, true),
        None => (model, false),
    };
    prices
        .iter()
        .filter_map(|(k, p)| match (k.strip_suffix(fast), is_fast) {
            (Some(kb), true) => Some((kb, p)),
            (None, false) => Some((k.as_str(), p)),
            _ => None,
        })
        .filter(|(k, _)| !k.is_empty() && base.starts_with(k))
        .max_by_key(|(k, _)| k.len())
        .map(|(_, p)| p)
}

/// Where the built-in prices came from, written into a new `config.toml`.
pub const PRICES_SOURCE: &str = "\
# Prices are USD per million tokens, per model id (or id prefix: the longest one that
# matches is used). The defaults come from the claude-api skill's pricing reference
# (Anthropic first-party API rates, cached 2026-10-06), taken 2026-10-21: input and
# output per model; cache reads at 0.1x input (0.05x on Opus 5.5, 0.025x on Fable 5.1 and
# Mythos 5.1); cache writes at 1.25x input (5-minute) and 2x input (1-hour).
# Not included, so their cost shows as unknown (with a 'no price for' note) rather than
# a wrong number:
# - Haiku 5.5: $0.10 / $0.50 for prompts up to 100K tokens, $0.50 / $2.50 above that;
#   one rate per model can't express two.
# - Fast mode (replies marked fast, shown as e.g. claude-opus-5-5 (fast)): $8 / $40
#   on Opus 5.5 and $10 / $50 on Opus 5, but the reference gives no cache rates for it.
#   Add an entry such as [prices.\"claude-opus-5-5 (fast)\"] to price it; fast replies are
#   never priced at the standard rate.
# Entries here override the defaults key by key.
";

/// The built-in price table (see [`PRICES_SOURCE`]).
pub fn default_prices() -> BTreeMap<String, Price> {
    // (model, input, output, cache read); writes are 1.25x / 2x input.
    const TABLE: &[(&str, f64, f64, f64)] = &[
        ("claude-fable-5-1", 10.0, 50.0, 0.25),
        ("claude-mythos-5-1", 10.0, 50.0, 0.25),
        ("claude-fable-5", 10.0, 50.0, 1.0),
        ("claude-mythos-5", 10.0, 50.0, 1.0),
        ("claude-opus-5-5", 4.0, 20.0, 0.2),
        ("claude-opus-5", 5.0, 25.0, 0.5),
        ("claude-opus-4-8", 5.0, 25.0, 0.5),
        ("claude-opus-4-7", 5.0, 25.0, 0.5),
        ("claude-opus-4-6", 5.0, 25.0, 0.5),
        ("claude-sonnet-5-5", 2.0, 10.0, 0.2),
        ("claude-sonnet-5", 2.0, 10.0, 0.2),
        ("claude-sonnet-4-6", 3.0, 15.0, 0.3),
        ("claude-haiku-4-5", 1.0, 5.0, 0.1),
    ];
    TABLE
        .iter()
        .map(|&(model, input, output, cache_read)| {
            (
                model.to_string(),
                Price {
                    input,
                    output,
                    cache_read,
                    cache_write_5m: input * 1.25,
                    cache_write_1h: input * 2.0,
                },
            )
        })
        .collect()
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
        let body = format!(
            "# bridge-screen settings\n{PRICES_SOURCE}{}",
            Self::default().to_toml()?
        );
        std::fs::write(path, body)?;
        Ok(true)
    }

    /// Whether the price entry for `model` (an exact key) is a built-in `default` or the
    /// owner's own (`yours`); `None` without an entry.
    pub fn price_source(&self, model: &str) -> Option<&'static str> {
        let mine = self.prices.get(model)?;
        Some(if default_prices().get(model) == Some(mine) {
            "default"
        } else {
            "yours"
        })
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

    fn price(input: f64) -> super::Price {
        super::Price {
            input,
            output: input * 5.0,
            cache_read: input / 10.0,
            cache_write_5m: input * 1.25,
            cache_write_1h: input * 2.0,
        }
    }

    #[test]
    fn prices_match_exactly_or_by_the_longest_prefix() {
        let prices = std::collections::BTreeMap::from([
            ("claude-opus-5".to_string(), price(5.0)),
            ("claude-opus-5-5".to_string(), price(4.0)),
            ("claude".to_string(), price(1.0)),
        ]);
        let input = |m: &str| super::price_for(&prices, m).map(|p| p.input);
        assert_eq!(input("claude-opus-5-5"), Some(4.0));
        assert_eq!(input("claude-opus-5-5-fast"), Some(4.0));
        assert_eq!(input("claude-opus-5-1"), Some(5.0));
        assert_eq!(input("claude-sonnet-9"), Some(1.0));
        assert_eq!(input("gpt-4"), None);
    }

    #[test]
    fn default_prices_and_owner_overrides() {
        let defaults = super::default_prices();
        let opus = defaults["claude-opus-5-5"];
        assert_eq!((opus.input, opus.output, opus.cache_read), (4.0, 20.0, 0.2));
        assert_eq!((opus.cache_write_5m, opus.cache_write_1h), (5.0, 8.0));
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        // A file without [prices] gets the defaults.
        std::fs::write(&path, "points_minutes = 20\n").unwrap();
        assert_eq!(Config::load(&path).unwrap().prices, defaults);
        // The owner's entries override defaults key by key, and add models.
        std::fs::write(
            &path,
            "[prices.claude-opus-5-5]\ninput = 1.0\noutput = 2.0\ncache_read = 0.1\n\
             cache_write_5m = 1.25\ncache_write_1h = 2.0\n\n\
             [prices.\"my-model\"]\ninput = 3.0\noutput = 4.0\ncache_read = 0.3\n\
             cache_write_5m = 3.75\ncache_write_1h = 6.0\n",
        )
        .unwrap();
        let cfg = Config::load(&path).unwrap();
        assert_eq!(cfg.prices["claude-opus-5-5"].input, 1.0);
        assert_eq!(cfg.prices["my-model"].output, 4.0);
        assert_eq!(
            cfg.prices["claude-sonnet-5-5"],
            defaults["claude-sonnet-5-5"]
        );
        assert_eq!(cfg.price_source("claude-opus-5-5"), Some("yours"));
        assert_eq!(cfg.price_source("my-model"), Some("yours"));
        assert_eq!(cfg.price_source("claude-sonnet-5-5"), Some("default"));
        assert_eq!(cfg.price_source("nope"), None);
    }

    #[test]
    fn the_default_file_names_where_prices_came_from() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        Config::write_default_if_missing(&path).unwrap();
        let text = std::fs::read_to_string(&path).unwrap();
        assert!(text.contains("claude-api"), "{text}");
        assert!(text.contains("2026-10-21"), "{text}");
        assert_eq!(Config::load(&path).unwrap(), Config::default());
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
