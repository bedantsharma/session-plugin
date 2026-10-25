//! Finding plan and spec files anywhere in a checkout (spec §12.1). The walk is bounded
//! and only looks at names; the parser stays the gate that decides whether a matched file
//! really is a plan, so a false match costs one read and never a wrong import.

use std::collections::VecDeque;
use std::path::{Path, PathBuf};

use crate::error::Result;

/// The preferred plan folder (superpowers layout), listed first when it exists.
pub const PLANS_DIR: &str = "docs/superpowers/plans";

/// The preferred spec folder.
pub const SPECS_DIR: &str = "docs/superpowers/specs";

/// Deepest folder level walked (the checkout root is level 0).
pub const MAX_DEPTH: usize = 8;

/// Most directory entries looked at in one walk.
pub const MAX_ENTRIES: usize = 3000;

/// Folders never entered (dot-folders are skipped as well).
const SKIPPED_DIRS: [&str; 5] = ["node_modules", "target", "dist", "build", "vendor"];

/// Words that make a file or folder name look like a plan.
const PLAN_WORDS: [&str; 4] = ["plan", "roadmap", "todo", "tasks"];

fn skipped(name: &str) -> bool {
    name.starts_with('.') || name == "venv" || SKIPPED_DIRS.contains(&name)
}

fn contains_word(name: &str, words: &[&str]) -> bool {
    let lower = name.to_ascii_lowercase();
    words.iter().any(|w| lower.contains(w))
}

/// Does any folder or the file name of `rel` (a `/`-separated relative path) look like a
/// plan?
fn looks_like_plan(rel: &str) -> bool {
    contains_word(rel, &PLAN_WORDS)
}

/// A folder named `spec`/`specs`, or a file name with `spec` or `design` as a word.
fn looks_like_spec(rel: &str) -> bool {
    let lower = rel.to_ascii_lowercase();
    let mut parts = lower.split('/').peekable();
    while let Some(part) = parts.next() {
        let last = parts.peek().is_none();
        let part = if last {
            part.strip_suffix(".md").unwrap_or(part)
        } else {
            part
        };
        if matches!(part, "spec" | "specs") && !last {
            return true;
        }
        if last
            && part
                .split(|c: char| !c.is_ascii_alphanumeric())
                .any(|w| matches!(w, "spec" | "specs" | "design"))
        {
            return true;
        }
    }
    false
}

/// Every `*.md` under `root` whose relative path passes `is_match`. The `preferred` folder
/// comes first (by name), then the rest by shortest path.
fn find(root: &Path, preferred: &str, is_match: fn(&str) -> bool) -> Result<Vec<PathBuf>> {
    let mut first: Vec<PathBuf> = Vec::new();
    match std::fs::read_dir(root.join(preferred)) {
        Ok(entries) => {
            first = entries
                .filter_map(std::result::Result::ok)
                .filter(|e| e.file_type().is_ok_and(|t| t.is_file()))
                .map(|e| e.path())
                .filter(|p| p.extension().is_some_and(|x| x == "md"))
                .collect();
            first.sort();
        }
        // A missing folder has no files; an unreadable one is an error worth logging.
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
        Err(e) => return Err(e.into()),
    }
    let preferred_dir = root.join(preferred);
    let mut rest: Vec<(usize, String, PathBuf)> = Vec::new();
    let mut seen = 0usize;
    let mut queue: VecDeque<(PathBuf, String, usize)> = VecDeque::new();
    queue.push_back((root.to_path_buf(), String::new(), 0));
    'walk: while let Some((dir, rel_dir, depth)) = queue.pop_front() {
        let Ok(entries) = std::fs::read_dir(&dir) else {
            continue;
        };
        let mut entries: Vec<_> = entries.filter_map(std::result::Result::ok).collect();
        entries.sort_by_key(std::fs::DirEntry::file_name);
        for e in entries {
            seen += 1;
            if seen > MAX_ENTRIES {
                break 'walk;
            }
            let Ok(ft) = e.file_type() else { continue };
            let name = e.file_name().to_string_lossy().into_owned();
            let rel = if rel_dir.is_empty() {
                name.clone()
            } else {
                format!("{rel_dir}/{name}")
            };
            if ft.is_dir() {
                if !skipped(&name) && depth < MAX_DEPTH && e.path() != preferred_dir {
                    queue.push_back((e.path(), rel, depth + 1));
                }
            } else if ft.is_file()
                && Path::new(&name)
                    .extension()
                    .is_some_and(|x| x.eq_ignore_ascii_case("md"))
                && is_match(&rel)
            {
                if e.path().parent() == Some(preferred_dir.as_path()) {
                    continue;
                }
                rest.push((rel.split('/').count(), rel, e.path()));
            }
        }
    }
    rest.sort();
    first.extend(rest.into_iter().map(|(_, _, p)| p));
    Ok(first)
}

/// The plan files under `root`: `docs/superpowers/plans` first, then every other `*.md`
/// whose folder or file name says plan, roadmap, todo or tasks, shortest path first. The
/// walk stops at [`MAX_DEPTH`] levels and [`MAX_ENTRIES`] entries.
pub fn plan_files(root: &Path) -> Result<Vec<PathBuf>> {
    find(root, PLANS_DIR, looks_like_plan)
}

/// The spec files under `root`, found the same way (listed as references).
pub fn spec_files(root: &Path) -> Result<Vec<PathBuf>> {
    find(root, SPECS_DIR, looks_like_spec)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn touch(root: &Path, rel: &str) {
        let p = root.join(rel);
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(p, "# x\n").unwrap();
    }

    fn rels(root: &Path, files: &[PathBuf]) -> Vec<String> {
        files
            .iter()
            .map(|p| p.strip_prefix(root).unwrap().to_string_lossy().into_owned())
            .collect()
    }

    #[test]
    fn finds_plans_in_any_folder() {
        let d = tempfile::tempdir().unwrap();
        for f in [
            "planning/a.md",
            "notes/plans/b.md",
            "docs/design/plan-x.md",
            "a/b/c/plans/deep.md",
            "ROADMAP.md",
            "README.md",
            "docs/guide.md",
        ] {
            touch(d.path(), f);
        }
        let got = rels(d.path(), &plan_files(d.path()).unwrap());
        assert_eq!(
            got,
            vec![
                "ROADMAP.md",
                "planning/a.md",
                "docs/design/plan-x.md",
                "notes/plans/b.md",
                "a/b/c/plans/deep.md"
            ]
        );
    }

    #[test]
    fn skips_vendored_and_hidden_folders() {
        let d = tempfile::tempdir().unwrap();
        for f in [
            "node_modules/x/plan.md",
            "target/plan.md",
            ".git/plan.md",
            ".hidden/plans/p.md",
            "vendor/plan.md",
            "plan.md",
        ] {
            touch(d.path(), f);
        }
        assert_eq!(
            rels(d.path(), &plan_files(d.path()).unwrap()),
            vec!["plan.md"]
        );
    }

    #[test]
    fn readme_text_is_not_a_name_match() {
        let d = tempfile::tempdir().unwrap();
        std::fs::write(d.path().join("README.md"), "# Plan\n\nmy plan is a plan\n").unwrap();
        assert_eq!(
            rels(d.path(), &plan_files(d.path()).unwrap()),
            Vec::<String>::new()
        );
    }

    #[test]
    fn old_layout_comes_first_in_name_order() {
        let d = tempfile::tempdir().unwrap();
        for f in [
            "plan.md",
            "docs/superpowers/plans/b.md",
            "docs/superpowers/plans/a.md",
        ] {
            touch(d.path(), f);
        }
        assert_eq!(
            rels(d.path(), &plan_files(d.path()).unwrap()),
            vec![
                "docs/superpowers/plans/a.md",
                "docs/superpowers/plans/b.md",
                "plan.md"
            ]
        );
    }

    #[test]
    fn walk_stops_at_the_bounds() {
        let d = tempfile::tempdir().unwrap();
        let dir = d.path().join("plans-many");
        std::fs::create_dir_all(&dir).unwrap();
        for i in 0..5000 {
            std::fs::write(dir.join(format!("p{i}.md")), "x").unwrap();
        }
        let n = plan_files(d.path()).unwrap().len();
        assert!(n > 0 && n <= MAX_ENTRIES, "{n}");
    }

    #[test]
    fn walk_stops_at_the_depth_bound() {
        let d = tempfile::tempdir().unwrap();
        touch(d.path(), "a/b/c/d/e/f/g/h/i/plan.md");
        touch(d.path(), "a/b/c/d/e/f/g/h/plan.md");
        let all = rels(d.path(), &plan_files(d.path()).unwrap());
        assert!(all.contains(&"a/b/c/d/e/f/g/h/plan.md".to_string()));
        assert!(!all.contains(&"a/b/c/d/e/f/g/h/i/plan.md".to_string()));
    }

    #[test]
    fn finds_specs_in_any_folder() {
        let d = tempfile::tempdir().unwrap();
        for f in [
            "docs/superpowers/specs/s1.md",
            "specs/a.md",
            "docs/x-design.md",
            "docs/other.md",
        ] {
            touch(d.path(), f);
        }
        let got = rels(d.path(), &spec_files(d.path()).unwrap());
        assert_eq!(
            got,
            vec![
                "docs/superpowers/specs/s1.md",
                "docs/x-design.md",
                "specs/a.md"
            ]
        );
    }
}
