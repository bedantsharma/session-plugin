//! Core library for bridge-screen.
//!
//! See `docs/superpowers/specs/2026-10-13-bridge-screen-design.md` §4.2 for the module map.

pub mod binding;
pub mod checkpoint;
pub mod claim;
pub mod config;
pub mod decision;
pub mod drift;
pub mod error;
pub mod forecast;
pub mod git;
pub mod handoff;
pub mod import;
pub mod init;
pub mod navigator;
pub mod owner;
pub mod paths;
pub mod plan;
pub mod projection;
pub mod recovery;
pub mod repo;
pub mod session;
pub mod squash;
pub mod store;
pub mod tangent;
pub mod telemetry;
pub mod tuning;
pub mod verify;
pub mod view;

/// Crate version, shared by the CLI and the plugin manifest.
pub const VERSION: &str = env!("CARGO_PKG_VERSION");

#[cfg(test)]
mod tests {
    use super::VERSION;

    #[test]
    fn version_is_semver() {
        let parts: Vec<&str> = VERSION.split('.').collect();
        assert_eq!(parts.len(), 3);
        assert!(parts.iter().all(|p| p.parse::<u32>().is_ok()));
    }
}
