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

/// Shortly after the scenarios' events: well inside the 24-hour takeover wait.
fn soon() -> Takeover {
    Takeover::new(at(3600), 24)
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
    let out = analyse(&mut s, "r", "me", wt, &soon()).unwrap();
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
    assert_eq!(
        analyse(&mut s, "r", "me", wt, &soon()).unwrap(),
        Analysis::default()
    );
    assert_eq!(leftovers(s.conn(), "r", true).unwrap().len(), 4);
}

#[test]
fn undo_is_reversible() {
    let tmp = repo();
    let wt = tmp.path();
    let mut s = scenario(wt);
    let out = analyse(&mut s, "r", "me", wt, &soon()).unwrap();
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
        &soon(),
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
    let out = analyse(&mut s, "r", "me", wt, &soon()).unwrap();
    let b = by_path(&out.found, "b.rs");
    let snaps = tempfile::tempdir().unwrap();
    let res = resolve(
        &mut s,
        "r",
        snaps.path(),
        std::slice::from_ref(&b.leftover_id),
        LeftoverAction::Adopt,
        &agent("me"),
        &soon(),
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
    let out = analyse(&mut s, "r", "me", wt, &soon()).unwrap();
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
        &soon(),
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
        &soon(),
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
    let out = analyse(&mut s, "r", "me", wt, &soon()).unwrap();
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
        &soon(),
    )
    .unwrap();
    let e = resolve(
        &mut s,
        "r",
        snaps.path(),
        std::slice::from_ref(&c.leftover_id),
        LeftoverAction::Adopt,
        &agent("me"),
        &soon(),
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
    let out = analyse(&mut s, "r", "me", main, &soon()).unwrap();
    assert_eq!(out.found.len(), 1);
    assert_eq!(out.found[0].state, LeftoverState::ReportOnly);
    let snaps = tempfile::tempdir().unwrap();
    let id = std::slice::from_ref(&out.found[0].leftover_id);
    for action in [
        LeftoverAction::Undo,
        LeftoverAction::Adopt,
        LeftoverAction::Resume,
    ] {
        let e = resolve(&mut s, "r", snaps.path(), id, action, &agent("me"), &soon()).unwrap_err();
        assert!(e.to_string().contains("live session b"), "{e}");
        let e = resolve(
            &mut s,
            "r",
            snaps.path(),
            id,
            action,
            &Caller::Owner,
            &soon(),
        )
        .unwrap_err();
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
    let out = analyse(&mut s, "r", "me", r.path(), &soon()).unwrap();
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

/// D worked in a side worktree, left s.rs uncommitted and went silent at 24 s; at 3600 s
/// `me` starts in the main worktree and the scan marks D crashed. Returns (repo, parent
/// of side, side, store).
fn crashed_elsewhere() -> (
    tempfile::TempDir,
    tempfile::TempDir,
    std::path::PathBuf,
    Store,
) {
    let r = repo();
    let mut s = plan();
    let parent = tempfile::tempdir().unwrap();
    let side = parent.path().join("side");
    sh(
        r.path(),
        &[
            "worktree",
            "add",
            "-q",
            "-b",
            "side",
            side.to_str().unwrap(),
        ],
    );
    start(&mut s, "d", &side, 10);
    unit(&mut s, "d", "1.1", &["s.rs"], Some("on_task"), 20);
    write(&side, "s.rs", "precious\n");
    start(&mut s, "me", r.path(), 3600);
    assert_eq!(scan_crashed(&mut s, "r", at(3600), 30).unwrap(), vec!["d"]);
    (r, parent, side, s)
}

fn untouched(main: &Path, side: &Path) {
    assert_eq!(
        std::fs::read_to_string(side.join("s.rs")).unwrap(),
        "precious\n"
    );
    assert_eq!(sh(main, &["stash", "list"]), "");
    assert_eq!(sh(side, &["log", "--format=%s"]), "init");
}

#[test]
fn crashed_session_elsewhere_is_report_only_until_the_takeover_wait() {
    let (r, _parent, side, mut s) = crashed_elsewhere();
    let out = analyse(&mut s, "r", "me", r.path(), &soon()).unwrap();
    assert_eq!(out.found.len(), 1);
    assert_eq!(out.found[0].state, LeftoverState::ReportOnly);
    let id = std::slice::from_ref(&out.found[0].leftover_id);
    let snaps = tempfile::tempdir().unwrap();
    for action in [LeftoverAction::Undo, LeftoverAction::Adopt] {
        let e = resolve(&mut s, "r", snaps.path(), id, action, &agent("me"), &soon()).unwrap_err();
        assert!(e.to_string().contains("crashed"), "{e}");
    }
    untouched(r.path(), &side);
    // A minute short of a day of silence: still hands-off.
    let almost = Takeover::new(at(24 + 24 * 3600 - 60), 24);
    resolve(
        &mut s,
        "r",
        snaps.path(),
        id,
        LeftoverAction::Undo,
        &agent("me"),
        &almost,
    )
    .unwrap_err();
    untouched(r.path(), &side);
    // A day after its last event, an agent may take over.
    let day = Takeover::new(at(24 + 24 * 3600), 24);
    let res = resolve(
        &mut s,
        "r",
        snaps.path(),
        id,
        LeftoverAction::Undo,
        &agent("me"),
        &day,
    )
    .unwrap();
    assert!(res.stash.is_some());
    assert!(!side.join("s.rs").exists());
}

#[test]
fn owner_may_resolve_a_crashed_sessions_leftovers_at_once() {
    let (r, _parent, side, mut s) = crashed_elsewhere();
    let out = analyse(&mut s, "r", "me", r.path(), &soon()).unwrap();
    let id = std::slice::from_ref(&out.found[0].leftover_id);
    let snaps = tempfile::tempdir().unwrap();
    resolve(
        &mut s,
        "r",
        snaps.path(),
        id,
        LeftoverAction::Undo,
        &Caller::Owner,
        &soon(),
    )
    .unwrap();
    assert!(!side.join("s.rs").exists());
}

#[test]
fn crashed_session_in_my_own_worktree_stays_resolvable() {
    let (_r, _parent, side, mut s) = crashed_elsewhere();
    start(&mut s, "me2", &side, 3601);
    let out = analyse(&mut s, "r", "me2", &side, &soon()).unwrap();
    assert_eq!(out.found.len(), 1);
    assert_eq!(out.found[0].state, LeftoverState::Open);
    let id = std::slice::from_ref(&out.found[0].leftover_id);
    let snaps = tempfile::tempdir().unwrap();
    resolve(
        &mut s,
        "r",
        snaps.path(),
        id,
        LeftoverAction::Undo,
        &agent("me2"),
        &soon(),
    )
    .unwrap();
    assert!(!side.join("s.rs").exists());
}

#[test]
fn a_crashed_session_that_speaks_again_is_live_and_its_leftovers_are_withdrawn() {
    let (r, _parent, side, mut s) = crashed_elsewhere();
    let out = analyse(&mut s, "r", "me", r.path(), &soon()).unwrap();
    let id = out.found[0].leftover_id.clone();
    // Its owner was only away: a long tool call finishes.
    ev(
        &mut s,
        "d",
        "file_changed",
        json!({"tool": "Bash", "files": []}),
        3700,
    );
    let d = session::get(s.conn(), "d").unwrap().unwrap();
    assert_ne!(d.state, session::SessionState::Ended);
    assert_eq!((d.end_kind, d.ended_at), (None, None));
    assert_eq!(leftovers(s.conn(), "r", true).unwrap(), Vec::new());
    let day = Takeover::new(at(48 * 3600), 24);
    let snaps = tempfile::tempdir().unwrap();
    for caller in [agent("me"), Caller::Owner] {
        resolve(
            &mut s,
            "r",
            snaps.path(),
            std::slice::from_ref(&id),
            LeftoverAction::Undo,
            &caller,
            &day,
        )
        .unwrap_err();
    }
    untouched(r.path(), &side);
    // The views rebuild to the same state.
    s.rebuild().unwrap();
    assert_eq!(leftovers(s.conn(), "r", true).unwrap(), Vec::new());
    assert_eq!(session::get(s.conn(), "d").unwrap().unwrap().end_kind, None);
}

#[test]
fn agent_never_touches_a_worktree_a_live_session_took_over_after_analysis() {
    let r = repo();
    let mut s = plan();
    let parent = tempfile::tempdir().unwrap();
    let side = parent.path().join("side");
    sh(
        r.path(),
        &[
            "worktree",
            "add",
            "-q",
            "-b",
            "side",
            side.to_str().unwrap(),
        ],
    );
    start(&mut s, "d", &side, 10);
    unit(&mut s, "d", "1.1", &["s.rs"], Some("on_task"), 20);
    ev(
        &mut s,
        "d",
        "session_ended",
        json!({"end_kind": "dirty"}),
        30,
    );
    write(&side, "s.rs", "precious\n");
    start(&mut s, "me", r.path(), 40);
    let out = analyse(&mut s, "r", "me", r.path(), &soon()).unwrap();
    assert_eq!(out.found[0].state, LeftoverState::Open);
    // Then b starts working in that worktree.
    start(&mut s, "b", &side, 50);
    let id = std::slice::from_ref(&out.found[0].leftover_id);
    let snaps = tempfile::tempdir().unwrap();
    for action in [LeftoverAction::Undo, LeftoverAction::Adopt] {
        let e = resolve(&mut s, "r", snaps.path(), id, action, &agent("me"), &soon()).unwrap_err();
        assert!(e.to_string().contains("live session b"), "{e}");
    }
    untouched(r.path(), &side);
}

#[test]
fn a_crash_cut_edit_is_attributed_by_its_intent() {
    let tmp = repo();
    let wt = tmp.path();
    let mut s = plan();
    start(&mut s, "d", wt, 10);
    unit(&mut s, "d", "1.1", &["a.rs"], Some("on_task"), 20);
    // The next unit announced an edit to x.rs and died before PostToolUse.
    ev(&mut s, "d", "prompt", json!({"chars": 1}), 40);
    ev(
        &mut s,
        "d",
        "tool_intent",
        json!({"tool": "Write", "files": ["x.rs"]}),
        41,
    );
    write(wt, "x.rs", "half written\n");
    start(&mut s, "me", wt, 3600);
    scan_crashed(&mut s, "r", at(3600), 30).unwrap();
    let out = analyse(&mut s, "r", "me", wt, &soon()).unwrap();
    let x = by_path(&out.found, "x.rs");
    assert_eq!(
        (x.task_id.as_deref(), x.suggested),
        (Some("1.1"), LeftoverAction::Resume)
    );
}

#[test]
fn files_of_an_abandoned_detour_suggest_undo() {
    let tmp = repo();
    let wt = tmp.path();
    let mut s = plan();
    start(&mut s, "d", wt, 10);
    ev(
        &mut s,
        "d",
        "task_claimed",
        json!({"task_id": "1.1", "mode": "auto", "confidence": 0.9}),
        11,
    );
    ev(
        &mut s,
        "d",
        "detour_opened",
        json!({"detour_id": "D1", "reason": "try a new lib", "blocks": "1.1", "resume_cue": "back to the form"}),
        12,
    );
    ev(&mut s, "d", "prompt", json!({"chars": 1}), 13);
    ev(
        &mut s,
        "d",
        "file_changed",
        json!({"tool": "Edit", "files": ["e.rs"]}),
        14,
    );
    ev(&mut s, "d", "unit_stopped", json!({}), 15);
    ev(
        &mut s,
        "d",
        "detour_closed",
        json!({"detour_id": "D1", "outcome": "abandoned"}),
        16,
    );
    ev(
        &mut s,
        "d",
        "session_ended",
        json!({"end_kind": "dirty"}),
        17,
    );
    write(wt, "e.rs", "experiment\n");
    start(&mut s, "me", wt, 20);
    let out = analyse(&mut s, "r", "me", wt, &soon()).unwrap();
    assert_eq!(by_path(&out.found, "e.rs").suggested, LeftoverAction::Undo);
}

#[test]
fn a_file_two_dead_sessions_left_is_reported_once() {
    let tmp = repo();
    let wt = tmp.path();
    let mut s = plan();
    for (d, t) in [("d1", 10), ("d2", 100)] {
        start(&mut s, d, wt, t);
        unit(&mut s, d, "1.1", &["a.rs"], Some("on_task"), t + 1);
        ev(
            &mut s,
            d,
            "session_ended",
            json!({"end_kind": "dirty"}),
            t + 10,
        );
    }
    write(wt, "a.rs", "work\n");
    start(&mut s, "me", wt, 200);
    let out = analyse(&mut s, "r", "me", wt, &soon()).unwrap();
    assert_eq!(out.found.len(), 1, "{out:?}");
    assert_eq!(leftovers(s.conn(), "r", true).unwrap().len(), 1);
}
