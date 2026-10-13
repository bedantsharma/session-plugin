//! Core library for bridge-screen.
//!
//! See `docs/superpowers/specs/2026-10-13-bridge-screen-design.md` §4.2 for the module map.

pub mod claim;
pub mod config;
pub mod error;
pub mod paths;
pub mod plan;
pub mod projection;
pub mod repo;
pub mod session;
pub mod store;

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
