#![allow(clippy::many_single_char_names)] // short names for repos and files in tests

use std::fmt::Write as _;

use super::parse::{MAX_TEXT_CHARS, parse};

const SUPERPOWERS: &str = r"# Login Flow Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: use superpowers:executing-plans.

**Goal:** Users can sign in with email
and a password.

**Architecture:** A form and an endpoint.

---

### Task 1: Login endpoint

**Files:**
- Create: `src/api/login.rs`
- Modify: `src/api/mod.rs:12-40`
- Test: `tests/login.rs`

- [x] **Step 1: Write the failing test**

```rust
- [ ] not a step
```

- [ ] **Step 2: Run it to see it fail**
- [ ] **Step 3: Implement**

### Task 2: Login form

**Files:**
- Create: `web/Login.svelte`

- [ ] **Step 1: Render the form**
";

#[test]
fn parses_superpowers_template() {
    let p = parse(SUPERPOWERS);
    assert_eq!(p.title.as_deref(), Some("Login Flow"));
    assert_eq!(
        p.goal.as_deref(),
        Some("Users can sign in with email and a password.")
    );
    assert_eq!(p.tasks.len(), 2);
    let t1 = &p.tasks[0];
    assert_eq!((t1.number, t1.title.as_str()), (1, "Login endpoint"));
    assert_eq!(
        t1.files,
        vec!["src/api/login.rs", "src/api/mod.rs", "tests/login.rs"]
    );
    let steps: Vec<(u32, &str, bool)> = t1
        .steps
        .iter()
        .map(|s| (s.number, s.text.as_str(), s.checked))
        .collect();
    assert_eq!(
        steps,
        vec![
            (1, "Write the failing test", true),
            (2, "Run it to see it fail", false),
            (3, "Implement", false),
        ]
    );
    assert_eq!(p.tasks[1].files, vec!["web/Login.svelte"]);
    assert_eq!(p.tasks[1].steps.len(), 1);
    assert_eq!(p.warnings, Vec::<String>::new());
}

#[test]
fn parses_this_repos_plan_style() {
    let text = "# Sub-project 7: Polish — Implementation Plan\n\n\
                **Goal:** Fewer false alarms.\n\n\
                ### Task 1: Words\n\n\
                - [ ] Failing tests:\n  - `a_test`: something\n  - [ ] nested is not a step\n\
                - [x] Implement; commit `core: words`.\n\n\
                ### Task 2 — Screens\n\n- [ ] Build it\n";
    let p = parse(text);
    assert_eq!(p.title.as_deref(), Some("Sub-project 7: Polish"));
    assert_eq!(p.goal.as_deref(), Some("Fewer false alarms."));
    assert_eq!(p.tasks.len(), 2);
    assert_eq!(p.tasks[0].files, Vec::<String>::new());
    let steps: Vec<(u32, &str, bool)> = p.tasks[0]
        .steps
        .iter()
        .map(|s| (s.number, s.text.as_str(), s.checked))
        .collect();
    assert_eq!(
        steps,
        vec![
            (1, "Failing tests:", false),
            (2, "Implement; commit `core: words`.", true)
        ]
    );
    assert_eq!(p.tasks[1].title, "Screens");
}

#[test]
fn fenced_checkboxes_are_not_steps() {
    let text = "### Task 1: A\n\n```\n- [ ] fake\n### Task 2: fake\n```\n~~~\n- [x] also fake\n~~~\n- [ ] real\n";
    let p = parse(text);
    assert_eq!(p.tasks.len(), 1);
    assert_eq!(p.tasks[0].steps.len(), 1);
    assert_eq!(p.tasks[0].steps[0].text, "real");
}

#[test]
fn odd_plan_files_are_skipped() {
    let empty = parse("");
    assert_eq!(empty.tasks.len(), 0);
    assert_ne!(empty.warnings, Vec::<String>::new());

    let none = parse("# Notes\n\nJust some text.\n- [ ] a stray box\n");
    assert_eq!(none.tasks.len(), 0);
    assert_ne!(none.warnings, Vec::<String>::new());

    let dup = parse(
        "### Task 1: First\n- [ ] a\n### Task 1: Again\n- [ ] b\n### Task 0: zero\n### Task 2:\n",
    );
    assert_eq!(dup.tasks.len(), 1);
    assert_eq!(dup.tasks[0].title, "First");
    assert_eq!(dup.tasks[0].steps.len(), 1);
    assert_eq!(dup.warnings.len(), 3, "{:?}", dup.warnings);

    let long_step = format!("### Task 1: T\n- [ ] {}\n", "x".repeat(1000));
    assert_eq!(
        parse(&long_step).tasks[0].steps[0].text.chars().count(),
        MAX_TEXT_CHARS
    );

    let mut big = String::with_capacity(5 << 20);
    let mut n = 0;
    while big.len() < 5 << 20 {
        n += 1;
        let _ = write!(
            big,
            "### Task {n}: Task number {n}\n- [ ] step one\n- [x] step two\n\n"
        );
    }
    let p = parse(&big);
    // Capped at MAX_NODES tasks and steps (3 per task here), with a warning.
    assert_eq!(p.tasks.len(), super::parse::MAX_NODES / 3 + 1);
    assert!(n > p.tasks.len());
    assert!(p.warnings.iter().any(|w| w.contains("too many")));

    // Lossy text from a binary file never panics.
    let binary = String::from_utf8_lossy(&[0xff, 0xfe, 0, 1, 2, b'#', b' ', 0xc3]).to_string();
    assert_eq!(parse(&binary).tasks.len(), 0);
}

#[test]
fn huge_checkbox_lists_are_capped() {
    // A 1 MiB file of checkboxes would otherwise become ~100,000 plan nodes.
    let mut text = String::from("# Big Implementation Plan\n\n### Task 1: Many\n\n");
    let mut n = 0;
    while text.len() < 1 << 20 {
        let _ = writeln!(text, "- [ ] step {n}");
        n += 1;
    }
    let p = parse(&text);
    assert_eq!(p.tasks.len(), 1);
    assert_eq!(p.tasks[0].steps.len(), super::parse::MAX_STEPS_PER_TASK);
    assert!(
        p.warnings.iter().any(|w| w.contains("steps")),
        "{:?}",
        p.warnings
    );
    let mut many = String::new();
    for i in 1..=5000 {
        let _ = write!(many, "### Task {i}: t{i}\n- [ ] a\n");
    }
    let p = parse(&many);
    let nodes: usize = p.tasks.iter().map(|t| 1 + t.steps.len()).sum();
    assert!(nodes <= super::parse::MAX_NODES, "{nodes}");
    assert!(p.tasks.len() >= 100);
    assert!(
        p.warnings.iter().any(|w| w.contains("too many")),
        "{:?}",
        p.warnings
    );
    // A repeated step number can't overflow.
    let dup = "### Task 1: A\n- [ ] **Step 4294967295: a**\n- [ ] **Step 4294967295: b**\n";
    assert_eq!(parse(dup).tasks[0].steps.len(), 2);
}

// ---- import and diff -------------------------------------------------------------------

use std::path::{Path, PathBuf};

use super::{ImportCtx, ImportOutcome, ScanBudget, import_path, scan};
use crate::error::Error;
use crate::plan::{
    EditContext, NodeKind, NodeSource, NodeStatus, PlanOp, get_node, load_tree, plan_edit,
};
use crate::store::{Actor, EventDraft, Store, read_all};

struct Repo {
    _dir: tempfile::TempDir,
    root: PathBuf,
    store: Store,
}

impl Repo {
    /// A repo whose owner already answered "which plans are finished" (a fresh run), so a
    /// scan imports at once.
    fn new() -> Self {
        let mut r = Self::undecided();
        r.store
            .write(|tx| {
                crate::store::append_in(
                    tx,
                    EventDraft::new(
                        "r",
                        "owner",
                        "plan_scope_answered",
                        serde_json::json!({"mode": "fresh"}),
                    ),
                )?;
                Ok(())
            })
            .unwrap();
        r
    }

    /// A repo nobody has been asked about yet.
    fn undecided() -> Self {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().to_path_buf();
        std::fs::create_dir_all(root.join(super::PLANS_DIR)).unwrap();
        Self {
            _dir: dir,
            root,
            store: Store::open_in_memory().unwrap(),
        }
    }

    fn write(&self, name: &str, text: &str) -> PathBuf {
        let p = self.root.join(super::PLANS_DIR).join(name);
        std::fs::write(&p, text).unwrap();
        p
    }

    fn scan(&mut self) -> Vec<ImportOutcome> {
        scan(
            &mut self.store,
            "r",
            &self.root,
            &ctx(),
            ScanBudget::SESSION_START,
        )
        .unwrap()
    }

    fn import(&mut self, path: &Path, into: Option<&str>) -> ImportOutcome {
        import_path(&mut self.store, "r", &self.root, path, &ctx(), into).unwrap()
    }

    fn kinds(&self) -> Vec<String> {
        read_all(self.store.conn(), Some("r"))
            .unwrap()
            .into_iter()
            .map(|e| e.kind)
            .filter(|k| !k.starts_with("plan_scope_"))
            .collect()
    }

    fn tree(&self) -> Vec<(String, String, NodeStatus)> {
        load_tree(self.store.conn(), "r", false)
            .unwrap()
            .into_iter()
            .map(|n| (n.id, n.title, n.status))
            .collect()
    }
}

fn ctx() -> ImportCtx {
    ImportCtx {
        session_id: "owner".into(),
        actor: Actor::default(),
        dry_run: false,
    }
}

fn plan_text(title: &str, tasks: &[(&str, &[(&str, bool)])]) -> String {
    let mut out = format!("# {title} Implementation Plan\n\n**Goal:** {title} works.\n\n");
    for (i, (t, steps)) in tasks.iter().enumerate() {
        let _ = write!(out, "### Task {}: {t}\n\n", i + 1);
        for (s, checked) in *steps {
            let _ = writeln!(out, "- [{}] {s}", if *checked { "x" } else { " " });
        }
        out.push('\n');
    }
    out
}

/// The file's bytes must not change: imports only read.
fn unchanged_on_disk(path: &Path, before: &[u8]) {
    assert_eq!(std::fs::read(path).unwrap(), before);
}

#[test]
fn first_import_creates_goal_then_milestones() {
    let mut r = Repo::new();
    let a_text = plan_text(
        "Login",
        &[
            ("Endpoint", &[("test", false), ("code", false)]),
            ("Form", &[]),
        ],
    );
    let a = r.write("a.md", &a_text);
    let out = r.scan();
    assert!(
        matches!(out[0], ImportOutcome::Imported { added: 4, .. }),
        "{out:?}"
    );
    let g = get_node(r.store.conn(), "r", "G").unwrap().unwrap();
    assert_eq!(
        (g.title.as_str(), g.intent.as_deref()),
        ("Login", Some("Login works."))
    );
    let t = get_node(r.store.conn(), "r", "1").unwrap().unwrap();
    assert_eq!((t.kind, t.title.as_str()), (NodeKind::Task, "Endpoint"));
    assert_eq!(
        t.source,
        NodeSource::File {
            file: "docs/superpowers/plans/a.md".into(),
            task: 1,
            step: None
        }
    );
    let b_text = plan_text("Signup", &[("Page", &[])]);
    let b = r.write("b.md", &b_text);
    r.scan();
    let m = get_node(r.store.conn(), "r", "3").unwrap().unwrap();
    assert_eq!((m.kind, m.title.as_str()), (NodeKind::Milestone, "Signup"));
    assert_eq!(
        get_node(r.store.conn(), "r", "3.1").unwrap().unwrap().title,
        "Page"
    );
    unchanged_on_disk(&a, a_text.as_bytes());
    unchanged_on_disk(&b, b_text.as_bytes());
}

#[test]
fn reimport_is_a_no_op() {
    let mut r = Repo::new();
    let text = plan_text("Login", &[("Endpoint", &[("test", true)])]);
    let a = r.write("a.md", &text);
    r.scan();
    let before = r.kinds().len();
    // Same file again, by hand and by scan: nothing.
    assert!(matches!(
        r.import(&a, None),
        ImportOutcome::Unchanged { .. }
    ));
    assert_eq!(r.scan(), vec![]);
    // Same content with a new mtime: read again, but still nothing written.
    std::thread::sleep(std::time::Duration::from_millis(20));
    std::fs::write(&a, &text).unwrap();
    let out = r.scan();
    assert!(matches!(out[0], ImportOutcome::Unchanged { .. }), "{out:?}");
    assert_eq!(r.kinds().len(), before);
    unchanged_on_disk(&a, text.as_bytes());
}

#[test]
fn edits_become_plan_events() {
    let mut r = Repo::new();
    let a = r.write(
        "a.md",
        &plan_text("Login", &[("Endpoint", &[("test", false)]), ("Form", &[])]),
    );
    r.scan();
    let before = r.kinds().len();
    let text = plan_text(
        "Login",
        &[
            ("Endpoint API", &[("test", true), ("docs", false)]),
            ("Form", &[]),
            ("Logout", &[]),
        ],
    );
    std::fs::write(&a, &text).unwrap();
    let out = r.scan();
    assert!(
        matches!(
            out[0],
            ImportOutcome::Imported {
                claimed: 1,
                replan: true,
                ..
            }
        ),
        "{out:?}"
    );
    let new: Vec<String> = r.kinds().split_off(before);
    assert!(new.contains(&"node_edited".to_string()), "{new:?}");
    assert_eq!(
        new.iter().filter(|k| *k == "node_added").count(),
        2,
        "{new:?}"
    );
    assert!(new.contains(&"claimed_done".to_string()));
    assert!(!new.contains(&"verified".to_string()));
    assert!(!new.contains(&"checkpoint_due".to_string()));
    assert!(!new.contains(&"decision_asked".to_string()));
    assert_eq!(
        get_node(r.store.conn(), "r", "1").unwrap().unwrap().title,
        "Endpoint API"
    );
    assert_eq!(
        get_node(r.store.conn(), "r", "1.1")
            .unwrap()
            .unwrap()
            .status,
        NodeStatus::Claimed
    );
    assert_eq!(
        get_node(r.store.conn(), "r", "1.2").unwrap().unwrap().title,
        "docs"
    );
    assert_eq!(
        get_node(r.store.conn(), "r", "3").unwrap().unwrap().title,
        "Logout"
    );
    unchanged_on_disk(&a, text.as_bytes());
}

#[test]
fn big_change_is_a_replan() {
    let mut r = Repo::new();
    let names: Vec<String> = (1..=10).map(|i| format!("Task {i}")).collect();
    let tasks: Vec<(&str, &[(&str, bool)])> = names.iter().map(|n| (n.as_str(), &[][..])).collect();
    let a = r.write("a.md", &plan_text("Big", &tasks));
    r.scan();
    let before = r.kinds().len();
    let mut changed = names.clone();
    for n in changed.iter_mut().take(4) {
        n.push_str(" renamed");
    }
    let tasks: Vec<(&str, &[(&str, bool)])> =
        changed.iter().map(|n| (n.as_str(), &[][..])).collect();
    std::fs::write(&a, plan_text("Big", &tasks)).unwrap();
    r.scan();
    let new = r.kinds().split_off(before);
    assert_eq!(new.iter().filter(|k| *k == "replan").count(), 1, "{new:?}");
    assert_eq!(new[0], "replan");
    // Three of ten is under the line: no replan.
    let mut r2 = Repo::new();
    let tasks: Vec<(&str, &[(&str, bool)])> = names.iter().map(|n| (n.as_str(), &[][..])).collect();
    let b = r2.write("b.md", &plan_text("Big", &tasks));
    r2.scan();
    let mut changed = names.clone();
    for n in changed.iter_mut().take(3) {
        n.push_str(" renamed");
    }
    let tasks: Vec<(&str, &[(&str, bool)])> =
        changed.iter().map(|n| (n.as_str(), &[][..])).collect();
    std::fs::write(&b, plan_text("Big", &tasks)).unwrap();
    r2.scan();
    assert!(!r2.kinds().contains(&"replan".to_string()));
}

#[test]
fn import_keeps_tasks_in_progress() {
    let mut r = Repo::new();
    let a = r.write(
        "a.md",
        &plan_text("P", &[("Busy", &[]), ("Idle", &[]), ("Done", &[])]),
    );
    r.scan();
    r.store
        .append(EventDraft::new(
            "r",
            "s1",
            "session_started",
            serde_json::json!({"cwd": "/x", "source": "startup"}),
        ))
        .unwrap();
    r.store
        .append(
            EventDraft::new(
                "r",
                "s1",
                "task_claimed",
                serde_json::json!({"task_id": "1", "mode": "confirmed", "confidence": 1.0}),
            )
            .task("1"),
        )
        .unwrap();
    r.store
        .append(
            EventDraft::new(
                "r",
                "s1",
                "claimed_done",
                serde_json::json!({"task_id": "3", "evidence": "x"}),
            )
            .task("3"),
        )
        .unwrap();
    // Tasks 1–3 are gone from the file; task 4 is new.
    std::fs::write(&a, "# P\n\n### Task 4: Other\n").unwrap();
    let out = r.scan();
    assert!(
        matches!(
            out[0],
            ImportOutcome::Imported {
                removed: 1,
                kept: 2,
                ..
            }
        ),
        "{out:?}"
    );
    let tree = r.tree();
    let ids: Vec<&str> = tree.iter().map(|(id, _, _)| id.as_str()).collect();
    assert_eq!(ids, vec!["G", "1", "3", "4"]);
    assert_eq!(
        get_node(r.store.conn(), "r", "1").unwrap().unwrap().title,
        "Busy"
    );
    // The keep is recorded with a reason, and the file no longer lists those tasks.
    let edits: Vec<serde_json::Value> = read_all(r.store.conn(), Some("r"))
        .unwrap()
        .into_iter()
        .filter(|e| e.kind == "node_edited")
        .map(|e| e.data)
        .collect();
    assert_eq!(edits.len(), 2);
    assert!(
        edits[0]["reason"]
            .as_str()
            .unwrap()
            .contains("no longer in")
    );
    assert_eq!(edits[0]["title"], serde_json::Value::Null);
    let items = super::source_items(r.store.conn(), "r", "docs/superpowers/plans/a.md").unwrap();
    assert_eq!(items.len(), 1);
}

#[test]
fn renaming_a_task_in_progress_keeps_its_work_and_its_old_title() {
    let mut r = Repo::new();
    let a = r.write("a.md", &plan_text("P", &[("Busy", &[("s", false)])]));
    r.scan();
    for (kind, data) in [
        (
            "session_started",
            serde_json::json!({"cwd": "/x", "source": "startup"}),
        ),
        (
            "task_claimed",
            serde_json::json!({"task_id": "1", "mode": "confirmed", "confidence": 1.0}),
        ),
    ] {
        r.store
            .append(EventDraft::new("r", "s1", kind, data).task("1"))
            .unwrap();
    }
    std::fs::write(&a, plan_text("P", &[("Busy, renamed", &[("s", false)])])).unwrap();
    let out = r.scan();
    assert!(
        matches!(out[0], ImportOutcome::Imported { edited: 1, .. }),
        "{out:?}"
    );
    let n = get_node(r.store.conn(), "r", "1").unwrap().unwrap();
    assert_eq!(
        (n.title.as_str(), n.status),
        ("Busy, renamed", NodeStatus::Active)
    );
    // The old title is still in the history, and the rename says where it came from.
    let events = read_all(r.store.conn(), Some("r")).unwrap();
    assert!(
        events
            .iter()
            .any(|e| e.kind == "node_added" && e.data["id"] == "1" && e.data["title"] == "Busy")
    );
    let rename = events
        .iter()
        .find(|e| e.kind == "node_edited" && e.data["title"] == "Busy, renamed")
        .expect("a node_edited for the rename");
    assert!(
        rename.data["reason"]
            .as_str()
            .unwrap()
            .starts_with("imported from "),
        "{}",
        rename.data
    );
}

#[test]
fn existing_plan_gets_a_baseline() {
    let mut r = Repo::new();
    let ectx = EditContext {
        repo_id: "r".into(),
        session_id: "owner".into(),
        actor: Actor::default(),
        reason: None,
        replan: false,
    };
    plan_edit(
        &mut r.store,
        &ectx,
        &[
            PlanOp::AddNode {
                parent: None,
                kind: NodeKind::Goal,
                title: "Hand".into(),
                intent: Some("why".into()),
                estimate_min: None,
                expected_files: vec![],
            },
            PlanOp::AddNode {
                parent: Some("G".into()),
                kind: NodeKind::Milestone,
                title: "SP1".into(),
                intent: Some("why".into()),
                estimate_min: None,
                expected_files: vec![],
            },
        ],
    )
    .unwrap();
    let old = plan_text("Old", &[("A", &[]), ("B", &[])]);
    let a = r.write("a.md", &old);
    let out = r.scan();
    assert!(matches!(out[0], ImportOutcome::Baseline { .. }), "{out:?}");
    assert_eq!(r.tree().len(), 2, "no nodes from a baseline");
    assert_eq!(r.scan(), vec![]);
    // A file added afterwards is imported.
    r.write("b.md", &plan_text("New", &[("C", &[])]));
    let out = r.scan();
    assert!(matches!(out[0], ImportOutcome::Imported { .. }), "{out:?}");
    assert_eq!(
        get_node(r.store.conn(), "r", "2").unwrap().unwrap().title,
        "New"
    );
    // A baseline file whose title is an existing milestone's joins it when it changes.
    std::fs::write(&a, plan_text("SP1", &[("A", &[]), ("B", &[])])).unwrap();
    let out = r.scan();
    assert!(
        matches!(&out[0], ImportOutcome::Imported { node_id, .. } if node_id == "1"),
        "{out:?}"
    );
    unchanged_on_disk(
        &r.root.join(super::PLANS_DIR).join("b.md"),
        plan_text("New", &[("C", &[])]).as_bytes(),
    );
}

#[test]
fn changed_baseline_file_without_a_match_needs_a_link() {
    let mut r = Repo::new();
    let ectx = EditContext {
        repo_id: "r".into(),
        session_id: "owner".into(),
        actor: Actor::default(),
        reason: None,
        replan: false,
    };
    plan_edit(
        &mut r.store,
        &ectx,
        &[
            PlanOp::AddNode {
                parent: None,
                kind: NodeKind::Goal,
                title: "Hand".into(),
                intent: Some("why".into()),
                estimate_min: None,
                expected_files: vec![],
            },
            PlanOp::AddNode {
                parent: Some("G".into()),
                kind: NodeKind::Milestone,
                title: "SP7 Telemetry and forecast".into(),
                intent: Some("why".into()),
                estimate_min: None,
                expected_files: vec![],
            },
        ],
    )
    .unwrap();
    let a = r.write(
        "sp7.md",
        &plan_text("Sub-project 7: Telemetry", &[("A", &[])]),
    );
    assert!(matches!(r.scan()[0], ImportOutcome::Baseline { .. }));
    let kinds_before = r.kinds().len();
    // The old plan file is edited: no milestone has its title, so nothing is created.
    let edited = plan_text("Sub-project 7: Telemetry", &[("A", &[]), ("B", &[])]);
    std::fs::write(&a, &edited).unwrap();
    let out = r.scan();
    assert!(
        matches!(&out[0], ImportOutcome::NeedsLink { path } if path.ends_with("sp7.md")),
        "{out:?}"
    );
    let line = out[0].summary().unwrap();
    assert!(
        line.contains("isn't linked to a milestone") && line.contains("--into <milestone>"),
        "{line}"
    );
    assert_eq!(r.tree().len(), 2, "no duplicate milestone, no tasks");
    assert!(!r.kinds().contains(&"plan_imported".to_string()));
    assert_eq!(r.kinds().len(), kinds_before + 1, "one plan_file_seen");
    // Said once: the same file isn't reported or read again.
    assert_eq!(r.scan(), vec![]);
    let src = super::sources(r.store.conn(), "r").unwrap();
    assert!(src[0].needs_link && src[0].baseline);
    r.store.rebuild().unwrap();
    assert_eq!(super::sources(r.store.conn(), "r").unwrap(), src);
    // Linking by hand imports it into the milestone and clears the flag.
    let out = r.import(&a, Some("1"));
    assert!(matches!(out, ImportOutcome::Imported { .. }), "{out:?}");
    assert!(!super::sources(r.store.conn(), "r").unwrap()[0].needs_link);
    assert_eq!(r.tree().len(), 4);
    unchanged_on_disk(&a, edited.as_bytes());
}

#[test]
fn files_over_the_budget_wait_for_the_next_scan() {
    let mut r = Repo::new();
    for i in 0..25 {
        r.write(
            &format!("p{i:02}.md"),
            &plan_text(&format!("Plan {i:02}"), &[("T", &[])]),
        );
    }
    let imported = |out: &[ImportOutcome]| {
        out.iter()
            .filter(|o| matches!(o, ImportOutcome::Imported { .. }))
            .count()
    };
    // At most 20 files are read per scan; the unchanged ones don't use up the budget.
    assert_eq!(imported(&r.scan()), 20);
    assert_eq!(imported(&r.scan()), 5);
    assert_eq!(r.scan(), vec![]);
    // A change to the last file is still picked up.
    r.write("p24.md", &plan_text("Plan 24", &[("T", &[]), ("U", &[])]));
    assert_eq!(imported(&r.scan()), 1);
}

#[test]
fn into_links_a_file_to_a_milestone() {
    let mut r = Repo::new();
    let ectx = EditContext {
        repo_id: "r".into(),
        session_id: "owner".into(),
        actor: Actor::default(),
        reason: None,
        replan: false,
    };
    plan_edit(
        &mut r.store,
        &ectx,
        &[
            PlanOp::AddNode {
                parent: None,
                kind: NodeKind::Goal,
                title: "Hand".into(),
                intent: Some("why".into()),
                estimate_min: None,
                expected_files: vec![],
            },
            PlanOp::AddNode {
                parent: Some("G".into()),
                kind: NodeKind::Milestone,
                title: "SP7".into(),
                intent: Some("why".into()),
                estimate_min: None,
                expected_files: vec![],
            },
            PlanOp::AddNode {
                parent: Some("1".into()),
                kind: NodeKind::Task,
                title: "words".into(),
                intent: None,
                estimate_min: None,
                expected_files: vec![],
            },
        ],
    )
    .unwrap();
    let a = r.write(
        "sp7.md",
        &plan_text("Polish", &[("Words", &[]), ("Screens", &[])]),
    );
    let out = r.import(&a, Some("1"));
    assert!(
        matches!(out, ImportOutcome::Imported { added: 1, .. }),
        "{out:?}"
    );
    let t = get_node(r.store.conn(), "r", "1.1").unwrap().unwrap();
    assert_eq!(t.title, "words", "matched by title, not renamed");
    assert!(matches!(t.source, NodeSource::File { task: 1, .. }));
    assert_eq!(
        get_node(r.store.conn(), "r", "1.2").unwrap().unwrap().title,
        "Screens"
    );
    // The milestone's own intent is left alone on a hand link.
    assert_eq!(
        get_node(r.store.conn(), "r", "1")
            .unwrap()
            .unwrap()
            .intent
            .as_deref(),
        Some("why")
    );
    // And a later scan sees it as linked and unchanged.
    assert_eq!(r.scan(), vec![]);
}

#[test]
fn dry_run_writes_nothing() {
    let mut r = Repo::new();
    let a = r.write("a.md", &plan_text("Login", &[("Endpoint", &[])]));
    let mut c = ctx();
    c.dry_run = true;
    let out = import_path(&mut r.store, "r", &r.root, &a, &c, None).unwrap();
    assert!(
        matches!(&out, ImportOutcome::Imported { changes, .. } if changes.len() == 2),
        "{out:?}"
    );
    assert_eq!(r.kinds(), Vec::<String>::new());
}

#[test]
fn odd_files_in_the_folder_import_nothing() {
    let mut r = Repo::new();
    r.write("empty.md", "");
    r.write("notes.md", "# Notes\n\nnothing here\n");
    std::fs::write(
        r.root.join(super::PLANS_DIR).join("bin.md"),
        [0xff, 0xfe, 0, 1],
    )
    .unwrap();
    r.write("dup.md", "### Task 1: A\n### Task 1: B\n");
    let big = "x".repeat(5 << 20);
    r.write("huge.md", &big);
    let out = r.scan();
    assert_eq!(
        r.tree().len(),
        2,
        "only dup.md's first task and its goal: {out:?}"
    );
    assert!(
        out.iter()
            .all(|o| !matches!(o, ImportOutcome::Failed { .. })),
        "{out:?}"
    );
    // A missing folder is nothing at all.
    std::fs::remove_dir_all(r.root.join("docs")).unwrap();
    assert_eq!(r.scan(), vec![]);
}

#[test]
fn rebuild_keeps_import_views() {
    let mut r = Repo::new();
    let a = r.write(
        "a.md",
        &plan_text("Login", &[("Endpoint", &[("t", true)]), ("Form", &[])]),
    );
    r.scan();
    std::fs::write(
        &a,
        plan_text("Login", &[("Endpoint", &[("t", true), ("u", false)])]),
    )
    .unwrap();
    r.scan();
    let dump = |s: &Store| -> Vec<String> {
        let mut out = Vec::new();
        for q in [
            "SELECT repo_id||path||hash||size||mtime||coalesce(node_id,'')||baseline||imported_at FROM plan_sources ORDER BY path",
            "SELECT path||task_no||step_no||node_id||title FROM plan_source_items ORDER BY 1",
            "SELECT id||status||source FROM plan_nodes ORDER BY id",
        ] {
            let mut st = s.conn().prepare(q).unwrap();
            out.extend(
                st.query_map([], |r| r.get::<_, String>(0))
                    .unwrap()
                    .map(Result::unwrap),
            );
        }
        out
    };
    let before = dump(&r.store);
    r.store.rebuild().unwrap();
    assert_eq!(dump(&r.store), before);
}

#[test]
fn plans_outside_the_superpowers_folder_import() {
    let mut r = Repo::new();
    std::fs::create_dir_all(r.root.join("planning")).unwrap();
    std::fs::write(
        r.root.join("planning/login.md"),
        plan_text("Login", &[("Endpoint", &[])]),
    )
    .unwrap();
    // A matched name that is no plan costs a read, never an import.
    std::fs::write(r.root.join("todo.md"), "# Todo\n\n- buy milk\n").unwrap();
    let out = r.scan();
    assert!(
        out.iter().any(
            |o| matches!(o, ImportOutcome::Imported { path, .. } if path == "planning/login.md")
        ),
        "{out:?}"
    );
    assert_eq!(r.tree().len(), 2, "goal and its task: {out:?}");
}

fn scope_events(r: &Repo) -> Vec<String> {
    read_all(r.store.conn(), Some("r"))
        .unwrap()
        .into_iter()
        .map(|e| e.kind)
        .filter(|k| k.starts_with("plan_scope_"))
        .collect()
}

#[test]
fn plans_with_nothing_ticked_raise_one_question_and_import_nothing() {
    let mut r = Repo::undecided();
    r.write(
        "a.md",
        &plan_text("Login", &[("Endpoint", &[("t", false)])]),
    );
    r.write("b.md", &plan_text("Search", &[("Index", &[])]));
    let out = r.scan();
    assert!(
        matches!(&out[..], [ImportOutcome::NeedsScope { paths }] if paths.len() == 2),
        "{out:?}"
    );
    assert_eq!(r.tree(), vec![]);
    // Asked once: later scans repeat the question without a new event.
    let again = r.scan();
    assert!(matches!(&again[..], [ImportOutcome::NeedsScope { .. }]));
    assert_eq!(scope_events(&r), vec!["plan_scope_asked"]);
    let q = super::scope::open(r.store.conn(), "r").unwrap().unwrap();
    assert_eq!(q.paths.len(), 2);
    assert!(
        out[0]
            .summary()
            .unwrap()
            .starts_with("Found 2 plan files (a, b)")
    );
}

#[test]
fn a_ticked_step_means_the_ticks_are_the_status() {
    let mut r = Repo::undecided();
    r.write("a.md", &plan_text("Login", &[("Endpoint", &[("t", true)])]));
    r.write("b.md", &plan_text("Search", &[("Index", &[])]));
    let out = r.scan();
    assert!(
        out.iter()
            .all(|o| matches!(o, ImportOutcome::Imported { .. })),
        "{out:?}"
    );
    assert_eq!(scope_events(&r), Vec::<String>::new());
}

#[test]
fn files_without_tasks_raise_no_question() {
    let mut r = Repo::undecided();
    r.write("notes.md", "# Notes\n\nnothing\n");
    let out = r.scan();
    assert!(
        !out.iter()
            .any(|o| matches!(o, ImportOutcome::NeedsScope { .. }))
    );
    assert_eq!(scope_events(&r), Vec::<String>::new());
}

#[test]
fn a_fresh_run_imports_everything_at_zero_and_is_not_asked_again() {
    let mut r = Repo::undecided();
    r.write("a.md", &plan_text("Login", &[("Endpoint", &[])]));
    r.write("b.md", &plan_text("Search", &[("Index", &[])]));
    r.scan();
    let line = super::scope::answer(&mut r.store, "r", &r.root, &ctx(), &[]).unwrap();
    assert!(
        line.starts_with("Fresh run: imported 2 plan files"),
        "{line}"
    );
    assert_eq!(
        r.tree().len(),
        4,
        "goal, milestone, two tasks: {:?}",
        r.tree()
    );
    assert!(r.tree().iter().all(|(_, _, s)| *s == NodeStatus::Todo));
    assert!(super::scope::open(r.store.conn(), "r").unwrap().is_none());
    r.write("c.md", &plan_text("Billing", &[("Invoice", &[])]));
    r.scan();
    assert_eq!(
        scope_events(&r),
        vec!["plan_scope_asked", "plan_scope_answered"]
    );
    // A second answer is stale.
    let again = super::scope::answer(&mut r.store, "r", &r.root, &ctx(), &[]);
    assert!(matches!(again, Err(Error::Stale(_))), "{again:?}");
}

#[test]
fn some_done_baselines_the_named_files_and_imports_the_rest() {
    let mut r = Repo::undecided();
    r.write("a.md", &plan_text("Login", &[("Endpoint", &[])]));
    r.write("b.md", &plan_text("Search", &[("Index", &[])]));
    r.scan();
    let bad = super::scope::answer(&mut r.store, "r", &r.root, &ctx(), &["nope".into()]);
    assert!(matches!(bad, Err(Error::Invalid(_))), "{bad:?}");
    assert!(super::scope::open(r.store.conn(), "r").unwrap().is_some());
    super::scope::answer(&mut r.store, "r", &r.root, &ctx(), &["a".into()]).unwrap();
    let sources = super::sources(r.store.conn(), "r").unwrap();
    let a = sources.iter().find(|s| s.path.ends_with("a.md")).unwrap();
    let b = sources.iter().find(|s| s.path.ends_with("b.md")).unwrap();
    assert!(a.baseline && a.node_id.is_none());
    assert!(!b.baseline && b.node_id.is_some());
    let titles: Vec<String> = r.tree().into_iter().map(|(_, t, _)| t).collect();
    assert!(titles.contains(&"Index".to_string()) && !titles.contains(&"Endpoint".to_string()));
}

#[test]
fn the_question_is_on_the_needs_you_queue_and_the_owner_answers_it() {
    use crate::config::Config;
    use crate::owner::{ActCtx, OwnerAction, act};
    use crate::paths::Home;
    use crate::view::{NeedsKind, glance};

    let mut r = Repo::undecided();
    r.store
        .conn()
        .execute(
            "INSERT INTO repos (id, name, root, created_at) VALUES ('r', 'shop', ?1, 't')",
            [r.root.to_string_lossy().as_ref()],
        )
        .unwrap();
    r.write("a.md", &plan_text("Login", &[("Endpoint", &[])]));
    r.scan();
    let now = jiff::Timestamp::now();
    let g = glance(r.store.conn(), &Config::default(), now).unwrap();
    let item = g
        .needs_you
        .iter()
        .find(|i| i.kind == NeedsKind::PlanScope)
        .expect("the question is queued");
    let labels: Vec<&str> = item.actions.iter().map(|a| a.label.as_str()).collect();
    assert_eq!(labels, vec!["Fresh run", "Some are done"]);
    assert!(
        item.title.starts_with("Found 1 plan file (a)"),
        "{}",
        item.title
    );

    let home_dir = tempfile::tempdir().unwrap();
    let home = Home::at(home_dir.path());
    let cfg = Config::default();
    let ctx = ActCtx {
        home: &home,
        cfg: &cfg,
        now,
        dir: None,
    };
    let reply = act(&mut r.store, &ctx, &item.actions[0].action).unwrap();
    assert!(reply.starts_with("Fresh run"), "{reply}");
    let g = glance(r.store.conn(), &Config::default(), now).unwrap();
    assert!(g.needs_you.iter().all(|i| i.kind != NeedsKind::PlanScope));
    // A second click is stale.
    let again = act(
        &mut r.store,
        &ctx,
        &OwnerAction::PlanScope {
            repo: "r".into(),
            done: None,
        },
    );
    assert!(matches!(again, Err(Error::Stale(_))), "{again:?}");
}
