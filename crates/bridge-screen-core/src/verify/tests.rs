use jiff::{SignedDuration, Timestamp};
use serde_json::{Value, json};

use super::*;
use crate::plan::{NodeStatus, get_node};
use crate::store::{EventDraft, Store, append_in_at};

fn t0() -> Timestamp {
    "2026-10-14T09:00:00Z".parse().unwrap()
}

/// Append events at scripted times (seconds after `t0`).
fn script(s: &mut Store, events: &[(&str, &str, i64, &str, Value)]) {
    s.write(|tx| {
        for (repo, session, secs, kind, data) in events {
            let draft = EventDraft::new(*repo, *session, *kind, data.clone());
            append_in_at(tx, draft, t0() + SignedDuration::from_secs(*secs))?;
        }
        Ok(())
    })
    .unwrap();
}

fn node(id: &str, parent: Option<&str>, kind: &str, estimate: Option<u32>) -> Value {
    json!({"id": id, "parent_id": parent, "kind": kind, "title": format!("T{id}"),
           "intent": "why", "estimate_min": estimate, "expected_files": []})
}

fn store() -> Store {
    let mut s = Store::open_in_memory().unwrap();
    let mut events = Vec::new();
    for repo in ["r", "other"] {
        for (id, parent, kind, est) in [
            ("G", None, "goal", None),
            ("1", Some("G"), "milestone", None),
            ("1.1", Some("1"), "task", Some(30)),
            ("1.2", Some("1"), "task", Some(10)),
            ("1.3", Some("1"), "task", None),
            ("1.4", Some("1"), "task", Some(60)),
            ("1.4.1", Some("1.4"), "subtask", Some(20)),
            ("1.4.2", Some("1.4"), "subtask", Some(40)),
        ] {
            events.push((repo, "owner", 0, "node_added", node(id, parent, kind, est)));
        }
    }
    events.push((
        "r",
        "s1",
        1,
        "session_started",
        json!({"cwd": "/w", "source": "startup"}),
    ));
    script(&mut s, &events);
    s
}

fn status(s: &Store, repo: &str, id: &str) -> NodeStatus {
    get_node(s.conn(), repo, id).unwrap().unwrap().status
}

fn verified(id: &str, task: &str, level: i64, sha: &str, files: &[&str]) -> Value {
    json!({"verif_id": id, "task_id": task, "level": level, "sha": sha, "files": files,
           "evidence": "ok", "by": "agent"})
}

#[test]
fn status_follows_the_ladder() {
    let mut s = store();
    let steps: Vec<(&str, Value, NodeStatus)> = vec![
        (
            "task_claimed",
            json!({"task_id": "1.1", "mode": "auto", "confidence": 0.9}),
            NodeStatus::Active,
        ),
        (
            "claimed_done",
            json!({"task_id": "1.1", "evidence": "done"}),
            NodeStatus::Claimed,
        ),
        (
            "verified",
            verified("v1", "1.1", 1, "a", &["src/a.rs"]),
            NodeStatus::L1,
        ),
        (
            "verified",
            verified("v2", "1.1", 2, "a", &["src/a.rs"]),
            NodeStatus::L2,
        ),
        (
            "verification_failed",
            json!({"task_id": "1.1", "level": 2, "evidence": "boom", "by": "rerun",
                   "rerun": "failed"}),
            NodeStatus::Failed,
        ),
        (
            "task_claimed",
            json!({"task_id": "1.1", "mode": "auto", "confidence": 0.9}),
            NodeStatus::Active,
        ),
        (
            "claimed_done",
            json!({"task_id": "1.1", "evidence": "fixed"}),
            NodeStatus::Claimed,
        ),
        (
            "verified",
            verified("v3", "1.1", 2, "b", &["src/a.rs"]),
            NodeStatus::L2,
        ),
        // A lower level later doesn't lower the status.
        (
            "verified",
            verified("v4", "1.1", 1, "c", &["src/a.rs"]),
            NodeStatus::L2,
        ),
    ];
    for (i, (kind, data, want)) in steps.into_iter().enumerate() {
        script(
            &mut s,
            &[("r", "s1", 10 + i64::try_from(i).unwrap(), kind, data)],
        );
        assert_eq!(status(&s, "r", "1.1"), want, "step {i}: {kind}");
    }
    let rows = verifications(s.conn(), "r", "1.1").unwrap();
    let states: Vec<VerifState> = rows.iter().map(|v| v.state).collect();
    assert_eq!(
        states,
        vec![
            VerifState::Failed,
            VerifState::Failed,
            VerifState::Current,
            VerifState::Current
        ]
    );
    assert_eq!(rows[2].stale_checked_sha, "b");
    assert_eq!(rows[2].session_id, "s1");
    assert_eq!(current_level(s.conn(), "r", "1.1").unwrap(), 2);
    // The other repo's task of the same id is untouched.
    assert_eq!(status(&s, "other", "1.1"), NodeStatus::Todo);
}

#[test]
fn stale_drops_one_level_and_the_status_follows() {
    let mut s = store();
    script(
        &mut s,
        &[
            (
                "r",
                "s1",
                5,
                "claimed_done",
                json!({"task_id": "1.2", "evidence": "x", "files": ["a.rs"]}),
            ),
            (
                "r",
                "s1",
                6,
                "verified",
                verified("v1", "1.2", 2, "a", &["a.rs"]),
            ),
            (
                "r",
                "s1",
                7,
                "verification_stale",
                json!({"verif_id": "v1", "task_id": "1.2", "from_level": 2, "to_level": 1,
                       "head_sha": "b", "changed": ["a.rs"]}),
            ),
        ],
    );
    assert_eq!(status(&s, "r", "1.2"), NodeStatus::L1);
    let v = &verifications(s.conn(), "r", "1.2").unwrap()[0];
    assert_eq!(
        (v.level, v.orig_level, v.state, v.stale_checked_sha.as_str()),
        (1, 2, VerifState::Stale, "b")
    );
    script(
        &mut s,
        &[(
            "r",
            "s1",
            8,
            "verification_stale",
            json!({"verif_id": "v1", "task_id": "1.2", "from_level": 1, "to_level": 0,
                   "head_sha": "c", "changed": ["a.rs"]}),
        )],
    );
    // A row at level 0 counts for nothing: back to claimed.
    assert_eq!(status(&s, "r", "1.2"), NodeStatus::Claimed);
    assert_eq!(top_verification(s.conn(), "r", "1.2").unwrap(), None);
    let claim = crate::handoff::latest_claim(s.conn(), "r", "1.2")
        .unwrap()
        .unwrap();
    assert_eq!(claim.files, vec!["a.rs"]);
}

#[test]
fn a_late_stale_drop_never_undoes_a_failure() {
    // A SessionStart scan read v1 as current, then the re-run failed before its drop
    // was written: the drop changes nothing and the task stays failed.
    let mut s = store();
    script(
        &mut s,
        &[
            (
                "r",
                "s1",
                5,
                "claimed_done",
                json!({"task_id": "1.2", "evidence": "x", "files": ["a.rs"]}),
            ),
            (
                "r",
                "s1",
                6,
                "verified",
                verified("v1", "1.2", 2, "a", &["a.rs"]),
            ),
            (
                "r",
                "s1",
                7,
                "verification_failed",
                json!({"task_id": "1.2", "level": 2, "evidence": "boom", "by": "rerun"}),
            ),
            (
                "r",
                "s2",
                8,
                "verification_stale",
                json!({"verif_id": "v1", "task_id": "1.2", "from_level": 2, "to_level": 1,
                       "head_sha": "b", "changed": ["a.rs"]}),
            ),
        ],
    );
    assert_eq!(status(&s, "r", "1.2"), NodeStatus::Failed);
    let v = &verifications(s.conn(), "r", "1.2").unwrap()[0];
    assert_eq!((v.level, v.state), (2, VerifState::Failed));
}

#[test]
fn dropped_nodes_are_never_touched() {
    let mut s = store();
    script(
        &mut s,
        &[
            ("r", "owner", 5, "node_removed", json!({"id": "1.2"})),
            ("r", "s1", 6, "verified", verified("v1", "1.2", 2, "a", &[])),
        ],
    );
    assert_eq!(status(&s, "r", "1.2"), NodeStatus::Dropped);
}

#[test]
fn squash_repoints_only_matching_shas_of_the_same_repo() {
    let mut s = store();
    script(
        &mut s,
        &[
            (
                "r",
                "s1",
                5,
                "verified",
                verified("v1", "1.1", 2, "w1", &[]),
            ),
            (
                "r",
                "s1",
                6,
                "verified",
                verified("v2", "1.2", 2, "w2", &[]),
            ),
            (
                "r",
                "s1",
                7,
                "verified",
                verified("v3", "1.3", 1, "keep", &[]),
            ),
            (
                "other",
                "s9",
                8,
                "verified",
                verified("v4", "1.1", 2, "w1", &[]),
            ),
            (
                "r",
                "s1",
                9,
                "squashed",
                json!({"branch": "feat", "old_shas": ["w1", "w2"], "new_sha": "sq",
                       "backup_ref": "refs/bridge-screen/backup/feat/1"}),
            ),
        ],
    );
    let sha = |repo: &str, task: &str| {
        let v = &verifications(s.conn(), repo, task).unwrap()[0];
        (v.sha.clone(), v.stale_checked_sha.clone())
    };
    assert_eq!(sha("r", "1.1"), ("sq".into(), "sq".into()));
    assert_eq!(sha("r", "1.2"), ("sq".into(), "sq".into()));
    assert_eq!(sha("r", "1.3"), ("keep".into(), "keep".into()));
    assert_eq!(sha("other", "1.1"), ("w1".into(), "w1".into()));
}

#[test]
fn test_settings_land_in_repo_settings_and_unset_clears() {
    let mut s = store();
    let change =
        |key: &str, new: Value| json!({"scope": "repo", "key": key, "old": null, "new": new});
    script(
        &mut s,
        &[
            (
                "r",
                "owner",
                5,
                "config_changed",
                change("test_command", json!("cargo test")),
            ),
            (
                "r",
                "owner",
                6,
                "config_changed",
                change("test_timeout_s", json!(300)),
            ),
            // The drift keys keep working alongside.
            (
                "r",
                "owner",
                7,
                "config_changed",
                json!({"scope": "repo", "key": "drift_lower", "old": 0.3, "new": 0.35,
                       "samples": 20}),
            ),
        ],
    );
    assert_eq!(
        settings(s.conn(), "r").unwrap(),
        RepoSettings {
            test_command: Some("cargo test".into()),
            test_timeout_s: Some(300)
        }
    );
    assert_eq!(
        settings(s.conn(), "other").unwrap(),
        RepoSettings::default()
    );
    script(
        &mut s,
        &[(
            "r",
            "owner",
            8,
            "config_changed",
            change("test_command", Value::Null),
        )],
    );
    assert_eq!(settings(s.conn(), "r").unwrap().test_command, None);
    assert_eq!(settings(s.conn(), "r").unwrap().test_timeout_s, Some(300));
    let lower: Option<f64> = s
        .conn()
        .query_row(
            "SELECT drift_lower FROM repo_tuning WHERE repo_id = 'r'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(lower, Some(0.35));
}

#[test]
fn progress_counts_l2_up_as_done_and_claimed_apart() {
    let mut s = store();
    // Leaves: 1.1 (30), 1.2 (10), 1.3 (none), 1.4.1 (20), 1.4.2 (40). 1.4 is a parent.
    assert_eq!(
        progress(s.conn(), "r").unwrap(),
        Progress {
            done_pct: 0,
            claimed_pct: 0,
            done_min: 0,
            claimed_min: 0,
            total_min: 100,
            unestimated: 1
        }
    );
    script(
        &mut s,
        &[
            (
                "r",
                "s1",
                5,
                "claimed_done",
                json!({"task_id": "1.1", "evidence": "x"}),
            ),
            (
                "r",
                "s1",
                6,
                "claimed_done",
                json!({"task_id": "1.2", "evidence": "x"}),
            ),
            ("r", "s1", 7, "verified", verified("v1", "1.2", 1, "a", &[])),
            (
                "r",
                "s1",
                8,
                "verified",
                verified("v2", "1.4.1", 3, "a", &[]),
            ),
            ("r", "s1", 9, "verified", verified("v3", "1.4", 4, "a", &[])),
            (
                "r",
                "s1",
                10,
                "verified",
                verified("v4", "1.3", 2, "a", &[]),
            ),
        ],
    );
    let p = progress(s.conn(), "r").unwrap();
    assert_eq!(
        (p.done_min, p.claimed_min, p.total_min, p.unestimated),
        (20, 40, 100, 1)
    );
    assert_eq!((p.done_pct, p.claimed_pct), (20, 40));
    // Dropping a leaf takes it out of both sums.
    script(
        &mut s,
        &[("r", "owner", 11, "node_removed", json!({"id": "1.4.2"}))],
    );
    let p = progress(s.conn(), "r").unwrap();
    assert_eq!((p.done_min, p.total_min, p.done_pct), (20, 60, 33));
    assert_eq!(progress(s.conn(), "nope").unwrap(), Progress::default());
}

#[test]
fn nudges_count_and_clear_on_l2_or_failure() {
    let mut s = store();
    let nudges = |s: &Store, task: &str| -> Option<(i64, i64, i64)> {
        s.conn()
            .query_row(
                "SELECT nudges, level_at_first, escalated FROM verify_nudges
                 WHERE repo_id = 'r' AND task_id = ?1",
                [task],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
            )
            .ok()
    };
    script(
        &mut s,
        &[
            (
                "r",
                "s1",
                5,
                "verify_nudged",
                json!({"task_id": "1.1", "level": 0}),
            ),
            (
                "r",
                "s1",
                6,
                "verify_nudged",
                json!({"task_id": "1.1", "level": 1}),
            ),
            (
                "r",
                "s1",
                7,
                "needs_owner",
                json!({"kind": "verify", "task_id": "1.1"}),
            ),
        ],
    );
    // The level moved between the nudges: the count starts again at the new level.
    assert_eq!(nudges(&s, "1.1"), Some((1, 1, 1)));
    // Not a wait: the session's state is unchanged.
    let session = crate::session::get(s.conn(), "s1").unwrap().unwrap();
    assert_eq!(session.state, crate::session::SessionState::Idle);
    script(
        &mut s,
        &[("r", "s1", 8, "verified", verified("v1", "1.1", 1, "a", &[]))],
    );
    assert_eq!(nudges(&s, "1.1"), Some((1, 1, 1)), "L1 doesn't answer");
    script(
        &mut s,
        &[("r", "s1", 9, "verified", verified("v2", "1.1", 2, "a", &[]))],
    );
    assert_eq!(nudges(&s, "1.1"), None);
    script(
        &mut s,
        &[
            (
                "r",
                "s1",
                10,
                "verify_nudged",
                json!({"task_id": "1.2", "level": 0}),
            ),
            (
                "r",
                "s1",
                11,
                "verification_failed",
                json!({"task_id": "1.2", "level": 4, "evidence": "broken", "by": "owner"}),
            ),
        ],
    );
    assert_eq!(nudges(&s, "1.2"), None);
}

#[test]
fn ignored_nudges_escalate_once_the_level_stops_moving() {
    let mut s = store();
    let nudge = |s: &mut Store, secs: i64, level: i64| -> Vec<String> {
        let drafts = nudge::nudge_drafts(s.conn(), "r", "s1", "1.1", level).unwrap();
        let kinds = drafts.iter().map(|d| d.kind.clone()).collect();
        s.write(|tx| {
            for d in drafts {
                append_in_at(tx, d, t0() + SignedDuration::from_secs(secs))?;
            }
            Ok(())
        })
        .unwrap();
        kinds
    };
    assert_eq!(nudge(&mut s, 5, 0), vec!["verify_nudged"]);
    // It reached L1 after the first nudge: that one wasn't ignored.
    assert_eq!(nudge(&mut s, 6, 1), vec!["verify_nudged"]);
    // Two nudges at L1 without a move: the owner is asked.
    assert_eq!(nudge(&mut s, 7, 1), vec!["verify_nudged", "needs_owner"]);
    assert_eq!(nudge(&mut s, 8, 1), vec!["verify_nudged"], "asked once");
    assert_eq!(
        nudge::overdue(s.conn(), "r").unwrap(),
        vec![("1.1".to_string(), "T1.1".to_string())]
    );
}

mod rerun_tests {
    use std::time::{Duration, Instant};

    use crate::verify::rerun::{RerunOutcome, last_line, run};

    #[test]
    fn a_command_that_cannot_run_is_not_a_test_failure() {
        let dir = tempfile::tempdir().unwrap();
        let out = run(
            dir.path(),
            "bridge-screen-no-such-command --all",
            Duration::from_secs(10),
        );
        assert!(
            matches!(&out, RerunOutcome::CannotRun { code: 127, last_line }
                if last_line.contains("not found")),
            "{out:?}"
        );
        // Present but not executable.
        std::fs::write(dir.path().join("tests.sh"), "echo hi\n").unwrap();
        let out = run(dir.path(), "./tests.sh", Duration::from_secs(10));
        assert!(
            matches!(&out, RerunOutcome::CannotRun { code: 126, .. }),
            "{out:?}"
        );
    }

    #[test]
    fn passes_and_fails_with_the_last_line() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("marker"), "x").unwrap();
        // Runs in the worktree, stdin closed.
        let out = run(
            dir.path(),
            "test -f marker && cat; echo '12 passed'; echo '  '",
            Duration::from_secs(10),
        );
        assert!(
            matches!(&out, RerunOutcome::Passed { last_line, .. } if last_line == "12 passed"),
            "{out:?}"
        );
        let out = run(
            dir.path(),
            "echo ok; echo 'test login ... FAILED' >&2; exit 3",
            Duration::from_secs(10),
        );
        assert!(
            matches!(&out, RerunOutcome::Failed { code: 3, last_line, .. }
                if last_line == "test login ... FAILED"),
            "{out:?}"
        );
        let long = "x".repeat(500);
        assert_eq!(last_line(long.as_bytes()).chars().count(), 200);
        assert_eq!(last_line(b""), "");
    }

    #[test]
    fn times_out_and_returns_promptly() {
        let dir = tempfile::tempdir().unwrap();
        let start = Instant::now();
        let out = run(dir.path(), "sleep 30", Duration::from_secs(1));
        assert!(matches!(out, RerunOutcome::TimedOut { secs: 1 }), "{out:?}");
        assert!(
            start.elapsed() < Duration::from_secs(4),
            "{:?}",
            start.elapsed()
        );
    }

    #[cfg(unix)]
    fn alive(pid: &str) -> bool {
        std::process::Command::new("kill")
            .args(["-0", pid])
            .stderr(std::process::Stdio::null())
            .status()
            .unwrap()
            .success()
    }

    /// The test command forks a background child that would outlive it: the whole
    /// process group is stopped at the time limit, and the call returns.
    #[cfg(unix)]
    #[test]
    fn rerun_timeout_kills_process_group() {
        let dir = tempfile::tempdir().unwrap();
        let pidfile = dir.path().join("bg.pid");
        let cmd = format!("sleep 60 & echo $! > '{}'; sleep 30", pidfile.display());
        let start = Instant::now();
        let out = run(dir.path(), &cmd, Duration::from_secs(1));
        assert!(matches!(out, RerunOutcome::TimedOut { .. }), "{out:?}");
        assert!(
            start.elapsed() < Duration::from_secs(6),
            "{:?}",
            start.elapsed()
        );
        let pid = std::fs::read_to_string(&pidfile)
            .unwrap()
            .trim()
            .to_string();
        // The orphan may linger as a zombie for a moment before it is reaped.
        let deadline = Instant::now() + Duration::from_secs(3);
        while alive(&pid) && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(50));
        }
        assert!(
            !alive(&pid),
            "background child {pid} survived the time limit"
        );
    }

    /// A background child that ignores TERM and doesn't hold the output open is still
    /// stopped at the time limit, though the shell itself exits on TERM.
    #[cfg(unix)]
    #[test]
    fn rerun_timeout_kills_a_term_proof_child_without_the_pipe() {
        let dir = tempfile::tempdir().unwrap();
        let pidfile = dir.path().join("bg.pid");
        let cmd = format!(
            "(trap '' TERM; exec sleep 60) >/dev/null 2>&1 & echo $! > '{}'; sleep 30",
            pidfile.display()
        );
        let out = run(dir.path(), &cmd, Duration::from_secs(1));
        assert!(matches!(out, RerunOutcome::TimedOut { .. }), "{out:?}");
        let pid = std::fs::read_to_string(&pidfile)
            .unwrap()
            .trim()
            .to_string();
        let deadline = Instant::now() + Duration::from_secs(3);
        while alive(&pid) && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(50));
        }
        let survived = alive(&pid);
        if survived {
            let _ = std::process::Command::new("kill")
                .args(["-9", &pid])
                .status();
        }
        assert!(!survived, "background child {pid} survived the time limit");
    }

    /// A command that exits while a background child still holds its output open
    /// doesn't hang the re-run.
    #[cfg(unix)]
    #[test]
    fn a_leftover_background_child_does_not_hang_a_finished_run() {
        let dir = tempfile::tempdir().unwrap();
        let start = Instant::now();
        let out = run(dir.path(), "sleep 30 & echo done", Duration::from_secs(20));
        assert!(
            matches!(&out, RerunOutcome::Passed { last_line, .. } if last_line == "done"),
            "{out:?}"
        );
        assert!(
            start.elapsed() < Duration::from_secs(5),
            "{:?}",
            start.elapsed()
        );
    }

    #[test]
    fn output_larger_than_a_pipe_does_not_deadlock() {
        let dir = tempfile::tempdir().unwrap();
        let out = run(
            dir.path(),
            "i=0; while [ $i -lt 20000 ]; do echo \"line $i of output that fills the pipe\"; echo err $i >&2; i=$((i+1)); done; echo last",
            Duration::from_secs(60),
        );
        assert!(
            matches!(&out, RerunOutcome::Passed { last_line, .. } if last_line == "last"),
            "{out:?}"
        );
    }
}
