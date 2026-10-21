//! Which repo a directory belongs to. All worktrees of a repo share one id (spec §6.1).

use std::fmt::Write as _;
use std::path::{Path, PathBuf};

use sha2::{Digest, Sha256};

use crate::error::{Error, Result};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RepoInfo {
    /// 16 hex chars derived from git's shared directory.
    pub id: String,
    /// Folder name of the main checkout.
    pub name: String,
    /// Path of the main checkout.
    pub root: PathBuf,
}

/// Identify the repo that contains `dir`.
pub fn identify(dir: &Path) -> Result<RepoInfo> {
    let common = git_path(
        dir,
        &["rev-parse", "--path-format=absolute", "--git-common-dir"],
    )?;
    let toplevel = git_path(dir, &["rev-parse", "--show-toplevel"])?;
    let common = std::fs::canonicalize(&common)?;
    let root = if common.file_name().is_some_and(|n| n == ".git") {
        common
            .parent()
            .map_or_else(|| toplevel.clone(), Path::to_path_buf)
    } else {
        toplevel
    };
    let name = root
        .file_name()
        .map_or_else(|| "repo".to_string(), |n| n.to_string_lossy().into_owned());
    Ok(RepoInfo {
        id: repo_id_for(&common),
        name,
        root,
    })
}

/// The checked-out branch of the worktree containing `dir` (`HEAD` when detached), or
/// `None` if git can't tell.
pub fn current_branch(dir: &Path) -> Option<String> {
    let out = git(dir, &["rev-parse", "--abbrev-ref", "HEAD"]).ok()?;
    let name = String::from_utf8(out).ok()?.trim().to_string();
    (!name.is_empty()).then_some(name)
}

/// Most directories [`read_head`] climbs before giving up.
const MAX_CLIMB: usize = 64;

/// The checked-out branch of the worktree containing `dir`, from file reads only (no git
/// process, so a hook can afford it): `.git/HEAD`, or for a linked worktree the `HEAD`
/// in the directory its `.git` file points at. A detached HEAD reads
/// `detached at <first 7 of the SHA>`. The climb stops at `stop_at` (the session's
/// worktree root, when known). `None` when nothing readable is found.
pub fn read_head(dir: &Path, stop_at: Option<&Path>) -> Option<String> {
    let mut here = Some(dir);
    for _ in 0..MAX_CLIMB {
        let d = here?;
        let dot_git = d.join(".git");
        let head = if dot_git.is_dir() {
            Some(dot_git.join("HEAD"))
        } else if dot_git.is_file() {
            let text = std::fs::read_to_string(&dot_git).ok()?;
            let target = text.lines().find_map(|l| l.strip_prefix("gitdir:"))?.trim();
            let target = Path::new(target);
            let gitdir = if target.is_absolute() {
                target.to_path_buf()
            } else {
                d.join(target)
            };
            Some(gitdir.join("HEAD"))
        } else {
            None
        };
        if let Some(head) = head {
            let text = std::fs::read_to_string(head).ok()?;
            let text = text.trim();
            return if let Some(name) = text.strip_prefix("ref:") {
                let name = name.trim();
                Some(name.strip_prefix("refs/heads/").unwrap_or(name).to_string())
            } else if text.len() >= 7 && text.chars().all(|c| c.is_ascii_hexdigit()) {
                Some(format!("detached at {}", &text[..7]))
            } else {
                None
            };
        }
        if stop_at.is_some_and(|s| s == d) {
            return None;
        }
        here = d.parent();
    }
    None
}

/// First 16 hex chars of SHA-256 over the git common directory path.
///
/// Hashes the path's raw bytes, so paths that aren't valid UTF-8 still get distinct ids
/// (for UTF-8 paths this is the same as hashing the string).
pub fn repo_id_for(common_dir: &Path) -> String {
    let digest = Sha256::digest(common_dir.as_os_str().as_encoded_bytes());
    digest
        .iter()
        .take(8)
        .fold(String::with_capacity(16), |mut out, b| {
            let _ = write!(out, "{b:02x}");
            out
        })
}

/// Run git and read its output as a path, keeping bytes that aren't valid UTF-8.
fn git_path(dir: &Path, args: &[&str]) -> Result<PathBuf> {
    let mut out = git(dir, args)?;
    // Strip only the line ending: a path may legitimately end in whitespace.
    while out.last().is_some_and(|b| matches!(b, b'\n' | b'\r')) {
        out.pop();
    }
    #[cfg(unix)]
    let path = {
        use std::os::unix::ffi::OsStringExt as _;
        PathBuf::from(std::ffi::OsString::from_vec(out))
    };
    #[cfg(not(unix))]
    let path = PathBuf::from(
        String::from_utf8(out)
            .map_err(|e| Error::Git(format!("git printed a path that isn't UTF-8: {e}")))?,
    );
    Ok(path)
}

/// Git with the same scrubbed environment as every other git call (`crate::git`): an
/// inherited `GIT_DIR` would otherwise identify some other repo.
fn git(dir: &Path, args: &[&str]) -> Result<Vec<u8>> {
    let out = crate::git::command(dir)
        .args(args)
        .output()
        .map_err(|e| Error::Git(format!("could not run git: {e}")))?;
    if !out.status.success() {
        let msg = String::from_utf8_lossy(&out.stderr).trim().to_string();
        if msg.contains("not a git repository") {
            return Err(Error::NotARepo(dir.display().to_string()));
        }
        return Err(Error::Git(msg));
    }
    Ok(out.stdout)
}

#[cfg(test)]
mod tests {
    use super::{identify, repo_id_for};
    use crate::error::Error;
    use std::path::Path;
    use std::process::Command;

    fn git(dir: &Path, args: &[&str]) {
        let mut cmd = Command::new("git");
        for var in crate::git::SCRUBBED {
            cmd.env_remove(var);
        }
        let status = cmd
            .arg("-C")
            .arg(dir)
            .args([
                "-c",
                "user.name=t",
                "-c",
                "user.email=t@t",
                "-c",
                "init.defaultBranch=main",
            ])
            .args(args)
            .status()
            .unwrap();
        assert!(status.success(), "git {args:?} failed");
    }

    fn new_repo() -> tempfile::TempDir {
        let dir = tempfile::tempdir().unwrap();
        git(dir.path(), &["init", "-q"]);
        git(dir.path(), &["commit", "-q", "--allow-empty", "-m", "init"]);
        dir
    }

    #[test]
    fn id_is_16_hex_chars_and_stable() {
        let a = repo_id_for(Path::new("/x/.git"));
        assert_eq!(a.len(), 16);
        assert!(
            a.chars()
                .all(|c| c.is_ascii_hexdigit() && !c.is_ascii_uppercase())
        );
        assert_eq!(a, repo_id_for(Path::new("/x/.git")));
        assert_ne!(a, repo_id_for(Path::new("/y/.git")));
    }

    #[test]
    fn current_branch_reads_head() {
        let repo = new_repo();
        assert_eq!(super::current_branch(repo.path()).as_deref(), Some("main"));
        let plain = tempfile::tempdir().unwrap();
        assert_eq!(super::current_branch(plain.path()), None);
    }

    #[test]
    fn read_head_without_git() {
        let repo = new_repo();
        std::fs::create_dir(repo.path().join("sub")).unwrap();
        assert_eq!(
            super::read_head(&repo.path().join("sub"), None).as_deref(),
            Some("main")
        );
        // Stops at the given root rather than climbing out of it.
        let plain = tempfile::tempdir().unwrap();
        let inner = plain.path().join("a");
        std::fs::create_dir(&inner).unwrap();
        assert_eq!(super::read_head(&inner, Some(plain.path())), None);
        // Detached.
        let sha = std::fs::read_to_string(repo.path().join(".git/refs/heads/main")).unwrap();
        git(repo.path(), &["checkout", "-q", "--detach"]);
        assert_eq!(
            super::read_head(repo.path(), None),
            Some(format!("detached at {}", &sha.trim()[..7]))
        );
    }

    #[test]
    fn subdirectory_resolves_to_same_repo() {
        let repo = new_repo();
        std::fs::create_dir(repo.path().join("sub")).unwrap();
        let top = identify(repo.path()).unwrap();
        let sub = identify(&repo.path().join("sub")).unwrap();
        assert_eq!(top, sub);
        assert_eq!(top.name, repo.path().file_name().unwrap().to_string_lossy());
    }

    #[test]
    fn linked_worktree_has_same_repo_id() {
        let repo = new_repo();
        let wt_parent = tempfile::tempdir().unwrap();
        let wt = wt_parent.path().join("wt");
        git(
            repo.path(),
            &[
                "worktree",
                "add",
                "-q",
                "-b",
                "feature",
                wt.to_str().unwrap(),
            ],
        );
        let main = identify(repo.path()).unwrap();
        let linked = identify(&wt).unwrap();
        assert_eq!(main.id, linked.id);
        assert_eq!(main.name, linked.name);
        assert_eq!(main.root, linked.root);
    }

    #[test]
    fn folder_name_ending_in_a_space_is_kept() {
        let base = tempfile::tempdir().unwrap();
        let dir = base.path().join("my repo ");
        std::fs::create_dir(&dir).unwrap();
        git(&dir, &["init", "-q"]);
        let info = identify(&dir).unwrap();
        assert_eq!(info.name, "my repo ");
        assert_eq!(info.root, std::fs::canonicalize(&dir).unwrap());
    }

    #[cfg(unix)]
    #[test]
    fn non_utf8_folder_name_is_identified() {
        use std::os::unix::ffi::OsStrExt as _;
        let base = tempfile::tempdir().unwrap();
        let dir = base.path().join(std::ffi::OsStr::from_bytes(b"caf\xe9"));
        // Some filesystems (APFS) refuse names that aren't UTF-8; nothing to test there.
        if std::fs::create_dir(&dir).is_err() {
            return;
        }
        git(&dir, &["init", "-q"]);
        let info = identify(&dir).unwrap();
        assert_eq!(info.root, std::fs::canonicalize(&dir).unwrap());
        assert_eq!(info.id, repo_id_for(&info.root.join(".git")));
    }

    #[test]
    fn outside_git_is_not_a_repo() {
        let dir = tempfile::tempdir().unwrap();
        assert!(matches!(identify(dir.path()), Err(Error::NotARepo(_))));
    }
}
