//! Where bridge-screen keeps its data (spec §6.1). Never inside a repo.

use std::path::{Path, PathBuf};

use crate::error::{Error, Result};

/// Environment variable that overrides the data folder (used by tests).
pub const HOME_ENV: &str = "BRIDGE_SCREEN_HOME";

/// The bridge-screen data folder and the files inside it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Home {
    root: PathBuf,
}

impl Home {
    /// `$BRIDGE_SCREEN_HOME` if set and non-empty, otherwise `~/.bridge-screen`.
    pub fn resolve() -> Result<Self> {
        if let Some(value) = std::env::var_os(HOME_ENV).filter(|v| !v.is_empty()) {
            return Ok(Self::at(PathBuf::from(value)));
        }
        let home = std::env::home_dir().ok_or(Error::NoHome)?;
        Ok(Self::at(home.join(".bridge-screen")))
    }

    pub fn at(root: impl Into<PathBuf>) -> Self {
        Self { root: root.into() }
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    pub fn db_path(&self) -> PathBuf {
        self.root.join("bridge-screen.db")
    }

    pub fn config_path(&self) -> PathBuf {
        self.root.join("config.toml")
    }

    /// Where undo snapshot records for a repo's leftovers go (spec §8.3).
    pub fn snapshots_dir(&self, repo_id: &str) -> PathBuf {
        self.root.join("snapshots").join(repo_id)
    }

    /// Create the data folder if it doesn't exist.
    pub fn ensure(&self) -> Result<()> {
        std::fs::create_dir_all(&self.root)?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::Home;
    use std::path::Path;

    #[test]
    fn layout_is_under_root() {
        let home = Home::at("/tmp/bs-home");
        assert_eq!(home.root(), Path::new("/tmp/bs-home"));
        assert_eq!(home.db_path(), Path::new("/tmp/bs-home/bridge-screen.db"));
        assert_eq!(home.config_path(), Path::new("/tmp/bs-home/config.toml"));
    }

    #[test]
    fn ensure_creates_the_folder() {
        let dir = tempfile::tempdir().unwrap();
        let home = Home::at(dir.path().join("nested/home"));
        home.ensure().unwrap();
        assert!(home.root().is_dir());
    }
}
