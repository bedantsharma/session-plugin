//! The History panel in plain sentences (spec §10.2). Built here, in the core, so the CLI
//! and the dashboard word every event the same way. The raw payload stays available
//! behind a "details" toggle; the sentences never show an internal code.

use serde_json::Value;

/// The task an event is about, for its sentence.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TaskLabel {
    /// The id as the owner reads it (`1.2`).
    pub shown: String,
    pub title: String,
}

/// `Task 1.2 (Login form)`, or `Task 1.2`, or `A task` when none is known.
fn task_text(task: Option<&TaskLabel>) -> String {
    match task {
        Some(t) if t.title.is_empty() => format!("Task {}", t.shown),
        Some(t) => format!("Task {} ({})", t.shown, t.title),
        None => "A task".to_string(),
    }
}

fn str_of<'a>(d: &'a Value, key: &str) -> Option<&'a str> {
    d.get(key).and_then(Value::as_str).filter(|s| !s.is_empty())
}

fn short(id: &str) -> String {
    id.chars().take(6).collect()
}

/// A sentence fragment of at most `max` characters.
fn clip(text: &str, max: usize) -> String {
    let one: String = text.split_whitespace().collect::<Vec<_>>().join(" ");
    if one.chars().count() <= max {
        return one;
    }
    let cut: String = one.chars().take(max.saturating_sub(1)).collect();
    format!("{}…", cut.trim_end())
}

/// `snake_case_words` as `snake case words`.
fn words(code: &str) -> String {
    code.replace(['_', '-'], " ")
}

fn sentence_case(text: &str) -> String {
    let mut c = text.chars();
    c.next()
        .map(|f| f.to_uppercase().collect::<String>() + c.as_str())
        .unwrap_or_default()
}

fn count(n: u64, one: &str, many: &str) -> String {
    format!("{n} {}", if n == 1 { one } else { many })
}

/// How a verification level reads, as a past-tense phrase after the task.
fn level_phrase(level: i64, by: &str) -> &'static str {
    match level {
        1 => "has its code in place (tests not run)",
        2 => "passed its tests",
        3 => "was tried end to end",
        _ if by == "owner" => "was signed off by you",
        _ => "was checked by hand",
    }
}

fn label_words(label: &str) -> &'static str {
    match label {
        "on_task" => "on task",
        "detour" => "a side trip",
        "drift" => "off task",
        "requested" => "work you asked for",
        _ => "labelled",
    }
}

fn nav_words(state: &str) -> &'static str {
    match state {
        "green" => "is back on track",
        "amber" => "is drifting a little",
        "red" => "is far off its task",
        _ => "changed course",
    }
}

fn owner_action_words(d: &Value) -> String {
    let answer = str_of(d, "answer").unwrap_or("");
    match str_of(d, "action").unwrap_or("") {
        "drift" => match answer {
            "continue" => "You told a session to carry on".to_string(),
            "make_task" => "You turned a side trip into a task".to_string(),
            "stop" => "You told a session to stop its side trip".to_string(),
            _ => "You answered a drift question".to_string(),
        },
        "label" => "You relabelled a turn".to_string(),
        "resolve" => match answer {
            "adopt" => "You kept the unsaved files (committed them)".to_string(),
            "resume" => "You chose to carry on with the unsaved files".to_string(),
            "undo" => "You put the unsaved files aside".to_string(),
            _ => "You decided what to do with unsaved files".to_string(),
        },
        "verify" => "You signed a task off as working".to_string(),
        "broken" => "You marked a task as broken".to_string(),
        "withdraw" => "You took back a sign-off".to_string(),
        "reset_drift" => "You reset the drift settings to the defaults".to_string(),
        "clear_wait" => "You cleared a wait by hand".to_string(),
        "promote_wait" => "You turned a wait into a plan link".to_string(),
        "plan_scope" => match answer {
            "fresh" => "You chose to count progress from now".to_string(),
            _ => "You said which plan files are already finished".to_string(),
        },
        "close_run" => "You closed the finished run".to_string(),
        "" => "You did something".to_string(),
        other => format!("You did this: {}", words(other)),
    }
}

/// One event in a plain sentence. `task` is the task the event is about (from its task
/// id), `session` the session that recorded it.
#[allow(clippy::too_many_lines)] // one arm per kind of event
pub fn summarize(kind: &str, d: &Value, session: &str, task: Option<&TaskLabel>) -> String {
    let who = format!("session {}", short(session));
    let who_cap = sentence_case(&who);
    let t = task_text(task);
    match kind {
        "session_started" => match str_of(d, "source") {
            Some("resume") => format!("{who_cap} resumed"),
            Some("compact") => format!("{who_cap} carried on after a memory squeeze"),
            Some("clear") => format!("{who_cap} started fresh"),
            _ => format!("{who_cap} started"),
        },
        "session_ended" => format!("{who_cap} ended"),
        "session_crashed" => format!("{who_cap} stopped unexpectedly"),
        "prompt" => format!("You sent a message in {who}"),
        "unit_stopped" => format!("{who_cap} finished a turn"),
        "subagent_stopped" => format!("A helper agent in {who} finished"),
        "precompact" => format!("{who_cap} is making room in its memory"),
        "branch_changed" => match str_of(d, "branch") {
            Some(b) => format!("{who_cap} moved to branch {b}"),
            None => format!("{who_cap} changed branch"),
        },
        "tool_intent" | "file_changed" => {
            let n = d.get("files").and_then(Value::as_array).map_or(0, Vec::len) as u64;
            if n == 0 {
                format!("{who_cap} edited files")
            } else {
                format!("{who_cap} edited {}", count(n, "file", "files"))
            }
        }
        "needs_owner" => match str_of(d, "kind") {
            Some("permission_prompt") => format!("{who_cap} is waiting for your permission"),
            Some("idle_prompt") => format!("{who_cap} is waiting for you"),
            Some("verify") => format!("{t} needs you to check it"),
            Some("drift") => format!("{who_cap} is far off its task and needs a look"),
            _ => format!("{who_cap} needs you"),
        },
        "owner_question" => format!("{who_cap} asked you a question"),
        "question_answered" => format!("{who_cap} got an answer to its question"),
        "task_claimed" => format!("{t} claimed by {who}"),
        "task_released" => format!("{who_cap} let go of {}", t.to_lowercase_first()),
        "claimed_done" => format!(
            "{who_cap} says {} is done (not checked yet)",
            t.to_lowercase_first()
        ),
        "verified" => format!(
            "{t} {}",
            level_phrase(
                d.get("level").and_then(Value::as_i64).unwrap_or(0),
                str_of(d, "by").unwrap_or("")
            )
        ),
        "verification_failed" => match str_of(d, "evidence") {
            Some(e) => format!("{t} failed its check: {}", clip(e, 80)),
            None => format!("{t} failed its check"),
        },
        "verification_stale" => format!(
            "The check on {} is out of date: files changed since",
            t.to_lowercase_first()
        ),
        "verification_withdrawn" => {
            format!("Your sign-off on {} was taken back", t.to_lowercase_first())
        }
        "verify_nudged" => format!("{who_cap} was reminded to check {}", t.to_lowercase_first()),
        "broken" => match str_of(d, "note") {
            Some(n) => format!("{t} marked broken: {}", clip(n, 80)),
            None => format!("{t} marked broken"),
        },
        "node_added" => {
            let what = match str_of(d, "kind") {
                Some("goal") => "the goal",
                Some("milestone") => "a milestone",
                Some("subtask") => "a step",
                _ => "a task",
            };
            match str_of(d, "title") {
                Some(title) => format!("Added {what} to the plan: {}", clip(title, 80)),
                None => format!("Added {what} to the plan"),
            }
        }
        "node_edited" => format!("Edited {} in the plan", t.to_lowercase_first()),
        "node_moved" => format!("Moved {} in the plan", t.to_lowercase_first()),
        "node_removed" => format!("Removed {} from the plan", t.to_lowercase_first()),
        "dep_added" => format!("{t} now waits for another task"),
        "dep_removed" => format!("{t} no longer waits for another task"),
        "intent_set" => "Changed what a part of the plan is for".to_string(),
        "estimate_set" => match d.get("estimate_min").and_then(Value::as_u64) {
            Some(m) => format!("Estimate for {} set to {m} min", t.to_lowercase_first()),
            None => format!("Estimate for {} changed", t.to_lowercase_first()),
        },
        "hill_moved" => match str_of(d, "hill") {
            Some("uphill") => "A milestone is back to exploring the unknowns".to_string(),
            Some("downhill") => "A milestone's path is clear: now executing".to_string(),
            _ => "A milestone changed how sure it is".to_string(),
        },
        "replan" => {
            let n = d.get("op_count").and_then(Value::as_u64).unwrap_or(0);
            match str_of(d, "reason") {
                Some(r) => format!(
                    "Plan edited: {} ({})",
                    count(n, "change", "changes"),
                    clip(r, 80)
                ),
                None => format!("Plan edited: {}", count(n, "change", "changes")),
            }
        }
        "plan_imported" => {
            let n = d.get("tasks").and_then(Value::as_u64).unwrap_or(0);
            let file =
                str_of(d, "path").map_or_else(String::new, |p| format!(" {}", short_file(p)));
            format!("Read plan file{file}: {}", count(n, "task", "tasks"))
        }
        "plan_file_seen" => match str_of(d, "path") {
            Some(p) => format!("Looked at plan file {}", short_file(p)),
            None => "Looked at a plan file".to_string(),
        },
        "plan_scope_asked" => {
            let n = d.get("paths").and_then(Value::as_array).map_or(0, Vec::len) as u64;
            format!(
                "Asked you which of {} are already finished",
                count(n, "plan file", "plan files")
            )
        }
        "plan_scope_answered" => match str_of(d, "mode") {
            Some("fresh") => "You chose to count progress from now".to_string(),
            _ => "You said which plan files are already finished".to_string(),
        },
        "run_closed" => {
            let n = d
                .get("closed_ids")
                .and_then(Value::as_array)
                .map_or(0, Vec::len) as u64;
            format!(
                "A finished run was closed ({})",
                count(n, "milestone", "milestones")
            )
        }
        "detour_opened" => match str_of(d, "reason") {
            Some(r) => format!("{who_cap} went on a side trip: {}", clip(r, 80)),
            None => format!("{who_cap} went on a side trip"),
        },
        "detour_closed" => format!("{who_cap} ended a side trip"),
        "detour_promoted" => format!("A side trip in {who} became {}", t.to_lowercase_first()),
        "item_suspended" => format!("{who_cap} set a task aside for a side trip"),
        "item_resumed" => format!("{who_cap} went back to the task it set aside"),
        "unit_labeled" => format!(
            "A turn in {who} was marked {}",
            label_words(str_of(d, "label").unwrap_or(""))
        ),
        "drift_flagged" => format!("{who_cap} drifted from its task"),
        "nav_state_changed" => format!("{who_cap} {}", nav_words(str_of(d, "to").unwrap_or(""))),
        "owner_action" => owner_action_words(d),
        "note_sent" => format!("{who_cap} was nudged"),
        "config_changed" => {
            let key = str_of(d, "key").map_or_else(|| "a setting".to_string(), words);
            match (
                d.get("old").and_then(Value::as_f64),
                d.get("new").and_then(Value::as_f64),
            ) {
                (Some(o), Some(n)) => format!("Drift setting ({key}) tuned from {o} to {n}"),
                (None, Some(n)) => format!("Drift setting ({key}) set to {n}"),
                _ => format!("Drift setting ({key}) changed"),
            }
        }
        "tuning_ran" => "Drift settings were re-checked".to_string(),
        "checkpoint_due" => format!("{who_cap} was asked to save its work"),
        "checkpoint" => format!("{who_cap} saved its work"),
        "checkpoint_missed" => format!("{who_cap} didn't save its work when asked"),
        "leftover_found" => format!("Found unsaved files left behind by {who}"),
        "leftovers_analysed" => "Looked over the files a stopped session left behind".to_string(),
        "leftover_resolved" => match str_of(d, "action") {
            Some("adopt") => "Unsaved files were kept (committed)".to_string(),
            Some("resume") => "Unsaved files were handed back to carry on with".to_string(),
            Some("undo") => "Unsaved files were put aside".to_string(),
            _ => "Unsaved files were dealt with".to_string(),
        },
        "handoff_note" => format!("{who_cap} left a note for the next session"),
        "readback" => match str_of(d, "result") {
            Some("pass") => format!("{who_cap} showed it understood the handoff"),
            Some("fail") => format!("{who_cap} got part of the handoff wrong"),
            _ => format!("{who_cap} was asked to confirm the handoff"),
        },
        "briefed" => format!("{who_cap} was briefed on where things stand"),
        "squashed" => "Work-in-progress commits were tidied into one".to_string(),
        "squash_failed" => "Tidying the commits didn't work".to_string(),
        "decision" => match str_of(d, "what") {
            Some(w) => format!("Decision recorded: {}", clip(w, 100)),
            None => "A decision was recorded".to_string(),
        },
        "decision_asked" => format!("{who_cap} was asked to record a decision"),
        "usage" => "Token use was recorded".to_string(),
        "forecast_ran" => "The forecast was recalculated".to_string(),
        "waiting_on" => match str_of(d, "target_text") {
            Some(w) => format!("{who_cap} is waiting on {}", clip(w, 80)),
            None => format!("{who_cap} is waiting on something"),
        },
        "wait_cleared" => "A wait ended".to_string(),
        "report_published" => "A progress report was published".to_string(),
        "archived" => "Old events were archived".to_string(),
        "restored" => "Archived events were restored".to_string(),
        other => sentence_case(&words(other)),
    }
}

fn short_file(path: &str) -> String {
    let name = path.rsplit('/').next().unwrap_or(path);
    name.strip_suffix(".md").unwrap_or(name).to_string()
}

trait LowerFirst {
    fn to_lowercase_first(&self) -> String;
}

impl LowerFirst for String {
    /// `Task 1.2 (Login)` stays as it is; only `A task` loses its capital.
    fn to_lowercase_first(&self) -> String {
        if self.starts_with("A task") {
            format!("a{}", &self[1..])
        } else {
            self.clone()
        }
    }
}

/// A kind of event as a short label for the filter menu.
pub fn kind_label(kind: &str) -> String {
    let known = match kind {
        "session_started" => "Session started",
        "session_ended" => "Session ended",
        "session_crashed" => "Session stopped unexpectedly",
        "prompt" => "Your messages",
        "unit_stopped" => "Turns finished",
        "task_claimed" => "Task claimed",
        "task_released" => "Task let go",
        "claimed_done" => "Said done",
        "verified" => "Checked and passed",
        "verification_failed" => "Failed a check",
        "verification_stale" => "Check out of date",
        "verification_withdrawn" => "Sign-off taken back",
        "broken" => "Marked broken",
        "node_added" => "Plan: item added",
        "node_edited" => "Plan: item edited",
        "node_moved" => "Plan: item moved",
        "node_removed" => "Plan: item removed",
        "dep_added" => "Plan: wait added",
        "dep_removed" => "Plan: wait removed",
        "intent_set" => "Plan: purpose changed",
        "estimate_set" => "Plan: estimate set",
        "hill_moved" => "Plan: exploring or executing",
        "replan" => "Plan edited",
        "plan_imported" => "Plan file read",
        "plan_file_seen" => "Plan file looked at",
        "plan_scope_asked" => "Asked which plans are finished",
        "plan_scope_answered" => "Answered which plans are finished",
        "run_closed" => "Run closed",
        "detour_opened" => "Side trip started",
        "detour_closed" => "Side trip ended",
        "detour_promoted" => "Side trip became a task",
        "item_suspended" => "Task set aside",
        "item_resumed" => "Task picked up again",
        "unit_labeled" => "Turn labelled",
        "drift_flagged" => "Drift flagged",
        "nav_state_changed" => "Course changed",
        "owner_action" => "Your answers",
        "decision" => "Decision recorded",
        "decision_asked" => "Decision requested",
        "usage" => "Token use",
        "forecast_ran" => "Forecast recalculated",
        "checkpoint" => "Work saved",
        "checkpoint_due" => "Asked to save work",
        "checkpoint_missed" => "Work not saved",
        "leftover_found" => "Unsaved files found",
        "leftover_resolved" => "Unsaved files dealt with",
        "leftovers_analysed" => "Unsaved files looked over",
        "handoff_note" => "Handoff note",
        "readback" => "Handoff confirmed",
        "briefed" => "Briefing given",
        _ => "",
    };
    if known.is_empty() {
        sentence_case(&words(kind))
    } else {
        known.to_string()
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    fn task() -> TaskLabel {
        TaskLabel {
            shown: "4".into(),
            title: "Login form".into(),
        }
    }

    #[test]
    fn common_kinds_read_as_sentences() {
        let t = task();
        let cases: Vec<(&str, Value, String)> = vec![
            (
                "task_claimed",
                json!({}),
                "Task 4 (Login form) claimed by session a1b2c3".into(),
            ),
            (
                "verified",
                json!({"level": 2, "by": "agent"}),
                "Task 4 (Login form) passed its tests".into(),
            ),
            (
                "verified",
                json!({"level": 4, "by": "owner"}),
                "Task 4 (Login form) was signed off by you".into(),
            ),
            (
                "replan",
                json!({"op_count": 3, "reason": "scope grew"}),
                "Plan edited: 3 changes (scope grew)".into(),
            ),
            (
                "session_started",
                json!({"source": "resume"}),
                "Session a1b2c3 resumed".into(),
            ),
            (
                "nav_state_changed",
                json!({"to": "red"}),
                "Session a1b2c3 is far off its task".into(),
            ),
            (
                "owner_action",
                json!({"action": "drift", "answer": "stop"}),
                "You told a session to stop its side trip".into(),
            ),
            (
                "unit_labeled",
                json!({"label": "on_task"}),
                "A turn in session a1b2c3 was marked on task".into(),
            ),
        ];
        for (kind, data, want) in cases {
            assert_eq!(
                summarize(kind, &data, "a1b2c3dddd", Some(&t)),
                want,
                "{kind}"
            );
        }
    }

    #[test]
    fn unknown_kinds_fall_back_to_their_name_in_words() {
        assert_eq!(
            summarize("brand_new_thing", &json!({}), "s1", None),
            "Brand new thing"
        );
        assert_eq!(kind_label("brand_new_thing"), "Brand new thing");
    }

    #[test]
    fn long_text_is_clipped_to_one_line() {
        let long = "word ".repeat(60);
        let s = summarize("decision", &json!({"what": long}), "s1", None);
        assert!(s.chars().count() < 130 && s.ends_with('…'), "{s}");
    }
}
