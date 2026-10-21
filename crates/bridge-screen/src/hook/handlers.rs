//! One handler per hook event: turn the payload into events for the session.

use std::path::{Path, PathBuf};

use anyhow::Result;
use bridge_screen_core::binding;
use bridge_screen_core::config::Config;
use bridge_screen_core::error::Error;
use bridge_screen_core::handoff::EndKind;
use bridge_screen_core::repo;
use bridge_screen_core::session::{
    self, FileEvent, NeedsOwner, NeedsOwnerKind, OwnerQuestion, Prompt, QuestionSource, Session,
    SessionEnded, SessionStarted, SessionState, SubagentStopped, UnitStopped,
};
use bridge_screen_core::store::{Actor, EventDraft, Store, append_in};
use bridge_screen_core::telemetry::{self, MAX_HOOK_BYTES};
use bridge_screen_core::tuning;
use serde::Serialize;

use super::{HookEvent, HookInput, HookOutput, context, handoff, nav, question};

/// Tools whose `tool_input` names files the agent is about to change.
const EDIT_TOOLS: &[&str] = &["Edit", "Write", "MultiEdit", "NotebookEdit"];

/// Longest unit summary kept from the agent's last message.
const SUMMARY_CHARS: usize = 300;

/// Who the hook fired for, once resolved.
struct Ctx<'a> {
    input: &'a HookInput,
    repo_id: String,
    /// The session as stored before this event; `None` if this hook is its first sight.
    known: Option<Session>,
}

impl Ctx<'_> {
    fn actor(&self) -> Actor {
        Actor {
            agent_id: self.input.agent_id.clone(),
            agent_type: self.input.agent_type.clone(),
            model: self.input.model.clone(),
        }
    }

    fn draft(&self, kind: &str, data: &impl Serialize) -> Result<EventDraft> {
        Ok(EventDraft::new(
            &self.repo_id,
            &self.input.session_id,
            kind,
            serde_json::to_value(data)?,
        )
        .actor(self.actor()))
    }

    fn session_started(&self, source: &str) -> SessionStarted {
        SessionStarted {
            cwd: self.input.cwd.to_string_lossy().into_owned(),
            branch: repo::current_branch(&self.input.cwd),
            source: source.to_string(),
            model: self.input.model.clone(),
            transcript_path: self.input.transcript_path.clone(),
            worktree: worktree_root(&self.input.cwd).map(|p| p.to_string_lossy().into_owned()),
        }
    }
}

/// Record what `event` means for the session and return any context for the agent.
pub fn handle(
    event: HookEvent,
    input: &HookInput,
    store: &mut Store,
    config: &Config,
) -> Result<Option<HookOutput>> {
    let Some(ctx) = resolve(event, input, store)? else {
        return Ok(None);
    };
    let call = (event == HookEvent::PreToolUse)
        .then(|| our_mcp_call(input))
        .flatten();
    let scored = score_closing(event, &ctx, store, input)?;
    let mut drafts = drafts_for(event, &ctx, store, scored)?;
    // An edit before the read-back the briefing asked for: one nudge (spec §8.4).
    let nudge = match event {
        HookEvent::PreToolUse => handoff::readback_nudge(
            store.conn(),
            ctx.known.as_ref(),
            edited_files(input).is_some(),
        )?
        .map(|(draft, text)| {
            drafts.push(draft.actor(ctx.actor()));
            text
        }),
        _ => None,
    };
    let navigates = ctx.known.is_some() && nav::navigates(event);
    // A known session with nothing to record writes nothing. An unknown one is tracked
    // from first sight (below), so its later hooks take the fast path without git.
    if drafts.is_empty()
        && call.is_none()
        && ctx.known.is_some()
        && !navigates
        && !(event == HookEvent::PreToolUse && handoff::opens_pr(input))
    {
        return Ok(None);
    }
    let mut all = Vec::with_capacity(drafts.len() + 1);
    if ctx.known.is_none() && event != HookEvent::SessionStart {
        // A session started before bridge-screen was installed: begin tracking it now.
        all.push(ctx.draft("session_started", &ctx.session_started("unknown"))?);
    }
    all.extend(drafts);
    let closed = scored.map(|(unit_no, _)| unit_no);
    let nav_ctx = nav::NavCtx {
        event,
        input,
        config,
        stopped_unit: closed.filter(|_| event == HookEvent::Stop),
        interrupted_unit: closed.filter(|_| event == HookEvent::UserPromptSubmit),
        actor: ctx.actor(),
        now: jiff::Timestamp::now(),
    };
    // Checkpoint git work, before taking the write lock (only when one is pending or a
    // commit was just made).
    let found = handoff::gather(store.conn(), event, ctx.known.as_ref(), input);
    // Nothing of our own to record: look first (read-only) whether the navigator has
    // anything to do, so the common tool call doesn't take the write lock.
    let worth_writing = !all.is_empty()
        || found.is_some()
        || (navigates
            && !(nav::step(store.conn(), &nav_ctx)?.is_empty()
                && nav::state_step(store.conn(), &nav_ctx)?.is_empty()));
    let note = if worth_writing {
        write_all(store, all, navigates, &nav_ctx, found.as_ref())?
    } else {
        None
    };
    if let Some((tool_use_id, tool, args)) = call {
        // Tell our MCP server which session is about to call it.
        binding::record(
            store.conn(),
            tool_use_id,
            &input.session_id,
            tool,
            &binding::args_hash(&args),
            jiff::Timestamp::now(),
        )?;
    }
    // Squash WIP commits before a PR opens (spec §8.1); never blocks the PR.
    let before_pr = match event {
        HookEvent::PreToolUse => handoff::before_pr(store, input)?,
        _ => None,
    };
    if event == HookEvent::SessionStart {
        start_steps(store, config, input, &ctx.repo_id)?;
    }
    let context = match event {
        HookEvent::SessionStart => handoff::start_context(store, input)?,
        HookEvent::UserPromptSubmit => {
            let session = stored_session(store, input)?;
            let unit_no =
                session::current_unit(store.conn(), &input.session_id)?.map_or(0, |u| u.unit_no);
            let prompt = input.prompt.as_deref().unwrap_or("");
            let reminder = context::claim_reminder(store.conn(), &session, unit_no, prompt)?;
            match (reminder, note) {
                (Some(r), Some(n)) => Some(format!("{r}\n{n}")),
                (r, n) => r.or(n),
            }
        }
        HookEvent::PreToolUse => before_pr.or(nudge).or(note),
        _ => note,
    };
    let context = match context {
        // A known session's note carries the pending decision ask (spec §8.6).
        Some(text) if event != HookEvent::SessionStart && ctx.known.is_some() => {
            Some(handoff::with_decision_ask(store, &input.session_id, text)?)
        }
        other => other,
    };
    Ok(context.map(|text| HookOutput {
        event,
        additional_context: Some(text),
    }))
}

/// SessionStart's work before the briefing: recovery and staleness (not after a
/// compaction), then drift tuning when due.
fn start_steps(store: &mut Store, config: &Config, input: &HookInput, repo_id: &str) -> Result<()> {
    if input.source.as_deref() != Some("compact") {
        // Recovery (spec §8.3): crashed sessions, then leftovers of dead ones.
        handoff::recover(store, config, repo_id, &input.session_id)?;
        // Verifications whose files changed since drop a level (spec §8.5).
        handoff::staleness(store, repo_id, &input.session_id)?;
    }
    if tuning::due(store.conn(), repo_id)? {
        // Enough new drift samples: tune this repo's thresholds (spec §7.6).
        store.write(|tx| tuning::run(tx, repo_id, config))?;
    }
    Ok(())
}

/// Append the hook's events, then the checkpoint and navigator steps, in one transaction.
/// Returns the note for the agent: a checkpoint request outranks every other note.
fn write_all(
    store: &mut Store,
    all: Vec<EventDraft>,
    navigates: bool,
    nav_ctx: &nav::NavCtx,
    found: Option<&handoff::CheckpointGit>,
) -> Result<Option<String>> {
    Ok(store.write(|tx| {
        for draft in all {
            append_in(tx, draft)?;
        }
        let checkpoint = match found {
            Some(f) => handoff::apply(tx, nav_ctx.input, f, nav_ctx.stopped_unit, &nav_ctx.actor)?,
            None => None,
        };
        if !navigates {
            return Ok(checkpoint);
        }
        // Labels first, so the tolerance band sees them.
        let labels = nav::step(tx, nav_ctx)?;
        for draft in labels.events {
            append_in(tx, draft)?;
        }
        let state = nav::state_step(tx, nav_ctx)?;
        for draft in state.events {
            append_in(tx, draft)?;
        }
        Ok(checkpoint.or(labels.note).or(state.note))
    })?)
}

/// The session as stored after this hook's events.
fn stored_session(store: &Store, input: &HookInput) -> Result<Session> {
    session::get(store.conn(), &input.session_id)?
        .ok_or_else(|| anyhow::anyhow!("session {} vanished", input.session_id))
}

/// Our MCP server's tools, as Claude Code names them for the bridge-screen plugin.
pub const MCP_TOOL_PREFIX: &str = "mcp__plugin_bridge-screen_bridge-screen__";

/// For a call to one of our MCP tools: (tool_use_id, short tool name, arguments).
fn our_mcp_call(input: &HookInput) -> Option<(&str, &str, serde_json::Value)> {
    let tool = input.tool_name.as_deref()?.strip_prefix(MCP_TOOL_PREFIX)?;
    let id = input.tool_use_id.as_deref()?;
    let args = input
        .tool_input
        .clone()
        .unwrap_or_else(|| serde_json::json!({}));
    Some((id, tool, args))
}

/// Find the session's repo. Known sessions never run git; unknown ones outside a git repo
/// are ignored (`None`).
fn resolve<'a>(event: HookEvent, input: &'a HookInput, store: &Store) -> Result<Option<Ctx<'a>>> {
    if let Some(known) = session::get(store.conn(), &input.session_id)? {
        return Ok(Some(Ctx {
            input,
            repo_id: known.repo_id.clone(),
            known: Some(known),
        }));
    }
    if event == HookEvent::SessionEnd {
        // Nothing to close for a session bridge-screen never saw.
        return Ok(None);
    }
    let info = match repo::identify(&input.cwd) {
        Ok(info) => info,
        Err(Error::NotARepo(_)) => return Ok(None),
        Err(e) => return Err(e.into()),
    };
    store.register_repo(&info)?;
    Ok(Some(Ctx {
        input,
        repo_id: info.id,
        known: None,
    }))
}

/// Stop scores the unit it is about to close (in memory; the message isn't stored). A
/// prompt closes a unit that missed its Stop and scores it from its files alone.
fn score_closing(
    event: HookEvent,
    ctx: &Ctx,
    store: &Store,
    input: &HookInput,
) -> Result<Option<(i64, Option<f64>)>> {
    let Some(known) = &ctx.known else {
        return Ok(None);
    };
    let summary = match event {
        HookEvent::Stop => input.last_assistant_message.as_deref().unwrap_or(""),
        HookEvent::UserPromptSubmit => "",
        _ => return Ok(None),
    };
    Ok(nav::score_open_unit(store.conn(), known, summary)?)
}

/// The events one hook call records (possibly none). `scored` is the unit a Stop (or a
/// prompt, when the unit missed its Stop) closes and its drift score.
fn drafts_for(
    event: HookEvent,
    ctx: &Ctx,
    store: &Store,
    scored: Option<(i64, Option<f64>)>,
) -> Result<Vec<EventDraft>> {
    let input = ctx.input;
    if asks_owner(input) && matches!(event, HookEvent::PreToolUse | HookEvent::PostToolUse) {
        return ask_tool_drafts(event, ctx, store);
    }
    let draft = match event {
        HookEvent::SessionStart => {
            let source = input.source.as_deref().unwrap_or("startup");
            ctx.draft("session_started", &ctx.session_started(source))?
        }
        HookEvent::UserPromptSubmit => ctx.draft(
            "prompt",
            &Prompt {
                prompt_id: input.prompt_id.clone(),
                chars: input
                    .prompt
                    .as_deref()
                    .map_or(0, |p| p.chars().count() as u64),
                closed_score: scored.and_then(|(_, score)| score),
            },
        )?,
        HookEvent::PreToolUse => {
            let Some((tool, files)) = edited_files(input) else {
                return wait_over(ctx, store, "tool_intent");
            };
            let seen = if ctx.known.is_some() {
                session::open_unit_intents(store.conn(), &input.session_id)?
            } else {
                std::collections::HashSet::default()
            };
            if files.iter().all(|f| seen.contains(f)) {
                return wait_over(ctx, store, "tool_intent");
            }
            ctx.draft("tool_intent", &FileEvent { tool, files })?
        }
        HookEvent::PostToolUse => {
            let Some((tool, files)) = edited_files(input) else {
                return wait_over(ctx, store, "file_changed");
            };
            ctx.draft("file_changed", &FileEvent { tool, files })?
        }
        HookEvent::Stop => {
            let mut drafts = stop_drafts(ctx, store, scored)?;
            drafts.extend(usage_draft(event, ctx, store)?);
            drafts.extend(lagging_subagent_draft(event, ctx, store)?);
            return Ok(drafts);
        }
        HookEvent::SubagentStop => {
            let stopped = ctx.draft(
                "subagent_stopped",
                &SubagentStopped {
                    agent_type: input.agent_type.clone(),
                    summary: summary(input),
                },
            )?;
            let mut drafts = vec![stopped];
            drafts.extend(usage_draft(event, ctx, store)?);
            return Ok(drafts);
        }
        HookEvent::PreCompact => ctx.draft("precompact", &serde_json::json!({}))?,
        HookEvent::Notification => {
            let kind = match input.notification_type.as_deref() {
                Some("permission_prompt") => NeedsOwnerKind::PermissionPrompt,
                Some("idle_prompt") => NeedsOwnerKind::IdlePrompt,
                Some("elicitation_dialog" | "elicitation_url_dialog" | "agent_needs_input") => {
                    NeedsOwnerKind::Other
                }
                // auth_success, agent_completed, elicitation_complete, …: nobody waits.
                _ => return Ok(Vec::new()),
            };
            ctx.draft(
                "needs_owner",
                &NeedsOwner {
                    kind,
                    task_id: None,
                },
            )?
        }
        HookEvent::SessionEnd => {
            // How it ended (spec §8.2): git runs here, once per session.
            let (end_kind, dirty_files) = match &ctx.known {
                Some(known) => handoff::end_state(store.conn(), known),
                None => (None, Vec::new()),
            };
            let ended = ctx.draft(
                "session_ended",
                &SessionEnded {
                    reason: input.reason.clone(),
                    end_kind,
                    dirty_files,
                },
            )?;
            // One last (still bounded) read of the transcript.
            let mut drafts = vec![ended];
            drafts.extend(usage_draft(event, ctx, store)?);
            return Ok(drafts);
        }
    };
    Ok(vec![draft])
}

/// The transcript a Stop, SessionEnd or SubagentStop reads: the session's, or the
/// subagent's own (`agent_transcript_path`, else derived from the session's).
fn transcript_for(event: HookEvent, ctx: &Ctx) -> Option<PathBuf> {
    let input = ctx.input;
    let main = input
        .transcript_path
        .clone()
        .or_else(|| ctx.known.as_ref().and_then(|k| k.transcript_path.clone()))
        .filter(|p| !p.is_empty())
        .map(PathBuf::from);
    if event != HookEvent::SubagentStop {
        return main;
    }
    if let Some(own) = input
        .agent_transcript_path
        .as_deref()
        .filter(|p| !p.is_empty())
    {
        return Some(PathBuf::from(own));
    }
    let agent = input.agent_id.as_deref()?;
    Some(telemetry::subagent_path(&main?, &input.session_id, agent))
}

/// The `usage` event for the transcript's new lines (at most [`MAX_HOOK_BYTES`]), if any
/// reply is new or the cursor moved (spec §9.2). Reading the transcript must never cost
/// the session its other events: a failure is logged and nothing is recorded.
fn usage_draft(event: HookEvent, ctx: &Ctx, store: &Store) -> Result<Option<EventDraft>> {
    let Some(path) = transcript_for(event, ctx) else {
        return Ok(None);
    };
    let found = match telemetry::pending(store.conn(), &path, Some(MAX_HOOK_BYTES)) {
        Ok(found) => found,
        Err(e) => {
            log(event, &format!("reading {}: {e}", path.display()));
            return Ok(None);
        }
    };
    if found.skipped_long_line {
        log(
            event,
            &format!(
                "skipped a transcript line too long to read in {}",
                path.display()
            ),
        );
    }
    let Some(mut usage) = found.usage else {
        return Ok(None);
    };
    if event == HookEvent::SubagentStop {
        usage.agent_id.clone_from(&ctx.input.agent_id);
        usage.agent_type.clone_from(&ctx.input.agent_type);
    }
    Ok(Some(ctx.draft("usage", &usage)?))
}

/// The next slice of one subagent transcript that its SubagentStop didn't finish (spec
/// §9.2): a subagent stops once, so the session's Stops read the rest, one file and at
/// most [`MAX_HOOK_BYTES`] per Stop. Failures are logged; nothing is recorded.
fn lagging_subagent_draft(
    event: HookEvent,
    ctx: &Ctx,
    store: &Store,
) -> Result<Option<EventDraft>> {
    // The first file with a whole new line (one only holding a half-written line yields
    // nothing and doesn't hold up the others).
    let found = (|| -> bridge_screen_core::error::Result<Option<telemetry::Usage>> {
        for (agent_id, agent_type, path) in
            telemetry::lagging_subagents(store.conn(), &ctx.input.session_id)?
        {
            let p = telemetry::pending(store.conn(), &path, Some(MAX_HOOK_BYTES))?;
            if let Some(mut u) = p.usage {
                u.agent_id = Some(agent_id);
                u.agent_type = agent_type;
                return Ok(Some(u));
            }
        }
        Ok(None)
    })();
    match found {
        Ok(Some(usage)) => Ok(Some(ctx.draft("usage", &usage)?)),
        Ok(None) => Ok(None),
        Err(e) => {
            log(event, &format!("reading a subagent transcript: {e}"));
            Ok(None)
        }
    }
}

/// Note a problem in `hook-errors.log` without failing the hook.
fn log(event: HookEvent, message: &str) {
    if let Ok(home) = bridge_screen_core::paths::Home::resolve() {
        super::log_error(&home, event.kebab(), message);
    }
}

/// Stop: the unit stops, and a turn that ends with a question to the owner is one (the
/// message isn't stored).
fn stop_drafts(
    ctx: &Ctx,
    store: &Store,
    scored: Option<(i64, Option<f64>)>,
) -> Result<Vec<EventDraft>> {
    let input = ctx.input;
    let unit_no = if ctx.known.is_some() {
        session::current_unit(store.conn(), &input.session_id)?.map(|u| u.unit_no)
    } else {
        None
    };
    let mut drafts = vec![ctx.draft(
        "unit_stopped",
        &UnitStopped {
            unit_no,
            summary: summary(input),
            score: scored.and_then(|(_, score)| score),
        },
    )?];
    // The branch may have changed since SessionStart: file reads only, no git process.
    // Read the session's own worktree when known: the turn's cwd may have moved into
    // another repo, whose branch isn't this session's.
    if let Some(known) = &ctx.known {
        let stop_at = known.worktree.as_deref().map(std::path::Path::new);
        if let Some(branch) = repo::read_head(stop_at.unwrap_or(&input.cwd), stop_at)
            && known.branch.as_deref() != Some(branch.as_str())
        {
            drafts.push(ctx.draft("branch_changed", &session::BranchChanged { branch })?);
        }
    }
    if question::ends_with_question(input.last_assistant_message.as_deref()) {
        drafts.push(ctx.draft(
            "owner_question",
            &OwnerQuestion {
                unit_no,
                source: QuestionSource::TurnEnd,
            },
        )?);
    }
    Ok(drafts)
}

/// The ask tool waits on the owner: a needs-you item from its PreToolUse until its
/// PostToolUse (the answer). No question text is kept.
fn ask_tool_drafts(event: HookEvent, ctx: &Ctx, store: &Store) -> Result<Vec<EventDraft>> {
    if event == HookEvent::PostToolUse {
        let mut drafts = wait_over(ctx, store, "file_changed")?;
        drafts.push(ctx.draft(
            "question_answered",
            &serde_json::json!({"source": QuestionSource::AskTool.as_str()}),
        )?);
        return Ok(drafts);
    }
    let unit_no = if ctx.known.is_some() {
        session::current_unit(store.conn(), &ctx.input.session_id)?.map(|u| u.unit_no)
    } else {
        None
    };
    let mut drafts = wait_over(ctx, store, "tool_intent")?;
    drafts.push(ctx.draft(
        "owner_question",
        &OwnerQuestion {
            unit_no,
            source: QuestionSource::AskTool,
        },
    )?);
    Ok(drafts)
}

/// A call to Claude Code's ask tool.
fn asks_owner(input: &HookInput) -> bool {
    input.tool_name.as_deref() == Some(question::ASK_TOOL)
}

/// A tool running while the session waits on the owner (e.g. a Bash call just approved)
/// ends the wait. Edit tools record their own events; any other tool records an empty
/// `kind` event only then, so the common path still writes nothing. A session marked
/// crashed that runs a tool was only quiet (a long build, an absent owner): the event
/// brings it back to life (spec §8.2).
fn wait_over(ctx: &Ctx, store: &Store, kind: &str) -> Result<Vec<EventDraft>> {
    let crashed = ctx
        .known
        .as_ref()
        .is_some_and(|s| s.end_kind == Some(EndKind::Crashed));
    let waiting = ctx
        .known
        .as_ref()
        .is_some_and(|s| s.state == SessionState::NeedsOwner)
        && session::current_unit(store.conn(), &ctx.input.session_id)?
            .is_some_and(|u| u.wait_started_at.is_some());
    if !crashed && !waiting {
        return Ok(Vec::new());
    }
    let tool = ctx.input.tool_name.clone().unwrap_or_default();
    Ok(vec![ctx.draft(
        kind,
        &FileEvent {
            tool,
            files: Vec::new(),
        },
    )?])
}

fn summary(input: &HookInput) -> Option<String> {
    input
        .last_assistant_message
        .as_deref()
        .map(|m| m.chars().take(SUMMARY_CHARS).collect())
}

/// For an edit tool: the tool name and the files it touches, relative to the worktree root
/// when inside it.
fn edited_files(input: &HookInput) -> Option<(String, Vec<String>)> {
    let tool = input.tool_name.as_deref()?;
    if !EDIT_TOOLS.contains(&tool) {
        return None;
    }
    let args = input.tool_input.as_ref()?;
    let files: Vec<String> = ["file_path", "notebook_path"]
        .iter()
        .filter_map(|k| args.get(*k).and_then(|v| v.as_str()))
        .map(|f| display_path(f, &input.cwd))
        .collect();
    (!files.is_empty()).then(|| (tool.to_string(), files))
}

/// `file` relative to the worktree containing `cwd`, or absolute if it's outside it.
/// Walks up looking for `.git` instead of running git, to keep hooks fast.
fn display_path(file: &str, cwd: &Path) -> String {
    let path = if Path::new(file).is_absolute() {
        PathBuf::from(file)
    } else {
        cwd.join(file)
    };
    let Some(root) = worktree_root(cwd) else {
        return path.to_string_lossy().into_owned();
    };
    if let Ok(rel) = path.strip_prefix(&root) {
        return rel.to_string_lossy().into_owned();
    }
    // The same place spelled differently (e.g. /var vs /private/var on macOS).
    let real_root = std::fs::canonicalize(&root).ok();
    let real_path = path
        .parent()
        .and_then(|p| std::fs::canonicalize(p).ok())
        .zip(path.file_name());
    if let (Some(root), Some((parent, name))) = (real_root, real_path)
        && let Ok(rel) = parent.join(name).strip_prefix(root)
    {
        return rel.to_string_lossy().into_owned();
    }
    path.to_string_lossy().into_owned()
}

fn worktree_root(cwd: &Path) -> Option<PathBuf> {
    cwd.ancestors()
        .find(|dir| dir.join(".git").exists())
        .map(Path::to_path_buf)
}

#[cfg(test)]
mod tests {
    use super::display_path;
    use std::path::Path;

    #[test]
    fn paths_are_relative_to_the_worktree() {
        let repo = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(repo.path().join(".git")).unwrap();
        std::fs::create_dir_all(repo.path().join("src/deep")).unwrap();
        let root = repo.path().to_str().unwrap();
        let sub = repo.path().join("src");
        assert_eq!(display_path(&format!("{root}/src/a.rs"), &sub), "src/a.rs");
        assert_eq!(display_path("deep/b.rs", &sub), "src/deep/b.rs");
        assert_eq!(display_path("/etc/hosts", &sub), "/etc/hosts");
        let plain = tempfile::tempdir().unwrap();
        let elsewhere = format!("{}/x.rs", plain.path().display());
        assert_eq!(display_path(&elsewhere, Path::new(root)), elsewhere);
    }
}
