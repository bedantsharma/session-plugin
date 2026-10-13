//! Every git operation bridge-screen performs (spec §8). Always the `git` command, run
//! with `-C <worktree>` in a scrubbed environment: no inherited `GIT_DIR`/`GIT_WORK_TREE`
//! (hooks may run inside another git process), no terminal prompts, literal pathspecs,
//! and stdin closed (the MCP server's stdin is its protocol channel).
//!
//! Commit identity comes from the user's git config and the repo's commit hooks run.
//! Failures become [`Error::Git`] with git's own message.

use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use serde::{Deserialize, Serialize};

use crate::error::{Error, Result};

/// Variables that would point git somewhere other than `-C <worktree>`.
const SCRUBBED: &[&str] = &[
    "GIT_DIR",
    "GIT_WORK_TREE",
    "GIT_INDEX_FILE",
    "GIT_COMMON_DIR",
    "GIT_OBJECT_DIRECTORY",
    "GIT_ALTERNATE_OBJECT_DIRECTORIES",
    "GIT_NAMESPACE",
    "GIT_PREFIX",
];

/// One changed path in a worktree.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FileState {
    /// Relative to the worktree root.
    pub path: String,
    /// For a rename or copy: where it came from.
    pub orig_path: Option<String>,
    pub staged: bool,
    pub unstaged: bool,
    pub untracked: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct WorktreeStatus {
    /// `None` when HEAD is detached.
    pub branch: Option<String>,
    /// `None` before the first commit.
    pub head: Option<String>,
    pub files: Vec<FileState>,
}

impl WorktreeStatus {
    /// Every path with uncommitted changes, including the old side of a rename, sorted.
    pub fn dirty_paths(&self) -> Vec<String> {
        let mut out: Vec<String> = self
            .files
            .iter()
            .flat_map(|f| std::iter::once(f.path.clone()).chain(f.orig_path.clone()))
            .collect();
        out.sort();
        out.dedup();
        out
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct StashRef {
    pub sha: String,
    pub message: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CommitInfo {
    pub sha: String,
    pub subject: String,
    pub trailers: Vec<(String, String)>,
}

/// HEAD's commit: what commit detection needs, in one git call.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HeadCommit {
    pub sha: String,
    /// Committer time, Unix seconds.
    pub time: i64,
    pub subject: String,
    pub trailers: Vec<(String, String)>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorktreeInfo {
    pub path: PathBuf,
    /// Short branch name; `None` when detached or bare.
    pub branch: Option<String>,
    pub head: Option<String>,
}

/// The git command for `wt`, environment scrubbed.
fn command(wt: &Path) -> Command {
    let mut cmd = Command::new("git");
    cmd.arg("-C").arg(wt);
    for var in SCRUBBED {
        cmd.env_remove(var);
    }
    cmd.env("GIT_TERMINAL_PROMPT", "0")
        .env("GIT_LITERAL_PATHSPECS", "1")
        .env("GIT_EDITOR", "true")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    cmd
}

/// Run git; its stdout on success, else `Error::Git` (or `NotARepo`).
fn run(wt: &Path, args: &[&str]) -> Result<Vec<u8>> {
    let out = command(wt)
        .args(args)
        .output()
        .map_err(|e| Error::Git(format!("could not run git: {e}")))?;
    if out.status.success() {
        return Ok(out.stdout);
    }
    let mut msg = String::from_utf8_lossy(&out.stderr).trim().to_string();
    if msg.is_empty() {
        msg = String::from_utf8_lossy(&out.stdout).trim().to_string();
    }
    if msg.contains("not a git repository") {
        return Err(Error::NotARepo(wt.display().to_string()));
    }
    Err(Error::Git(format!("git {}: {msg}", args.join(" "))))
}

/// Run git and read stdout as text, trailing newline removed.
fn text(wt: &Path, args: &[&str]) -> Result<String> {
    let out = run(wt, args)?;
    let mut s = String::from_utf8_lossy(&out).into_owned();
    while s.ends_with(['\n', '\r']) {
        s.pop();
    }
    Ok(s)
}

/// Run git where a non-zero exit means "no answer" rather than an error.
fn optional(wt: &Path, args: &[&str]) -> Result<Option<String>> {
    let out = command(wt)
        .args(args)
        .output()
        .map_err(|e| Error::Git(format!("could not run git: {e}")))?;
    if !out.status.success() {
        let err = String::from_utf8_lossy(&out.stderr);
        if err.contains("not a git repository") {
            return Err(Error::NotARepo(wt.display().to_string()));
        }
        return Ok(None);
    }
    let s = String::from_utf8_lossy(&out.stdout).trim().to_string();
    Ok((!s.is_empty()).then_some(s))
}

fn nul_fields(bytes: &[u8]) -> Vec<String> {
    bytes
        .split(|b| *b == 0)
        .filter(|f| !f.is_empty())
        .map(|f| String::from_utf8_lossy(f).into_owned())
        .collect()
}

/// The worktree's root directory.
pub fn toplevel(dir: &Path) -> Result<PathBuf> {
    Ok(PathBuf::from(text(dir, &["rev-parse", "--show-toplevel"])?))
}

/// Branch, HEAD and every changed path (porcelain v2, untracked files listed one by one).
pub fn status(wt: &Path) -> Result<WorktreeStatus> {
    let out = run(
        wt,
        &[
            "status",
            "--porcelain=v2",
            "-z",
            "--branch",
            "--untracked-files=all",
        ],
    )?;
    let mut st = WorktreeStatus::default();
    let mut fields = nul_fields(&out).into_iter();
    while let Some(line) = fields.next() {
        if let Some(h) = line.strip_prefix("# branch.oid ") {
            st.head = (h != "(initial)").then(|| h.to_string());
        } else if let Some(b) = line.strip_prefix("# branch.head ") {
            st.branch = (b != "(detached)").then(|| b.to_string());
        } else if let Some(path) = line.strip_prefix("? ") {
            st.files.push(FileState {
                path: path.to_string(),
                orig_path: None,
                staged: false,
                unstaged: false,
                untracked: true,
            });
        } else {
            let (n, renamed) = match line.as_bytes().first() {
                Some(b'1') => (9, false),
                Some(b'2') => (10, true),
                Some(b'u') => (11, false),
                _ => continue,
            };
            let parts: Vec<&str> = line.splitn(n, ' ').collect();
            let (Some(xy), Some(path)) = (parts.get(1), parts.get(n - 1)) else {
                continue;
            };
            let xy = xy.as_bytes();
            let unmerged = line.starts_with('u');
            st.files.push(FileState {
                path: (*path).to_string(),
                orig_path: if renamed { fields.next() } else { None },
                staged: unmerged || xy.first().is_some_and(|c| *c != b'.'),
                unstaged: unmerged || xy.get(1).is_some_and(|c| *c != b'.'),
                untracked: false,
            });
        }
    }
    Ok(st)
}

/// Any uncommitted change, staged, unstaged or untracked (ignored files don't count).
pub fn is_dirty(wt: &Path) -> Result<bool> {
    let out = run(wt, &["status", "--porcelain=v1", "-z"])?;
    Ok(!out.is_empty())
}

/// The commit HEAD points at, or `None` before the first commit.
pub fn head(wt: &Path) -> Result<Option<String>> {
    optional(wt, &["rev-parse", "--verify", "-q", "HEAD^{commit}"])
}

/// HEAD's commit, or `None` before the first commit.
pub fn head_commit(wt: &Path) -> Result<Option<HeadCommit>> {
    let Some(out) = optional(
        wt,
        &[
            "log",
            "-1",
            "--format=%H%x1f%ct%x1f%s%x1f%(trailers:only,unfold)",
            "HEAD",
            "--",
        ],
    )?
    else {
        return Ok(None);
    };
    let mut f = out.splitn(4, '\x1f');
    let sha = f.next().unwrap_or_default().to_string();
    let time = f.next().and_then(|t| t.parse().ok()).unwrap_or(0);
    let subject = f.next().unwrap_or_default().to_string();
    let trailers = parse_trailers(f.next().unwrap_or_default());
    Ok(Some(HeadCommit {
        sha,
        time,
        subject,
        trailers,
    }))
}

/// The full message of `sha`.
pub fn commit_message(wt: &Path, sha: &str) -> Result<String> {
    let out = run(wt, &["log", "-1", "--format=%B", sha, "--"])?;
    Ok(String::from_utf8_lossy(&out).into_owned())
}

fn parse_trailers(text: &str) -> Vec<(String, String)> {
    text.lines()
        .filter_map(|l| l.split_once(':'))
        .map(|(k, v)| (k.trim().to_string(), v.trim().to_string()))
        .filter(|(k, _)| !k.is_empty())
        .collect()
}

/// The trailers (`Key: value`) of commit `sha`, in order.
pub fn trailers(wt: &Path, sha: &str) -> Result<Vec<(String, String)>> {
    let out = text(
        wt,
        &["log", "-1", "--format=%(trailers:only,unfold)", sha, "--"],
    )?;
    Ok(parse_trailers(&out))
}

/// Paths that differ from `sha`: committed since, staged, unstaged and untracked. Sorted.
pub fn diff_names_since(wt: &Path, sha: &str) -> Result<Vec<String>> {
    let mut names = nul_fields(&run(wt, &["diff", "--name-only", "-z", sha, "--"])?);
    names.extend(nul_fields(&run(
        wt,
        &["ls-files", "-z", "--others", "--exclude-standard"],
    )?));
    names.sort();
    names.dedup();
    Ok(names)
}

/// `-m subject` plus, if any, a paragraph of trailers.
fn message_args(subject: &str, trailers: &[(String, String)]) -> Vec<String> {
    let mut args = vec!["-m".to_string(), subject.to_string()];
    if !trailers.is_empty() {
        let block: Vec<String> = trailers.iter().map(|(k, v)| format!("{k}: {v}")).collect();
        args.push("-m".into());
        args.push(block.join("\n"));
    }
    args
}

fn commit(wt: &Path, mut args: Vec<String>) -> Result<String> {
    args.insert(0, "commit".into());
    args.insert(1, "-q".into());
    let refs: Vec<&str> = args.iter().map(String::as_str).collect();
    run(wt, &refs)?;
    head(wt)?.ok_or_else(|| Error::Git("commit made no HEAD".into()))
}

/// Stage everything and commit it. The repo's commit hooks run. Returns the new HEAD.
pub fn commit_all(wt: &Path, subject: &str, trailers: &[(String, String)]) -> Result<String> {
    run(wt, &["add", "-A"])?;
    commit(wt, message_args(subject, trailers))
}

/// Commit exactly `paths` (new, changed or deleted), leaving anything else staged or not
/// as it was. Returns the new HEAD.
pub fn commit_paths(
    wt: &Path,
    paths: &[String],
    subject: &str,
    trailers: &[(String, String)],
) -> Result<String> {
    if paths.is_empty() {
        return Err(Error::Git("no paths to commit".into()));
    }
    let mut add = vec!["add", "-A", "--"];
    add.extend(paths.iter().map(String::as_str));
    run(wt, &add)?;
    let mut args = message_args(subject, trailers);
    args.push("--only".into());
    args.push("--".into());
    args.extend(paths.iter().cloned());
    commit(wt, args)
}

fn stash_top(wt: &Path) -> Result<Option<String>> {
    optional(wt, &["rev-parse", "--verify", "-q", "refs/stash"])
}

/// Stash the changes to `paths` only, untracked files included; they leave the worktree.
/// Nothing to stash is an error, so a caller never records a stash that isn't there.
pub fn stash_paths(wt: &Path, paths: &[String], message: &str) -> Result<StashRef> {
    if paths.is_empty() {
        return Err(Error::Git("no paths to stash".into()));
    }
    let before = stash_top(wt)?;
    let mut args = vec!["stash", "push", "--include-untracked", "-m", message, "--"];
    args.extend(paths.iter().map(String::as_str));
    run(wt, &args)?;
    let sha = stash_top(wt)?
        .filter(|s| Some(s) != before.as_ref())
        .ok_or_else(|| Error::Git(format!("nothing to stash in {}", paths.join(", "))))?;
    let message = text(wt, &["log", "-1", "--format=%s", &sha, "--"])?;
    Ok(StashRef { sha, message })
}

/// The merge base of HEAD and `base`, or `None` if there is none (or `base` is unknown).
pub fn merge_base(wt: &Path, base: &str) -> Result<Option<String>> {
    optional(wt, &["merge-base", "HEAD", base])
}

/// What a branch is usually merged into: `origin/HEAD`'s target, else `main`, else `master`.
pub fn default_base(wt: &Path) -> Result<String> {
    if let Some(r) = optional(wt, &["symbolic-ref", "-q", "refs/remotes/origin/HEAD"])? {
        return Ok(r.strip_prefix("refs/remotes/").unwrap_or(&r).to_string());
    }
    if optional(wt, &["rev-parse", "--verify", "-q", "refs/heads/main"])?.is_some() {
        return Ok("main".into());
    }
    Ok("master".into())
}

/// Commits from `base_sha` (exclusive) to HEAD, oldest first.
pub fn commits_since(wt: &Path, base_sha: &str) -> Result<Vec<CommitInfo>> {
    let range = format!("{base_sha}..HEAD");
    let out = text(
        wt,
        &[
            "log",
            "--reverse",
            "--format=%H%x1f%s%x1f%(trailers:only,unfold)%x1e",
            &range,
            "--",
        ],
    )?;
    Ok(out
        .split('\x1e')
        .filter_map(|rec| {
            let rec = rec.trim_start_matches('\n');
            let mut f = rec.splitn(3, '\x1f');
            let sha = f.next()?.trim();
            if sha.is_empty() {
                return None;
            }
            Some(CommitInfo {
                sha: sha.to_string(),
                subject: f.next().unwrap_or_default().to_string(),
                trailers: parse_trailers(f.next().unwrap_or_default()),
            })
        })
        .collect())
}

/// Point ref `name` at `sha` (creating it).
pub fn create_ref(wt: &Path, name: &str, sha: &str) -> Result<()> {
    run(wt, &["update-ref", name, sha])?;
    Ok(())
}

/// Move the branch to `sha`, keeping all changes staged.
pub fn soft_reset(wt: &Path, sha: &str) -> Result<()> {
    run(wt, &["reset", "-q", "--soft", sha])?;
    Ok(())
}

/// The current branch's short name, `None` when detached.
pub fn current_branch(wt: &Path) -> Result<Option<String>> {
    optional(wt, &["symbolic-ref", "-q", "--short", "HEAD"])
}

/// The current branch's upstream (`origin/feat`), if it has one.
pub fn upstream(wt: &Path) -> Result<Option<String>> {
    optional(
        wt,
        &[
            "rev-parse",
            "--abbrev-ref",
            "--symbolic-full-name",
            "@{upstream}",
        ],
    )
}

/// The commit `rev` names, if any.
pub fn rev_parse(wt: &Path, rev: &str) -> Result<Option<String>> {
    let spec = format!("{rev}^{{commit}}");
    optional(wt, &["rev-parse", "--verify", "-q", &spec])
}

/// Push HEAD to the current branch's upstream branch, overwriting it only if it still is
/// `expected_remote_sha` there.
pub fn push_force_with_lease(wt: &Path, expected_remote_sha: &str) -> Result<()> {
    let branch = current_branch(wt)?.ok_or_else(|| Error::Git("HEAD is detached".into()))?;
    let remote = optional(wt, &["config", &format!("branch.{branch}.remote")])?
        .ok_or_else(|| Error::Git(format!("{branch} has no upstream remote")))?;
    let merge = optional(wt, &["config", &format!("branch.{branch}.merge")])?
        .ok_or_else(|| Error::Git(format!("{branch} has no upstream branch")))?;
    let lease = format!("--force-with-lease={merge}:{expected_remote_sha}");
    let refspec = format!("HEAD:{merge}");
    run(wt, &["push", "-q", &lease, &remote, &refspec])?;
    Ok(())
}

/// Every worktree of the repo, the main one first.
pub fn worktree_list(wt: &Path) -> Result<Vec<WorktreeInfo>> {
    let out = run(wt, &["worktree", "list", "--porcelain", "-z"])?;
    let mut list = Vec::new();
    let mut current: Option<WorktreeInfo> = None;
    // Records are attribute fields separated by NUL, ended by an empty field.
    for field in out.split(|b| *b == 0) {
        let field = String::from_utf8_lossy(field);
        if field.is_empty() {
            list.extend(current.take());
            continue;
        }
        if let Some(path) = field.strip_prefix("worktree ") {
            list.extend(current.take());
            current = Some(WorktreeInfo {
                path: PathBuf::from(path),
                branch: None,
                head: None,
            });
        } else if let Some(info) = current.as_mut() {
            if let Some(h) = field.strip_prefix("HEAD ") {
                info.head = Some(h.to_string());
            } else if let Some(b) = field.strip_prefix("branch ") {
                info.branch = Some(b.strip_prefix("refs/heads/").unwrap_or(b).to_string());
            }
        }
    }
    list.extend(current);
    Ok(list)
}

#[cfg(test)]
pub(crate) mod tests;
