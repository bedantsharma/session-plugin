use std::path::Path;

use jiff::{SignedDuration, Timestamp};
use serde_json::{Value, json};

use super::*;
use crate::git::tests::{repo, sh};
use crate::handoff::{LeftoverAction, LeftoverState, ResolvedBy};
use crate::store::{EventDraft, Store, append_in_at, read_all};

const T0: &str = "2026-10-13T09:00:00Z";

fn at(secs: i64) -> Timestamp {
    T0.parse::<Timestamp>().unwrap() + SignedDuration::from_secs(secs)
}

fn ev(s: &mut Store, session: &str, kind: &str, data: Value, secs: i64) {
    s.write(|tx| append_in_at(tx, EventDraft::new("r", session, kind, data), at(secs)))
        .unwrap();
}

/// A plan: G, 1, tasks 1.1 and 1.2 (1.2 claimed done by someone earlier).
fn plan() -> Store {
    let mut s = Store::open_in_memory().unwrap();
    for (id, parent, kind) in [
        ("G", None, "goal"),
        ("1", Some("G"), "milestone"),
        ("1.1", Some("1"), "task"),
        ("1.2", Some("1"), "task"),
    ] {
        ev(
            &mut s,
            "owner",
            "node_added",
            json!({"id": id, "parent_id": parent, "kind": kind, "title": format!("T{id}"),
                   "intent": "why", "expected_files": []}),
            0,
        );
    }
    ev(
        &mut s,
        "old",
        "session_started",
        json!({"cwd": "/x", "source": "startup"}),
        0,
    );
    ev(
        &mut s,
        "old",
        "claimed_done",
        json!({"task_id": "1.2", "evidence": "e"}),
        1,
    );
    ev(
        &mut s,
        "old",
        "session_ended",
        json!({"end_kind": "clean"}),
        2,
    );
    s
}

fn start(s: &mut Store, id: &str, wt: &Path, secs: i64) {
    ev(
        s,
        id,
        "session_started",
        json!({"cwd": wt.to_str().unwrap(), "worktree": wt.to_str().unwrap(), "source": "startup"}),
        secs,
    );
}

/// One finished unit of `session` on `task` touching `files`, optionally labelled.
fn unit(s: &mut Store, session: &str, task: &str, files: &[&str], label: Option<&str>, secs: i64) {
    ev(
        s,
        session,
        "task_claimed",
        json!({"task_id": task, "mode": "auto", "confidence": 0.9}),
        secs,
    );
    ev(s, session, "prompt", json!({"chars": 1}), secs + 1);
    ev(
        s,
        session,
        "file_changed",
        json!({"tool": "Edit", "files": files}),
        secs + 2,
    );
    ev(s, session, "unit_stopped", json!({}), secs + 3);
    if let Some(l) = label {
        let n = session::last_unit(s.conn(), session)
            .unwrap()
            .unwrap()
            .unit_no;
        ev(
            s,
            session,
            "unit_labeled",
            json!({"unit_no": n, "label": l, "source": "agent"}),
            secs + 4,
        );
    }
}

fn write(dir: &Path, path: &str, text: &str) {
    let p = dir.join(path);
    std::fs::create_dir_all(p.parent().unwrap()).unwrap();
    std::fs::write(p, text).unwrap();
}

fn commit_files(dir: &Path, files: &[&str]) {
    for f in files {
        write(dir, f, "base\n");
    }
    sh(dir, &["add", "-A"]);
    sh(dir, &["commit", "-q", "-m", "base"]);
}

/// Dead session `d` in `wt` left a.rs (1.1), b.rs (1.2, claimed), c.rs (drift only) and
/// an untracked d.rs nobody recorded; `me` starts in `wt`.
fn scenario(wt: &Path) -> Store {
    let mut s = plan();
    commit_files(wt, &["a.rs", "b.rs", "c.rs"]);
    start(&mut s, "d", wt, 10);
    unit(&mut s, "d", "1.2", &["b.rs"], Some("on_task"), 20);
    unit(&mut s, "d", "1.1", &["c.rs"], Some("drift"), 30);
    unit(&mut s, "d", "1.1", &["a.rs"], Some("on_task"), 40);
    ev(
        &mut s,
        "d",
        "session_ended",
        json!({"reason": "other", "end_kind": "dirty"}),
        50,
    );
    write(wt, "a.rs", "a work\n");
    write(wt, "b.rs", "b work\n");
    write(wt, "c.rs", "c drift\n");
    write(wt, "new dir/d.rs", "d untracked\n");
    start(&mut s, "me", wt, 60);
    s
}

fn by_path(found: &[Leftover], path: &str) -> Leftover {
    found
        .iter()
        .find(|l| l.path == path)
        .unwrap_or_else(|| panic!("{path} not in {found:?}"))
        .clone()
}

fn agent(id: &str) -> Caller {
    Caller::Agent {
        session_id: id.into(),
    }
}

#[test]
fn end_kind_rules() {
    let mut s = plan();
    start(&mut s, "a", Path::new("/w"), 1);
    let get = |s: &Store| session::get(s.conn(), "a").unwrap().unwrap();
    assert_eq!(end_kind(s.conn(), &get(&s), &[]).unwrap(), EndKind::Clean);
    assert_eq!(
        end_kind(s.conn(), &get(&s), &["x".into()]).unwrap(),
        EndKind::Dirty
    );
    ev(
        &mut s,
        "a",
        "task_claimed",
        json!({"task_id": "1.1", "mode": "auto", "confidence": 1}),
        2,
    );
    assert_eq!(
        end_kind(s.conn(), &get(&s), &[]).unwrap(),
        EndKind::Dirty,
        "task unfinished"
    );
    ev(
        &mut s,
        "a",
        "claimed_done",
        json!({"task_id": "1.1", "evidence": "e"}),
        3,
    );
    assert_eq!(end_kind(s.conn(), &get(&s), &[]).unwrap(), EndKind::Clean);
}

#[test]
fn crash_scan_marks_silent_sessions_once() {
    let mut s = plan();
    start(&mut s, "quiet", Path::new("/w"), 0);
    ev(&mut s, "quiet", "prompt", json!({"chars": 1}), 10);
    start(&mut s, "idle", Path::new("/w"), 0);
    ev(&mut s, "idle", "prompt", json!({"chars": 1}), 1000);
    ev(&mut s, "idle", "unit_stopped", json!({}), 1010);
    // Idle notifications aren't activity.
    ev(
        &mut s,
        "idle",
        "needs_owner",
        json!({"kind": "idle_prompt"}),
        3000,
    );
    start(&mut s, "busy", Path::new("/w"), 0);
    ev(&mut s, "busy", "prompt", json!({"chars": 1}), 2800);

    let marked = scan_crashed(&mut s, "r", at(2850), 30).unwrap();
    let mut sorted = marked.clone();
    sorted.sort();
    assert_eq!(sorted, vec!["idle", "quiet"]);
    let quiet = session::get(s.conn(), "quiet").unwrap().unwrap();
    assert_eq!(quiet.end_kind, Some(EndKind::Crashed));
    assert_eq!(quiet.ended_at.as_deref(), Some(at(10).to_string().as_str()));
    let idle = session::get(s.conn(), "idle").unwrap().unwrap();
    assert_eq!(
        idle.ended_at.as_deref(),
        Some(at(1010).to_string().as_str())
    );
    assert_eq!(idle.active_secs, 10);
    let busy = session::get(s.conn(), "busy").unwrap().unwrap();
    assert_eq!(busy.end_kind, None);
    // Idempotent.
    assert_eq!(
        scan_crashed(&mut s, "r", at(2850), 30).unwrap(),
        Vec::<String>::new()
    );
    let crashes = read_all(s.conn(), None)
        .unwrap()
        .into_iter()
        .filter(|e| e.kind == "session_crashed")
        .count();
    assert_eq!(crashes, 2);
}

#[test]
fn analysis_attributes_files_and_suggests() {
    let tmp = repo();
    let wt = tmp.path();
    let mut s = scenario(wt);
    let out = analyse(&mut s, "r", "me", wt).unwrap();
    assert_eq!(out.found.len(), 4, "{out:?}");
    let la = by_path(&out.found, "a.rs");
    assert_eq!(
        (la.task_id.as_deref(), la.suggested),
        (Some("1.1"), LeftoverAction::Resume)
    );
    let lb = by_path(&out.found, "b.rs");
    assert_eq!(
        (lb.task_id.as_deref(), lb.suggested),
        (Some("1.2"), LeftoverAction::Adopt)
    );
    let lc = by_path(&out.found, "c.rs");
    assert_eq!(
        (lc.task_id.as_deref(), lc.suggested),
        (Some("1.1"), LeftoverAction::Undo)
    );
    let ld = by_path(&out.found, "new dir/d.rs");
    assert_eq!(
        (ld.task_id.as_deref(), ld.suggested),
        (Some("1.1"), LeftoverAction::Undo)
    );
    assert!(
        out.found
            .iter()
            .all(|l| l.state == LeftoverState::Open && l.from_session == "d")
    );
    assert!(
        session::get(s.conn(), "d")
            .unwrap()
            .unwrap()
            .leftovers_analysed
    );
    // Never triaged twice.
    assert_eq!(analyse(&mut s, "r", "me", wt).unwrap(), Analysis::default());
    assert_eq!(leftovers(s.conn(), "r", true).unwrap().len(), 4);
}

#[test]
fn undo_is_reversible() {
    let tmp = repo();
    let wt = tmp.path();
    let mut s = scenario(wt);
    let out = analyse(&mut s, "r", "me", wt).unwrap();
    let ids = vec![
        by_path(&out.found, "a.rs").leftover_id,
        by_path(&out.found, "new dir/d.rs").leftover_id,
    ];
    let snaps = tempfile::tempdir().unwrap();
    let res = resolve(
        &mut s,
        "r",
        snaps.path(),
        &ids,
        LeftoverAction::Undo,
        &agent("me"),
    )
    .unwrap();
    let stash = res.stash.unwrap();
    // Gone from the worktree; the others untouched.
    assert_eq!(std::fs::read_to_string(wt.join("a.rs")).unwrap(), "base\n");
    assert!(!wt.join("new dir/d.rs").exists());
    assert_eq!(
        std::fs::read_to_string(wt.join("b.rs")).unwrap(),
        "b work\n"
    );
    for id in &ids {
        let snap: Value = serde_json::from_str(
            &std::fs::read_to_string(snaps.path().join(format!("{id}.json"))).unwrap(),
        )
        .unwrap();
        assert_eq!(snap["stash"], stash.sha.as_str());
        assert_eq!(snap["paths"], json!(["a.rs", "new dir/d.rs"]));
        assert_eq!(snap["from_session"], "d");
        let l = get_leftover(s.conn(), id).unwrap().unwrap();
        assert_eq!(l.state, LeftoverState::Resolved);
        assert_eq!(l.stash_ref.as_deref(), Some(stash.sha.as_str()));
        assert_eq!(l.resolved_by, Some(ResolvedBy::Agent));
    }
    // The stash brings every file back byte for byte.
    sh(wt, &["stash", "apply", &stash.sha]);
    assert_eq!(std::fs::read(wt.join("a.rs")).unwrap(), b"a work\n");
    assert_eq!(
        std::fs::read(wt.join("new dir/d.rs")).unwrap(),
        b"d untracked\n"
    );
}

#[test]
fn adopt_commits_only_those_paths() {
    let tmp = repo();
    let wt = tmp.path();
    let mut s = scenario(wt);
    let out = analyse(&mut s, "r", "me", wt).unwrap();
    let b = by_path(&out.found, "b.rs");
    let snaps = tempfile::tempdir().unwrap();
    let res = resolve(
        &mut s,
        "r",
        snaps.path(),
        std::slice::from_ref(&b.leftover_id),
        LeftoverAction::Adopt,
        &agent("me"),
    )
    .unwrap();
    let sha = res.commit.unwrap();
    assert_eq!(sh(wt, &["show", "--name-only", "--format=", &sha]), "b.rs");
    assert_eq!(
        crate::git::trailers(wt, &sha).unwrap(),
        vec![("Bridge-Task".to_string(), "1.2".to_string())]
    );
    assert!(
        sh(wt, &["log", "-1", "--format=%s"]).starts_with("wip(bridge): adopt leftovers from d")
    );
    assert_eq!(
        std::fs::read_to_string(wt.join("a.rs")).unwrap(),
        "a work\n"
    );
}

#[test]
fn resume_claims_from_the_same_worktree_only() {
    let tmp = repo();
    let wt = tmp.path();
    let mut s = scenario(wt);
    let out = analyse(&mut s, "r", "me", wt).unwrap();
    let a = by_path(&out.found, "a.rs");
    let elsewhere = tempfile::tempdir().unwrap();
    start(&mut s, "far", elsewhere.path(), 70);
    let snaps = tempfile::tempdir().unwrap();
    let id = std::slice::from_ref(&a.leftover_id);
    let e = resolve(
        &mut s,
        "r",
        snaps.path(),
        id,
        LeftoverAction::Resume,
        &agent("far"),
    )
    .unwrap_err();
    assert!(e.to_string().starts_with("resume it from "), "{e}");
    let res = resolve(
        &mut s,
        "r",
        snaps.path(),
        id,
        LeftoverAction::Resume,
        &agent("me"),
    )
    .unwrap();
    assert_eq!(res.claimed.as_deref(), Some("1.1"));
    let me = session::get(s.conn(), "me").unwrap().unwrap();
    assert_eq!(me.active_task.as_deref(), Some("1.1"));
    assert_eq!(
        std::fs::read_to_string(wt.join("a.rs")).unwrap(),
        "a work\n"
    );
}

#[test]
fn owner_resolution_stands() {
    let tmp = repo();
    let wt = tmp.path();
    let mut s = scenario(wt);
    let out = analyse(&mut s, "r", "me", wt).unwrap();
    let c = by_path(&out.found, "c.rs");
    let snaps = tempfile::tempdir().unwrap();
    // Prefixes work for the owner's CLI.
    let prefix = c.leftover_id[..5].to_string();
    let others: Vec<&Leftover> = out
        .found
        .iter()
        .filter(|l| l.leftover_id.starts_with(&prefix))
        .collect();
    let which = if others.len() == 1 {
        prefix
    } else {
        c.leftover_id.clone()
    };
    resolve(
        &mut s,
        "r",
        snaps.path(),
        &[which],
        LeftoverAction::Undo,
        &Caller::Owner,
    )
    .unwrap();
    let e = resolve(
        &mut s,
        "r",
        snaps.path(),
        std::slice::from_ref(&c.leftover_id),
        LeftoverAction::Adopt,
        &agent("me"),
    )
    .unwrap_err();
    assert_eq!(
        e.to_string(),
        format!(
            "leftover {} was already resolved by the owner",
            c.leftover_id
        )
    );
    let l = get_leftover(s.conn(), &c.leftover_id).unwrap().unwrap();
    assert_eq!(
        (l.resolution, l.resolved_by),
        (Some(LeftoverAction::Undo), Some(ResolvedBy::Owner))
    );
}

#[test]
fn live_worktree_is_report_only() {
    let r = repo();
    let main = r.path();
    let mut s = plan();
    let parent = tempfile::tempdir().unwrap();
    let side = parent.path().join("side");
    sh(
        main,
        &[
            "worktree",
            "add",
            "-q",
            "-b",
            "side",
            side.to_str().unwrap(),
        ],
    );
    // D died dirty in the side worktree, where B is still working.
    start(&mut s, "d", &side, 10);
    unit(&mut s, "d", "1.1", &["s.rs"], Some("on_task"), 20);
    ev(
        &mut s,
        "d",
        "session_ended",
        json!({"end_kind": "dirty"}),
        30,
    );
    start(&mut s, "b", &side, 40);
    write(&side, "s.rs", "precious\n");
    start(&mut s, "me", main, 50);
    let out = analyse(&mut s, "r", "me", main).unwrap();
    assert_eq!(out.found.len(), 1);
    assert_eq!(out.found[0].state, LeftoverState::ReportOnly);
    let snaps = tempfile::tempdir().unwrap();
    let id = std::slice::from_ref(&out.found[0].leftover_id);
    for action in [
        LeftoverAction::Undo,
        LeftoverAction::Adopt,
        LeftoverAction::Resume,
    ] {
        let e = resolve(&mut s, "r", snaps.path(), id, action, &agent("me")).unwrap_err();
        assert!(e.to_string().contains("live session b"), "{e}");
        let e = resolve(&mut s, "r", snaps.path(), id, action, &Caller::Owner).unwrap_err();
        assert!(e.to_string().contains("live session b"), "{e}");
    }
    assert_eq!(
        std::fs::read_to_string(side.join("s.rs")).unwrap(),
        "precious\n"
    );
    assert_eq!(sh(main, &["stash", "list"]), "");
    assert_eq!(sh(&side, &["status", "--porcelain"]), "?? s.rs");
    assert_eq!(sh(&side, &["log", "--format=%s"]), "init");
}

#[test]
fn removed_worktree_is_noted_without_errors() {
    let r = repo();
    let mut s = plan();
    let gone = tempfile::tempdir().unwrap();
    let gone_path = gone.path().to_path_buf();
    start(&mut s, "d", &gone_path, 10);
    ev(
        &mut s,
        "d",
        "session_ended",
        json!({"end_kind": "dirty"}),
        20,
    );
    drop(gone);
    start(&mut s, "me", r.path(), 30);
    let out = analyse(&mut s, "r", "me", r.path()).unwrap();
    assert_eq!(out.found, Vec::new());
    assert_eq!(out.removed.len(), 1);
    assert_eq!(out.removed[0].0, "d");
    assert!(
        session::get(s.conn(), "d")
            .unwrap()
            .unwrap()
            .leftovers_analysed
    );
}
