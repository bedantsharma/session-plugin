//! One handler per hook event: turn the payload into events for the session.

use std::path::{Path, PathBuf};

use anyhow::Result;
use bridge_screen_core::binding;
use bridge_screen_core::error::Error;
use bridge_screen_core::repo;
use bridge_screen_core::session::{
    self, FileEvent, NeedsOwner, NeedsOwnerKind, Prompt, Session, SessionEnded, SessionStarted,
    SessionState, SubagentStopped, UnitStopped,
};
use bridge_screen_core::store::{Actor, EventDraft, Store, append_in};
use serde::Serialize;

use super::{HookEvent, HookInput, HookOutput, context};

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
        }
    }
}

/// Record what `event` means for the session and return any context for the agent.
pub fn handle(
    event: HookEvent,
    input: &HookInput,
    store: &mut Store,
) -> Result<Option<HookOutput>> {
    let Some(ctx) = resolve(event, input, store)? else {
        return Ok(None);
    };
    let call = (event == HookEvent::PreToolUse)
        .then(|| our_mcp_call(input))
        .flatten();
    let drafts = drafts_for(event, &ctx, store)?;
    if drafts.is_empty() && call.is_none() {
        return Ok(None);
    }
    let mut all = Vec::with_capacity(drafts.len() + 1);
    if ctx.known.is_none() && event != HookEvent::SessionStart {
        // A session started before bridge-screen was installed: begin tracking it now.
        all.push(ctx.draft("session_started", &ctx.session_started("unknown"))?);
    }
    all.extend(drafts);
    if !all.is_empty() {
        store.write(|tx| {
            for draft in all {
                append_in(tx, draft)?;
            }
            Ok(())
        })?;
    }
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
    let context = match event {
        HookEvent::SessionStart => {
            let session = stored_session(store, input)?;
            Some(context::session_start(store.conn(), &session)?)
        }
        HookEvent::UserPromptSubmit => {
            let session = stored_session(store, input)?;
            let unit_no =
                session::current_unit(store.conn(), &input.session_id)?.map_or(0, |u| u.unit_no);
            let prompt = input.prompt.as_deref().unwrap_or("");
            context::claim_reminder(store.conn(), &session, unit_no, prompt)?
        }
        _ => None,
    };
    Ok(context.map(|text| HookOutput {
        event,
        additional_context: Some(text),
    }))
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

/// The events one hook call records (possibly none).
fn drafts_for(event: HookEvent, ctx: &Ctx, store: &Store) -> Result<Vec<EventDraft>> {
    let input = ctx.input;
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
            let unit_no = if ctx.known.is_some() {
                session::current_unit(store.conn(), &input.session_id)?.map(|u| u.unit_no)
            } else {
                None
            };
            ctx.draft(
                "unit_stopped",
                &UnitStopped {
                    unit_no,
                    summary: summary(input),
                },
            )?
        }
        HookEvent::SubagentStop => ctx.draft(
            "subagent_stopped",
            &SubagentStopped {
                agent_type: input.agent_type.clone(),
                summary: summary(input),
            },
        )?,
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
            ctx.draft("needs_owner", &NeedsOwner { kind })?
        }
        HookEvent::SessionEnd => ctx.draft(
            "session_ended",
            &SessionEnded {
                reason: input.reason.clone(),
            },
        )?,
    };
    Ok(vec![draft])
}

/// A tool running while the session waits on the owner (e.g. a Bash call just approved)
/// ends the wait. Edit tools record their own events; any other tool records an empty
/// `kind` event only then, so the common path still writes nothing.
fn wait_over(ctx: &Ctx, store: &Store, kind: &str) -> Result<Vec<EventDraft>> {
    let waiting = ctx
        .known
        .as_ref()
        .is_some_and(|s| s.state == SessionState::NeedsOwner)
        && session::current_unit(store.conn(), &ctx.input.session_id)?
            .is_some_and(|u| u.wait_started_at.is_some());
    if !waiting {
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
