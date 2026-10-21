//! `bridge-screen mcp`: the agent's tools over MCP stdio (spec §11.2).
//!
//! Claude Code starts one server per session but doesn't say which session it is. Each
//! call is matched to the binding the `PreToolUse` hook recorded moments earlier (see
//! `bridge_screen_core::binding`); without one, the newest live session inside
//! `CLAUDE_PROJECT_DIR` is used.

use std::borrow::Cow;
use std::collections::HashMap;
use std::fmt::Write as _;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use anyhow::Result;

use crate::hook::handoff::worktree_of;
use bridge_screen_core::binding;
use bridge_screen_core::checkpoint::{self, ClaimDoneRequest};
use bridge_screen_core::claim::{self, ClaimMode, ClaimRequest, ClaimTarget};
use bridge_screen_core::config::Config;
use bridge_screen_core::decision;
use bridge_screen_core::handoff::{self, LeftoverAction};
use bridge_screen_core::owner::{self, OwnerAction};
use bridge_screen_core::paths::Home;
use bridge_screen_core::plan::{EditContext, NodeKind, NodeStatus, PlanOp, get_node, load_tree};
use bridge_screen_core::recovery::{self, Caller};
use bridge_screen_core::session::{self, Session, SessionState};
use bridge_screen_core::store::{Actor, Store};
use bridge_screen_core::tangent::{
    self, CloseRequest, DetourOutcome, Label, LabelRequest, LabelSource, StartRequest,
};
use bridge_screen_core::verify::{self, L1Outcome};
use bridge_screen_core::view;
use rmcp::handler::server::router::tool::ToolRouter;
use rmcp::handler::server::wrapper::Parameters;
use rmcp::model::{
    CallToolResult, ContentBlock, Implementation, ListResourcesResult, MetaObject,
    PaginatedRequestParams, ReadResourceRequestParams, ReadResourceResponse, ReadResourceResult,
    ServerCapabilities, ServerConfig,
};
use rmcp::service::RequestContext;
use rmcp::{ErrorData, RoleServer};
use rmcp::{ServerHandler, ServiceExt, tool, tool_handler, tool_router};
use schemars::JsonSchema;
use serde::de::DeserializeOwned;
use serde::{Deserialize, Deserializer};
use serde_json::Value;

const NO_SESSION: &str =
    "bridge-screen couldn't tell which session this is; is the plugin's hook enabled?";

/// Tool arguments kept as sent (for matching the hook's binding) plus their parsed form.
/// Parse failures become tool errors the agent can read, not protocol errors.
pub struct Args<T> {
    raw: Value,
    parsed: Result<T, String>,
}

impl<'de, T: DeserializeOwned> Deserialize<'de> for Args<T> {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let raw = Value::deserialize(d)?;
        let parsed =
            serde_json::from_value(raw.clone()).map_err(|e| format!("invalid arguments: {e}"));
        Ok(Self { raw, parsed })
    }
}

/// The schema agents see is the parsed type's.
impl<T: JsonSchema> JsonSchema for Args<T> {
    fn schema_name() -> Cow<'static, str> {
        T::schema_name()
    }

    fn schema_id() -> Cow<'static, str> {
        T::schema_id()
    }

    fn inline_schema() -> bool {
        T::inline_schema()
    }

    fn json_schema(generator: &mut schemars::SchemaGenerator) -> schemars::Schema {
        T::json_schema(generator)
    }
}

/// A task to create and claim.
#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct NewTask {
    /// Id of the milestone (or goal) for a new task, or of the task for a new subtask.
    pub parent: String,
    /// One line, at most 200 characters.
    pub title: String,
    /// Your estimate in agent-minutes.
    #[serde(default)]
    pub estimate_min: Option<u32>,
}

/// Claim the task this session is working on. Give exactly one of `id` or `new_task`.
#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ClaimTaskInput {
    /// Id of an existing task or subtask in the plan, e.g. "1.2".
    #[serde(default)]
    pub id: Option<String>,
    /// Create this task and claim it, when nothing in the plan fits.
    #[serde(default)]
    pub new_task: Option<NewTask>,
    /// "auto" when the request clearly matches the task; "confirmed" when the owner
    /// confirmed it.
    pub mode: ClaimMode,
    /// How sure you are that this is the right task, from 0 to 1.
    pub confidence: f64,
}

/// Change the plan. All operations apply together or not at all.
#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct PlanEditInput {
    /// Operations, applied in order.
    #[schemars(with = "Vec<PlanOp>")]
    pub ops: Vec<Value>,
    /// Why the plan is changing.
    #[serde(default)]
    pub reason: Option<String>,
    /// Record this batch as one deliberate re-plan (needs a reason).
    #[serde(default)]
    pub replan: bool,
}

/// No arguments.
#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct StatusInput {}

/// Open a detour: work that isn't your task but blocks it.
#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct StartDetourInput {
    /// One line: what the detour is, e.g. "build is broken on main".
    pub reason: String,
    /// One line (at most 200 characters) naming the next action on the work you are
    /// suspending and the file or test it concerns. You get it back verbatim on return.
    pub resume_cue: String,
    /// The task this detour blocks; defaults to your active task.
    #[serde(default)]
    pub blocks: Option<String>,
    /// true when the owner asked for this work in this session (e.g. "also add a
    /// licence"): it is then never counted as drift or time away from the task.
    #[serde(default)]
    pub requested_by_owner: bool,
}

/// Close a detour and get the resume cue of the work it suspended.
#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct CloseDetourInput {
    /// "done" when the blocker is dealt with, "abandoned" when you gave up on it.
    pub outcome: DetourOutcome,
    /// Optional one-line note on how it ended.
    #[serde(default)]
    pub note: Option<String>,
    /// Which detour, as "D2"; defaults to the one you are on.
    #[serde(default)]
    pub detour: Option<String>,
}

/// Resume a suspended item.
#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ResumeInput {
    /// The item as `status` lists it: a task id such as "1.2", a detour such as "D1", or
    /// the item's id.
    pub item_id: String,
}

/// Label your last unit of work.
#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct LabelUnitInput {
    /// "on_task" if it advanced your task, "detour" if it wasn't the task but blocked it,
    /// "requested" if it wasn't the task but the owner asked for it in this session,
    /// "drift" if it did none of these.
    pub label: Label,
    /// One line on why.
    pub why: String,
}

/// Turn the current detour into a real task and claim it.
#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct PromoteDetourInput {
    /// Id of the milestone (or goal) for the new task, or of a task for a new subtask.
    pub parent: String,
    /// One line, at most 200 characters.
    pub title: String,
    /// Your estimate in agent-minutes.
    #[serde(default)]
    pub estimate_min: Option<u32>,
}

/// Say a task is done (verification level L0).
#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ClaimDoneInput {
    /// The task you finished; defaults to your active task.
    #[serde(default)]
    pub id: Option<String>,
    /// What shows it is done: the tests you ran and their result, the files changed.
    pub evidence: String,
    /// The files (paths relative to the repo root) that show the work exists, at most 50.
    /// bridge-screen checks them at HEAD for L1.
    #[serde(default)]
    pub files: Vec<String>,
    /// Optionally the key functions or types in those files, at most 50.
    #[serde(default)]
    pub symbols: Vec<String>,
}

/// Climb the verification ladder for a task you claimed done.
#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct VerifyInput {
    /// The task; defaults to your active task.
    #[serde(default)]
    pub id: Option<String>,
    /// 2: the unit tests pass (committed; give exit_code and summary). 3: end-to-end — you
    /// drove it (browser or CLI) and saw it work. 4: the owner said it works (owner_said).
    pub level: i64,
    /// What you ran or drove and what it showed.
    pub evidence: String,
    /// Level 2: the exit code of your test run. Report it as it was; a non-zero code is
    /// refused.
    #[serde(default)]
    pub exit_code: Option<i64>,
    /// Level 2: the test summary line, e.g. "42 passed, 0 failed".
    #[serde(default)]
    pub summary: Option<String>,
    /// Level 4: the owner's own words, at most 200 characters.
    #[serde(default)]
    pub owner_said: Option<String>,
}

/// Record a decision.
#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct LogDecisionInput {
    /// What was decided, in one line (at most 200 characters).
    pub what: String,
    /// Why (at most 500 characters).
    pub why: String,
    /// The options considered and not taken (at most 5).
    #[serde(default)]
    pub alternatives: Vec<String>,
    /// The id (e.g. "D4") of the decision this one replaces. Supersede rather than
    /// contradict.
    #[serde(default)]
    pub supersedes: Option<String>,
    /// The task it belongs to; defaults to your active task. null for a repo-wide decision.
    #[serde(default, deserialize_with = "present")]
    #[schemars(with = "Option<String>")]
    pub task: Option<Nullable>,
}

/// A field that may be given as null, which differs from leaving it out.
#[derive(Debug, Clone, Deserialize)]
#[serde(transparent)]
pub struct Nullable(pub Option<String>);

/// A field that was given (even as null) is `Some`; a missing one stays `None`.
fn present<'de, D: Deserializer<'de>>(d: D) -> Result<Option<Nullable>, D::Error> {
    Ok(Some(Nullable::deserialize(d)?))
}

/// Resolve leftovers a dead session left in a worktree.
#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ResolveLeftoverInput {
    /// Leftover ids as the briefing or status lists them.
    pub ids: Vec<String>,
    /// "adopt" (commit them as they are), "resume" (claim their task and finish them) or
    /// "undo" (stash them; nothing is lost).
    pub action: LeftoverAction,
}

/// Read back the briefing before your first edit.
#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct HandoffReadbackInput {
    /// The goal, in your words.
    pub goal: String,
    /// What is in flight: the task id and what is half-done.
    pub in_flight: String,
    /// Your next action.
    pub next_action: String,
}

/// Leave a note for the next session.
#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct HandoffNoteInput {
    /// What is half-done right now, in one or two sentences.
    pub in_flight: String,
    /// Up to 5 warnings for the next session, as "if X, then Y".
    #[serde(default)]
    pub if_then: Vec<String>,
}

/// Which projects the progress card shows.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum Scope {
    /// This session's repo.
    Repo,
    /// Every project.
    All,
}

/// Show the owner where things stand.
#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ShowProgressInput {
    /// "all" (default) or "repo" for this session's project only.
    #[serde(default)]
    pub scope: Option<Scope>,
}

/// Make a progress report for publishing, on the owner's explicit request.
#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ProgressReportInput {
    /// "repo" (default: this session's project) or "all".
    #[serde(default)]
    pub scope: Option<Scope>,
}

/// Record that the owner published a report.
#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ReportPublishedInput {
    /// The published page's link.
    pub url: String,
    /// The scope the report was made with: "repo" or "all".
    pub scope: Scope,
}

/// A click on the progress card.
#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct OwnerActionInput {
    /// The action exactly as the card received it.
    pub action: OwnerAction,
    /// The nonce `show_progress` gave the card.
    pub nonce: String,
}

#[derive(Clone)]
pub struct Bridge {
    home: Home,
    project_dir: Option<PathBuf>,
    tool_router: ToolRouter<Self>,
    /// Nonces `show_progress` handed to the progress card, by when they were minted.
    nonces: Arc<Mutex<HashMap<String, Instant>>>,
}

fn text(result: Result<String, String>) -> CallToolResult {
    match result {
        Ok(t) => CallToolResult::success(vec![ContentBlock::text(t)]),
        Err(e) => CallToolResult::error(vec![ContentBlock::text(e)]),
    }
}

fn msg(e: impl std::fmt::Display) -> String {
    e.to_string()
}

#[tool_router]
impl Bridge {
    pub fn new(home: Home, project_dir: Option<PathBuf>) -> Self {
        Self {
            home,
            project_dir,
            tool_router: Self::tool_router(),
            nonces: Arc::default(),
        }
    }

    fn open(&self) -> Result<Store, String> {
        self.home.ensure().map_err(msg)?;
        // A newer data format (bridge-screen upgraded while this session was open) comes
        // back as the tool's result text, telling the agent to start a new session.
        Store::open(&self.home.db_path()).map_err(msg)
    }

    /// The session making this call.
    fn caller(&self, store: &Store, tool: &str, raw: &Value) -> Result<Session, String> {
        let hash = binding::args_hash(raw);
        let bound =
            binding::take(store.conn(), tool, &hash, jiff::Timestamp::now()).map_err(msg)?;
        if let Some(id) = bound
            && let Some(s) = session::get(store.conn(), &id).map_err(msg)?
        {
            return Ok(s);
        }
        if let Some(dir) = &self.project_dir
            && let Some(s) = newest_session_in(store, dir)?
        {
            return Ok(s);
        }
        Err(NO_SESSION.to_string())
    }

    #[tool(
        description = "Claim the plan task this session is working on. Call it before starting work: with mode \"auto\" when the request clearly matches a task, or \"confirmed\" after the owner confirmed it. Use new_task to create and claim a task (or subtask) when nothing in the plan fits."
    )]
    async fn claim_task(
        &self,
        Parameters(args): Parameters<Args<ClaimTaskInput>>,
    ) -> CallToolResult {
        text(self.do_claim(&args))
    }

    #[tool(
        description = "Say a task is done when you finish it (verification level L0), with evidence: the tests you ran and their result. List the files (and optionally the key functions/types) that show the work exists — bridge-screen checks them at HEAD for L1. A checkpoint commit is then due before you stop."
    )]
    async fn claim_done(
        &self,
        Parameters(args): Parameters<Args<ClaimDoneInput>>,
    ) -> CallToolResult {
        text(self.do_claim_done(&args))
    }

    #[tool(
        description = "Climb the verification ladder for a task you claimed done, honestly: level 2 when its unit tests pass and the work is committed (give the exit code and summary; bridge-screen re-runs the owner's test command), level 3 when you drove it end to end (say what you drove), level 4 only with the owner's own words. Every level is tied to the current commit."
    )]
    async fn verify(&self, Parameters(args): Parameters<Args<VerifyInput>>) -> CallToolResult {
        // The re-run can take minutes: keep the server responsive meanwhile.
        let this = self.clone();
        text(
            tokio::task::spawn_blocking(move || this.do_verify(&args))
                .await
                .unwrap_or_else(|e| Err(format!("verify stopped: {e}"))),
        )
    }

    #[tool(
        description = "Record a decision: what was decided, why, and the alternatives. Log decisions when a task is done, before compaction and before you stop. To change one, supersede it (supersedes: \"D4\") rather than contradicting it; the same decision logged again on a task is merged."
    )]
    async fn log_decision(
        &self,
        Parameters(args): Parameters<Args<LogDecisionInput>>,
    ) -> CallToolResult {
        text(self.do_log_decision(&args))
    }

    #[tool(
        description = "Resolve leftovers a dead session left uncommitted: adopt (commit them), resume (claim their task and continue; only in their worktree) or undo (stash them with a snapshot record; nothing is lost). Prefer the suggested action; ask the owner when unsure."
    )]
    async fn resolve_leftover(
        &self,
        Parameters(args): Parameters<Args<ResolveLeftoverInput>>,
    ) -> CallToolResult {
        text(self.do_resolve_leftover(&args))
    }

    #[tool(
        description = "Read back the briefing before your first edit when it asks: the goal, what's in flight (name the task id) and your next action. bridge-screen checks it against the log and corrects you once if it differs."
    )]
    async fn handoff_readback(
        &self,
        Parameters(args): Parameters<Args<HandoffReadbackInput>>,
    ) -> CallToolResult {
        text(self.do_handoff_readback(&args))
    }

    #[tool(
        description = "Leave a note for the next session when you stop mid-task or finish one: what's in flight and up to 5 if/then warnings. The next session's briefing carries it."
    )]
    async fn handoff_note(
        &self,
        Parameters(args): Parameters<Args<HandoffNoteInput>>,
    ) -> CallToolResult {
        text(self.do_handoff_note(&args))
    }

    #[tool(
        description = "Change the plan: add, edit, move or remove nodes; set intents, estimates (agent-minutes), dependencies and milestone hill state. Give a reason. All ops apply together or none do."
    )]
    async fn plan_edit(&self, Parameters(args): Parameters<Args<PlanEditInput>>) -> CallToolResult {
        text(self.do_plan_edit(&args))
    }

    #[tool(
        description = "This session's bridge-screen state: active task, units of work and active minutes, the plan goal and the next tasks to do."
    )]
    async fn status(&self, Parameters(args): Parameters<Args<StatusInput>>) -> CallToolResult {
        text(self.do_status(&args))
    }

    #[tool(
        description = "Open a detour before tangent work that isn't your task but blocks it (a broken build, a missing dependency), or that the owner asked for in this session (set requested_by_owner: true; it is then never drift). Give a one-line reason and a resume_cue: the next action on the work you are suspending and the file or test it concerns. Close it with close_detour when done."
    )]
    async fn start_detour(
        &self,
        Parameters(args): Parameters<Args<StartDetourInput>>,
    ) -> CallToolResult {
        text(self.do_start_detour(&args))
    }

    #[tool(
        description = "Close your current detour (outcome \"done\" or \"abandoned\"). Returns the task to go back to and its resume cue, verbatim."
    )]
    async fn close_detour(
        &self,
        Parameters(args): Parameters<Args<CloseDetourInput>>,
    ) -> CallToolResult {
        text(self.do_close_detour(&args))
    }

    #[tool(
        description = "Resume any suspended item (a task or a detour) directly, by the id status lists. Returns its resume cue."
    )]
    async fn resume(&self, Parameters(args): Parameters<Args<ResumeInput>>) -> CallToolResult {
        text(self.do_resume(&args))
    }

    #[tool(
        description = "Label your last unit of work when bridge-screen asks: on_task, detour, requested (the owner asked for it in this session) or drift, with a one-line why. Be honest: drift is information for the owner, not blame. Calling it again corrects the label."
    )]
    async fn label_unit(
        &self,
        Parameters(args): Parameters<Args<LabelUnitInput>>,
    ) -> CallToolResult {
        text(self.do_label_unit(&args))
    }

    #[tool(
        description = "Turn a detour that has grown into real work into a plan task under parent, and claim it. The work it suspended stays suspended."
    )]
    async fn promote_detour(
        &self,
        Parameters(args): Parameters<Args<PromoteDetourInput>>,
    ) -> CallToolResult {
        text(self.do_promote_detour(&args))
    }

    #[tool(
        description = "Show the owner where things stand: needs-you items, project progress and live sessions. Call it when the owner asks for progress or runs /bridge-screen. In the Claude desktop app it renders as an interactive card; elsewhere it returns the text view.",
        meta = ui_meta(serde_json::json!({ "resourceUri": PROGRESS_URI }))
    )]
    async fn show_progress(
        &self,
        Parameters(args): Parameters<Args<ShowProgressInput>>,
    ) -> CallToolResult {
        self.do_show_progress(&args)
            .unwrap_or_else(|e| CallToolResult::error(vec![ContentBlock::text(e)]))
    }

    #[tool(
        description = "Only when the owner explicitly asks to publish a progress report: writes a self-contained HTML snapshot (no scripts, no outside links) to a temp file and returns its path and a summary of exactly what it contains. Show the owner that summary; publish with the Artifact tool only after the owner confirms; then call report_published. Never publish on your own initiative."
    )]
    async fn progress_report(
        &self,
        Parameters(args): Parameters<Args<ProgressReportInput>>,
    ) -> CallToolResult {
        text(self.do_progress_report(&args))
    }

    #[tool(
        description = "After the owner confirmed and you published a progress report as an Artifact: record the link and scope."
    )]
    async fn report_published(
        &self,
        Parameters(args): Parameters<Args<ReportPublishedInput>>,
    ) -> CallToolResult {
        text(self.do_report_published(&args))
    }

    #[tool(
        description = "Used by the progress card for the owner's clicks. Not for the agent: it needs the card's nonce and refuses without it.",
        meta = ui_meta(serde_json::json!({ "visibility": ["app"] }))
    )]
    async fn owner_action(
        &self,
        Parameters(args): Parameters<Args<OwnerActionInput>>,
    ) -> CallToolResult {
        text(self.do_owner_action(&args))
    }
}

/// The first 7 characters of a SHA.
fn short(sha: &str) -> String {
    sha.chars().take(7).collect()
}

/// `text` as a sentence: a full stop added unless it already ends with one.
fn sentence(text: &str) -> String {
    if text.ends_with(['.', '!', '?']) {
        text.to_string()
    } else {
        format!("{text}.")
    }
}

impl Bridge {
    fn do_claim(&self, args: &Args<ClaimTaskInput>) -> Result<String, String> {
        let mut store = self.open()?;
        let session = self.caller(&store, "claim_task", &args.raw)?;
        let input = args.parsed.as_ref().map_err(Clone::clone)?;
        let task = match (&input.id, &input.new_task) {
            (Some(id), None) => ClaimTarget::Existing(id.clone()),
            (None, Some(t)) => ClaimTarget::New {
                parent: t.parent.clone(),
                title: t.title.clone(),
                estimate_min: t.estimate_min,
            },
            _ => return Err("give exactly one of id or new_task".into()),
        };
        let req = ClaimRequest {
            session_id: session.session_id.clone(),
            repo_id: session.repo_id.clone(),
            task,
            mode: input.mode,
            confidence: input.confidence,
            actor: Actor::default(),
        };
        let out = claim::claim(&mut store, &req).map_err(msg)?;
        let title = claim::task_title(store.conn(), &session.repo_id, &out.task_id)
            .map_err(msg)?
            .unwrap_or_default();
        let mut reply = format!("Claimed {} {title}.", out.task_id);
        if let Some(prev) = out.released {
            let _ = write!(reply, " Released {prev}.");
        }
        Ok(reply)
    }

    fn do_claim_done(&self, args: &Args<ClaimDoneInput>) -> Result<String, String> {
        let mut store = self.open()?;
        let session = self.caller(&store, "claim_done", &args.raw)?;
        let input = args.parsed.as_ref().map_err(Clone::clone)?;
        let task = checkpoint::claim_done(
            &mut store,
            &ClaimDoneRequest {
                session_id: session.session_id.clone(),
                task_id: input.id.clone(),
                evidence: input.evidence.clone(),
                files: input.files.clone(),
                symbols: input.symbols.clone(),
                actor: Actor::default(),
            },
        )
        .map_err(msg)?;
        let mut reply = format!("Marked {task} claimed done.");
        // L1 right away when the files are committed (a git failure just skips it).
        let wt = worktree_of(&session);
        match verify::check_l1(store.conn(), &wt, &session.repo_id, &task) {
            Ok(outcome @ L1Outcome::Passed { .. }) => {
                if let Some(draft) =
                    verify::l1_draft(&session.repo_id, &session.session_id, &task, &outcome)
                        .map_err(msg)?
                {
                    store.append(draft).map_err(msg)?;
                }
                if let L1Outcome::Passed { sha, .. } = &outcome {
                    let _ = write!(reply, " L1: the code exists at {}.", short(sha));
                }
            }
            Ok(L1Outcome::NotCommitted { .. }) => {
                reply.push_str(" L1 is checked when you commit the checkpoint.");
            }
            Ok(L1Outcome::Missing {
                sha,
                paths,
                symbols,
            }) => {
                let _ = write!(
                    reply,
                    " L1 check: {} not found at {}. Fix the evidence with claim_done again.",
                    verify::missing_text(&paths, &symbols),
                    short(&sha)
                );
            }
            Ok(L1Outcome::NoEvidence) => {
                reply.push_str(
                    " Name the files that show the work (files) so bridge-screen can check L1.",
                );
            }
            Err(_) => {}
        }
        reply.push_str(
            " Commit a checkpoint before you stop, then leave a handoff_note if work remains in flight.",
        );
        // Task completion: the moment to capture decisions (spec §8.6); `claim_done`
        // recorded the ask.
        if checkpoint::asks_for_decisions(store.conn(), &session.repo_id, &task).map_err(msg)? {
            let _ = write!(reply, " {}", decision::DECISION_ASK);
        }
        Ok(reply)
    }

    fn do_log_decision(&self, args: &Args<LogDecisionInput>) -> Result<String, String> {
        let mut store = self.open()?;
        let session = self.caller(&store, "log_decision", &args.raw)?;
        let input = args.parsed.as_ref().map_err(Clone::clone)?;
        let logged = decision::log_decision(
            &mut store,
            &decision::LogRequest {
                session_id: session.session_id.clone(),
                task: match &input.task {
                    None => decision::DecisionTask::Active,
                    Some(Nullable(None)) => decision::DecisionTask::RepoWide,
                    Some(Nullable(Some(t))) => decision::DecisionTask::Task(t.clone()),
                },
                what: input.what.clone(),
                why: input.why.clone(),
                alternatives: input.alternatives.clone(),
                supersedes: input.supersedes.clone(),
                actor: Actor::default(),
            },
        )
        .map_err(msg)?;
        Ok(logged.reply())
    }

    fn do_verify(&self, args: &Args<VerifyInput>) -> Result<String, String> {
        let mut store = self.open()?;
        let session = self.caller(&store, "verify", &args.raw)?;
        let input = args.parsed.as_ref().map_err(Clone::clone)?;
        let config = Config::load(&self.home.config_path()).unwrap_or_default();
        let out = verify::verify(
            &mut store,
            &verify::VerifyRequest {
                session_id: session.session_id.clone(),
                task_id: input.id.clone(),
                level: input.level,
                evidence: input.evidence.clone(),
                exit_code: input.exit_code,
                summary: input.summary.clone(),
                owner_said: input.owner_said.clone(),
                actor: Actor::default(),
            },
            &worktree_of(&session),
            std::time::Duration::from_secs(config.verify_timeout_s),
        )
        .map_err(msg)?;
        Ok(out.reply)
    }

    fn do_resolve_leftover(&self, args: &Args<ResolveLeftoverInput>) -> Result<String, String> {
        let mut store = self.open()?;
        let session = self.caller(&store, "resolve_leftover", &args.raw)?;
        let input = args.parsed.as_ref().map_err(Clone::clone)?;
        let res = recovery::resolve(
            &mut store,
            &session.repo_id,
            &self.home.snapshots_dir(&session.repo_id),
            &input.ids,
            input.action,
            &Caller::Agent {
                session_id: session.session_id.clone(),
            },
            &recovery::Takeover::new(
                jiff::Timestamp::now(),
                Config::load(&self.home.config_path())
                    .unwrap_or_default()
                    .crash_takeover_hours,
            ),
        )
        .map_err(msg)?;
        bridge_screen_core::owner::resolution_text(store.conn(), &session.repo_id, &res)
            .map_err(msg)
    }

    fn do_handoff_readback(&self, args: &Args<HandoffReadbackInput>) -> Result<String, String> {
        let mut store = self.open()?;
        let session = self.caller(&store, "handoff_readback", &args.raw)?;
        let input = args.parsed.as_ref().map_err(Clone::clone)?;
        let check = handoff::read_back(
            &mut store,
            &session.session_id,
            &input.goal,
            &input.in_flight,
            &input.next_action,
            &Actor::default(),
        )
        .map_err(msg)?;
        Ok(check.reply)
    }

    fn do_handoff_note(&self, args: &Args<HandoffNoteInput>) -> Result<String, String> {
        let mut store = self.open()?;
        let session = self.caller(&store, "handoff_note", &args.raw)?;
        let input = args.parsed.as_ref().map_err(Clone::clone)?;
        let task = handoff::leave_note(
            &mut store,
            &session.session_id,
            &input.in_flight,
            &input.if_then,
            &Actor::default(),
        )
        .map_err(msg)?;
        let mut reply = match &task {
            Some(t) => {
                let title = claim::task_title(store.conn(), &session.repo_id, t)
                    .map_err(msg)?
                    .unwrap_or_default();
                format!("Handoff note saved for {t} {title}.")
            }
            None => "Handoff note saved (no active task).".to_string(),
        };
        // A clean session end goes through the handoff note: ask for decisions (§8.6).
        let logged = match &task {
            Some(t) => decision::list(store.conn(), &session.repo_id, Some(t), true),
            None => decision::by_session(store.conn(), &session.session_id),
        }
        .map_err(msg)?;
        if logged.is_empty() {
            let _ = write!(reply, " {}", decision::DECISION_ASK);
            store
                .append(decision::asked_draft(
                    &session.repo_id,
                    &session.session_id,
                    "handoff_note",
                ))
                .map_err(msg)?;
        }
        Ok(reply)
    }

    fn do_plan_edit(&self, args: &Args<PlanEditInput>) -> Result<String, String> {
        let mut store = self.open()?;
        let session = self.caller(&store, "plan_edit", &args.raw)?;
        let input = args.parsed.as_ref().map_err(Clone::clone)?;
        let ops = input
            .ops
            .iter()
            .enumerate()
            .map(|(i, op)| {
                serde_json::from_value::<PlanOp>(op.clone()).map_err(|e| format!("op {i}: {e}"))
            })
            .collect::<Result<Vec<_>, _>>()?;
        let ctx = EditContext {
            repo_id: session.repo_id.clone(),
            session_id: session.session_id.clone(),
            actor: Actor::default(),
            reason: input.reason.clone(),
            replan: input.replan,
        };
        let out = bridge_screen_core::plan::plan_edit(&mut store, &ctx, &ops).map_err(msg)?;
        let mut reply = format!("Applied {} operation(s).", ops.len());
        if !out.added_ids.is_empty() {
            let _ = write!(reply, " Added: {}.", out.added_ids.join(", "));
        }
        Ok(reply)
    }

    fn do_start_detour(&self, args: &Args<StartDetourInput>) -> Result<String, String> {
        let mut store = self.open()?;
        let session = self.caller(&store, "start_detour", &args.raw)?;
        let input = args.parsed.as_ref().map_err(Clone::clone)?;
        let started = tangent::start_detour(
            &mut store,
            &StartRequest {
                session_id: session.session_id.clone(),
                reason: input.reason.clone(),
                resume_cue: input.resume_cue.clone(),
                blocks: input.blocks.clone(),
                requested: input.requested_by_owner,
                actor: Actor::default(),
            },
        )
        .map_err(msg)?;
        let d = &started.detour;
        let head = format!("Detour {} opened: {}.", d.label(), d.reason);
        Ok(match &started.suspended {
            Some(item) => {
                let what =
                    tangent::item_display(store.conn(), &session.repo_id, item).map_err(msg)?;
                format!(
                    "{head} Suspended {what} — on return: {} If you stop before returning, leave a handoff_note.",
                    sentence(&item.resume_cue)
                )
            }
            None => format!(
                "{head} Nothing to suspend (no active task); close it with close_detour when done."
            ),
        })
    }

    fn do_close_detour(&self, args: &Args<CloseDetourInput>) -> Result<String, String> {
        let mut store = self.open()?;
        let session = self.caller(&store, "close_detour", &args.raw)?;
        let input = args.parsed.as_ref().map_err(Clone::clone)?;
        let closed = tangent::close_detour(
            &mut store,
            &CloseRequest {
                session_id: session.session_id.clone(),
                detour: input.detour.clone(),
                outcome: input.outcome,
                note: input.note.clone(),
                actor: Actor::default(),
            },
        )
        .map_err(msg)?;
        if !closed.was_focus {
            return Ok(format!(
                "Detour {} closed. Continue with your current work.",
                closed.detour.label()
            ));
        }
        Ok(match &closed.resumed {
            Some(item) => {
                let what =
                    tangent::item_display(store.conn(), &session.repo_id, item).map_err(msg)?;
                format!(
                    "Detour closed. Return to {what}. On return: {}",
                    sentence(&item.resume_cue)
                )
            }
            None => "Detour closed. No suspended item; continue with your task.".to_string(),
        })
    }

    fn do_resume(&self, args: &Args<ResumeInput>) -> Result<String, String> {
        let mut store = self.open()?;
        let session = self.caller(&store, "resume", &args.raw)?;
        let input = args.parsed.as_ref().map_err(Clone::clone)?;
        let item = tangent::resume(
            &mut store,
            &session.session_id,
            &input.item_id,
            &Actor::default(),
        )
        .map_err(msg)?;
        let what = tangent::item_display(store.conn(), &session.repo_id, &item).map_err(msg)?;
        Ok(format!(
            "Resumed {what}. On return: {}",
            sentence(&item.resume_cue)
        ))
    }

    fn do_label_unit(&self, args: &Args<LabelUnitInput>) -> Result<String, String> {
        let mut store = self.open()?;
        let session = self.caller(&store, "label_unit", &args.raw)?;
        let input = args.parsed.as_ref().map_err(Clone::clone)?;
        let config = Config::load(&self.home.config_path()).unwrap_or_default();
        let th = tangent::thresholds(store.conn(), &session.repo_id, &config).map_err(msg)?;
        let unit_no = tangent::label_unit(
            &mut store,
            &LabelRequest {
                session_id: session.session_id.clone(),
                unit_no: None,
                label: input.label,
                source: LabelSource::Agent,
                why: Some(input.why.clone()),
            },
            &th,
            &Actor::default(),
        )
        .map_err(msg)?;
        Ok(format!(
            "Labelled unit {unit_no} as {}.",
            input.label.display()
        ))
    }

    fn do_promote_detour(&self, args: &Args<PromoteDetourInput>) -> Result<String, String> {
        let mut store = self.open()?;
        let session = self.caller(&store, "promote_detour", &args.raw)?;
        let input = args.parsed.as_ref().map_err(Clone::clone)?;
        let out = tangent::promote_detour(
            &mut store,
            &session.session_id,
            &input.parent,
            &input.title,
            input.estimate_min,
            &Actor::default(),
        )
        .map_err(msg)?;
        Ok(format!(
            "Detour {} is now task {}.",
            out.detour.label(),
            out.task_id
        ))
    }

    /// A fresh nonce for the card; expired ones are dropped.
    fn mint_nonce(&self) -> Result<String, String> {
        let mut bytes = [0_u8; 16];
        getrandom::fill(&mut bytes).map_err(msg)?;
        let nonce: String = bytes.iter().fold(String::new(), |mut h, b| {
            let _ = write!(h, "{b:02x}");
            h
        });
        let mut nonces = self.nonces.lock().map_err(msg)?;
        nonces.retain(|_, at| at.elapsed() < NONCE_TTL);
        nonces.insert(nonce.clone(), Instant::now());
        Ok(nonce)
    }

    fn nonce_valid(&self, nonce: &str) -> bool {
        self.nonces
            .lock()
            .is_ok_and(|n| n.get(nonce).is_some_and(|at| at.elapsed() < NONCE_TTL))
    }

    fn do_show_progress(&self, args: &Args<ShowProgressInput>) -> Result<CallToolResult, String> {
        let input = args.parsed.as_ref().map_err(Clone::clone)?;
        let store = self.open()?;
        let cfg = Config::load(&self.home.config_path()).unwrap_or_default();
        let mut glance = view::glance(store.conn(), &cfg, jiff::Timestamp::now()).map_err(msg)?;
        if input.scope == Some(Scope::Repo) {
            let session = self.caller(&store, "show_progress", &args.raw)?;
            glance = glance.for_repo(&session.repo_id);
        }
        // The text view for hosts without MCP Apps; the dashboard's address without its
        // token (`bridge-screen ui` prints the link).
        let opts = crate::status::Options {
            color: false,
            dashboard: crate::ui::running(&self.home).map(|l| l.address()),
        };
        let text = crate::status::render(&glance, &opts);
        let mut result = CallToolResult::success(vec![ContentBlock::text(text)]);
        result.structured_content = Some(serde_json::to_value(&glance).map_err(msg)?);
        let mut meta = serde_json::Map::new();
        meta.insert(NONCE_KEY.into(), Value::from(self.mint_nonce()?));
        result.meta = Some(MetaObject::from(meta));
        Ok(result)
    }

    fn do_progress_report(&self, args: &Args<ProgressReportInput>) -> Result<String, String> {
        let input = args.parsed.as_ref().map_err(Clone::clone)?;
        let store = self.open()?;
        let cfg = Config::load(&self.home.config_path()).unwrap_or_default();
        let glance = view::glance(store.conn(), &cfg, jiff::Timestamp::now()).map_err(msg)?;
        let (glance, label) = match input.scope.unwrap_or(Scope::Repo) {
            Scope::All => (glance, "all".to_string()),
            Scope::Repo => {
                let session = self.caller(&store, "progress_report", &args.raw)?;
                let name = bridge_screen_core::store::repo_name(store.conn(), &session.repo_id)
                    .map_err(msg)?
                    .unwrap_or_else(|| session.repo_id.clone());
                let safe: String = name
                    .chars()
                    .map(|c| {
                        if c.is_ascii_alphanumeric() || c == '-' {
                            c
                        } else {
                            '_'
                        }
                    })
                    .collect();
                (glance.for_repo(&session.repo_id), safe)
            }
        };
        crate::report::write(&store, &glance, &label)
            .map(|r| r.summary)
            .map_err(msg)
    }

    fn do_report_published(&self, args: &Args<ReportPublishedInput>) -> Result<String, String> {
        let input = args.parsed.as_ref().map_err(Clone::clone)?;
        let url = input.url.trim();
        if !(url.starts_with("https://") || url.starts_with("http://"))
            || url.len() > 500
            || url.contains(char::is_whitespace)
        {
            return Err("url must be the published page's http(s) link".into());
        }
        let mut store = self.open()?;
        let session = self.caller(&store, "report_published", &args.raw)?;
        let scope = match input.scope {
            Scope::Repo => "repo",
            Scope::All => "all",
        };
        store
            .append(bridge_screen_core::store::EventDraft::new(
                &session.repo_id,
                &session.session_id,
                "report_published",
                serde_json::json!({ "url": url, "scope": scope }),
            ))
            .map_err(msg)?;
        Ok(format!("Recorded the published report: {url}"))
    }

    fn do_owner_action(&self, args: &Args<OwnerActionInput>) -> Result<String, String> {
        let input = args.parsed.as_ref().map_err(Clone::clone)?;
        if !self.nonce_valid(&input.nonce) {
            return Err(
                "refused: owner_action answers clicks on the progress card only (its nonce is missing, wrong or expired); nothing was recorded"
                    .into(),
            );
        }
        let mut store = self.open()?;
        let cfg = Config::load(&self.home.config_path()).unwrap_or_default();
        let ctx = owner::ActCtx {
            home: &self.home,
            cfg: &cfg,
            now: jiff::Timestamp::now(),
            dir: None,
        };
        owner::act(&mut store, &ctx, &input.action).map_err(msg)
    }

    fn do_status(&self, args: &Args<StatusInput>) -> Result<String, String> {
        let store = self.open()?;
        let session = self.caller(&store, "status", &args.raw)?;
        args.parsed.as_ref().map_err(Clone::clone)?;
        let cfg = Config::load(&self.home.config_path()).unwrap_or_default();
        agent_status(&store, &session, &cfg).map_err(msg)
    }
}

// The macro generates `async fn`s that return ready futures; that is rmcp's shape, not ours.
#[allow(clippy::unused_async_trait_impl)]
#[tool_handler(router = self.tool_router)]
impl ServerHandler for Bridge {
    fn get_info(&self) -> ServerConfig {
        ServerConfig::new(
            ServerCapabilities::builder()
                .enable_tools()
                .enable_resources()
                .build(),
        )
            .with_server_info(Implementation::new("bridge-screen", bridge_screen_core::VERSION))
            .with_instructions(
                "bridge-screen tracks this session's task and time. Claim your task with claim_task before working; use plan_edit for plan changes; status shows where things stand.",
            )
    }

    async fn list_resources(
        &self,
        _request: Option<PaginatedRequestParams>,
        _context: RequestContext<RoleServer>,
    ) -> Result<ListResourcesResult, ErrorData> {
        serde_json::from_value(serde_json::json!({
            "resources": [{
                "uri": PROGRESS_URI,
                "name": "progress",
                "title": "bridge-screen progress",
                "description": "Needs-you items, project progress and live sessions, with the owner's buttons.",
                "mimeType": APP_MIME,
            }]
        }))
        .map_err(|e| ErrorData::internal_error(e.to_string(), None))
    }

    async fn read_resource(
        &self,
        request: ReadResourceRequestParams,
        _context: RequestContext<RoleServer>,
    ) -> Result<ReadResourceResponse, ErrorData> {
        if request.uri != PROGRESS_URI {
            return Err(ErrorData::resource_not_found(
                format!("no resource {}", request.uri),
                None,
            ));
        }
        let html = CardDist::get("card.html")
            .map(|f| String::from_utf8_lossy(&f.data).into_owned())
            .unwrap_or_default();
        let result: ReadResourceResult = serde_json::from_value(serde_json::json!({
            "contents": [{
                "uri": PROGRESS_URI,
                "mimeType": APP_MIME,
                "text": html,
                // No outside origins at all: the card is self-contained.
                "_meta": { "ui": { "prefersBorder": true, "csp": {
                    "connectDomains": [], "resourceDomains": [] } } }
            }]
        }))
        .map_err(|e| ErrorData::internal_error(e.to_string(), None))?;
        Ok(result.into())
    }
}

/// The progress card's resource (MCP Apps, spec §10.5).
pub const PROGRESS_URI: &str = "ui://bridge-screen/progress";
/// MCP Apps' HTML resource type.
pub const APP_MIME: &str = "text/html;profile=mcp-app";
/// Where `show_progress` puts the card's nonce in the result's `_meta`.
pub const NONCE_KEY: &str = "bridge-screen/nonce";
/// How long a card's nonce is good for (many uses).
const NONCE_TTL: Duration = Duration::from_mins(30);

/// The single-file card build (or a placeholder page), embedded like the dashboard.
#[derive(rust_embed::RustEmbed)]
#[folder = "$BRIDGE_CARD_DIST"]
struct CardDist;

/// A tool's `_meta.ui`.
fn ui_meta(ui: Value) -> MetaObject {
    let mut m = serde_json::Map::new();
    m.insert("ui".into(), ui);
    MetaObject::from(m)
}

/// The newest live session whose working directory is inside `dir`.
fn newest_session_in(store: &Store, dir: &Path) -> Result<Option<Session>, String> {
    let real_dir = std::fs::canonicalize(dir).ok();
    let inside = |cwd: &str| {
        let cwd = Path::new(cwd);
        cwd.starts_with(dir)
            || real_dir
                .as_ref()
                .is_some_and(|real| std::fs::canonicalize(cwd).is_ok_and(|c| c.starts_with(real)))
    };
    let mut live: Vec<Session> = session::list_live(store.conn(), None)
        .map_err(msg)?
        .into_iter()
        .filter(|s| inside(&s.cwd))
        .collect();
    live.sort_by_cached_key(|s| std::cmp::Reverse(s.last_event_at.parse::<jiff::Timestamp>().ok()));
    Ok(live.into_iter().next())
}

/// The handoff part of `status`: a pending or missed checkpoint, the read-back, and the
/// repo's open leftovers.
fn handoff_status(
    conn: &rusqlite::Connection,
    s: &Session,
    out: &mut String,
) -> bridge_screen_core::error::Result<()> {
    if let Some(reason) = &s.checkpoint_pending {
        if s.checkpoint_missed == 0 {
            let _ = writeln!(
                out,
                "Checkpoint pending ({reason}): commit as `wip(bridge): <id> <title>` before you stop."
            );
        } else {
            let _ = writeln!(
                out,
                "Checkpoint missed ({reason}): commit your work now as `wip(bridge): <id> <title>`."
            );
        }
    }
    if s.readback == handoff::ReadbackState::Pending {
        out.push_str("Read-back pending: call handoff_readback.\n");
    }
    let left: Vec<handoff::Leftover> = handoff::leftovers(conn, &s.repo_id, true)?;
    if !left.is_empty() {
        let list: Vec<String> = left
            .iter()
            .take(5)
            .map(|l| {
                let only = if l.state == handoff::LeftoverState::ReportOnly {
                    " (report-only)"
                } else {
                    ""
                };
                format!(
                    "{} {} ({}) → {}{only}",
                    l.leftover_id,
                    l.path,
                    l.task_id.as_deref().unwrap_or("no task"),
                    l.suggested.as_str()
                )
            })
            .collect();
        let more = if left.len() > 5 {
            format!(" (+{} more)", left.len() - 5)
        } else {
            String::new()
        };
        let _ = writeln!(
            out,
            "Leftovers ({}): {}{more} — resolve with resolve_leftover",
            left.len(),
            list.join(" · ")
        );
    }
    Ok(())
}

/// Where a task stands on the verification ladder and the next step (spec §8.5).
fn ladder_step(status: NodeStatus) -> Option<&'static str> {
    Some(match status {
        NodeStatus::Claimed => {
            "claimed (L0) — next: commit the files you named, then run tests and verify level 2"
        }
        NodeStatus::L1 => "L1 (code exists) — next: run tests, verify level 2",
        NodeStatus::L2 => "L2 (tests pass) — next: drive it end to end, verify level 3",
        NodeStatus::L3 => "L3 (end-to-end) — next: ask the owner, verify level 4 with their words",
        NodeStatus::L4 => "L4 (the owner confirmed)",
        NodeStatus::Failed => "failed — fix it, commit, claim_done again",
        _ => return None,
    })
}

/// Compact state for the agent.
fn agent_status(
    store: &Store,
    s: &Session,
    cfg: &Config,
) -> bridge_screen_core::error::Result<String> {
    let conn = store.conn();
    let short: String = s.session_id.chars().take(6).collect();
    let state = match s.state {
        SessionState::Working => "working",
        SessionState::Idle => "idle",
        SessionState::NeedsOwner => "waiting on the owner",
        SessionState::Ended => "ended",
    };
    let units = if s.unit_count == 1 { "unit" } else { "units" };
    let mut out = format!(
        "Session {short}: {state} · {} {units} · {} min active\n",
        s.unit_count,
        s.active_secs / 60
    );
    match &s.active_task {
        Some(id) => {
            let title = claim::task_title(conn, &s.repo_id, id)?.unwrap_or_default();
            let _ = writeln!(out, "Active task: {id} {title}");
            if let Some(node) = get_node(conn, &s.repo_id, id)?
                && let Some(step) = ladder_step(node.status)
            {
                let _ = writeln!(out, "Verification: {step}");
            }
            let live = decision::list(conn, &s.repo_id, Some(id), false)?;
            if let Some(last) = live.last() {
                let _ = writeln!(
                    out,
                    "Decisions on {id}: {} (latest: {} {})",
                    live.len(),
                    last.decision_id,
                    last.what
                );
            }
        }
        None => out.push_str("Active task: none — claim one with claim_task.\n"),
    }
    cost_status(conn, s, cfg, &mut out)?;
    handoff_status(conn, s, &mut out)?;
    let pending = verify::pending_claims(conn, &s.session_id, jiff::Timestamp::MAX)?;
    if !pending.is_empty() {
        let list: Vec<String> = pending
            .iter()
            .map(|(t, level)| format!("{t} (L{level})"))
            .collect();
        let _ = writeln!(out, "Unverified: {} — verify them", list.join(" · "));
    }
    let open = tangent::open_detours(conn, &s.session_id)?;
    if !open.is_empty() {
        let list: Vec<String> = open
            .iter()
            .map(|d| {
                let current = s.current_detour.as_deref() == Some(d.detour_id.as_str());
                format!(
                    "{} {}{}",
                    d.label(),
                    d.reason,
                    if current { " (current)" } else { "" }
                )
            })
            .collect();
        let _ = writeln!(out, "Open detours: {}", list.join(" · "));
    }
    for item in tangent::suspended(conn, &s.session_id)? {
        let what = tangent::item_display(conn, &s.repo_id, &item)?;
        let _ = writeln!(out, "Suspended: {what} — on return: {}", item.resume_cue);
    }
    plan_status(conn, s, &mut out)?;
    Ok(out)
}

/// The goal with its rolled-up status, the milestones, the forecast and what's next.
fn plan_status(
    conn: &rusqlite::Connection,
    s: &Session,
    out: &mut String,
) -> bridge_screen_core::error::Result<()> {
    match get_node(conn, &s.repo_id, "G")?.filter(|g| g.status != NodeStatus::Dropped) {
        Some(goal) => {
            let tree = load_tree(conn, &s.repo_id, false)?;
            let rolled = bridge_screen_core::plan::rollups(&tree);
            let _ = writeln!(
                out,
                "Goal: {} ({})",
                goal.title,
                crate::words::status(bridge_screen_core::plan::shown_status(&goal, &rolled))
            );
            let milestones: Vec<String> = tree
                .iter()
                .filter(|n| n.kind == NodeKind::Milestone)
                .map(|n| {
                    format!(
                        "{} {}",
                        n.id,
                        crate::words::status(bridge_screen_core::plan::shown_status(n, &rolled))
                    )
                })
                .collect();
            if !milestones.is_empty() {
                let _ = writeln!(out, "Milestones: {}", milestones.join(" · "));
            }
            let f = view::forecast_row(conn, &s.repo_id, jiff::Timestamp::now())?;
            if f.compact.is_some() {
                let _ = writeln!(out, "Forecast: {} ({})", f.text, f.basis);
            } else {
                let _ = writeln!(out, "Forecast: {}", f.text);
            }
            let next: Vec<String> = tree
                .into_iter()
                .filter(|n| {
                    matches!(n.kind, NodeKind::Task | NodeKind::Subtask)
                        && n.status == NodeStatus::Todo
                })
                .take(3)
                .map(|n| format!("{} {}", n.id, n.title))
                .collect();
            if next.is_empty() {
                out.push_str("Next: nothing left to do in the plan.");
            } else {
                let _ = write!(out, "Next: {}", next.join(" · "));
            }
        }
        None => out.push_str("No plan yet — ask the owner whether to create one."),
    }
    Ok(())
}

/// `Cost so far: this session $1.20 (340k tokens) · task 2.3 $0.80 (200k tokens)`.
fn cost_status(
    conn: &rusqlite::Connection,
    s: &Session,
    cfg: &Config,
    out: &mut String,
) -> bridge_screen_core::error::Result<()> {
    use bridge_screen_core::telemetry::Scope;
    let mut parts = Vec::new();
    if let Some(c) = view::cost_row(conn, &Scope::Session(s.session_id.clone()), cfg)? {
        parts.push(format!(
            "this session {} ({} tokens)",
            c.text, c.tokens_text
        ));
    }
    if let Some(task) = &s.active_task {
        let scope = Scope::Task {
            repo: s.repo_id.clone(),
            id: task.clone(),
        };
        if let Some(c) = view::cost_row(conn, &scope, cfg)? {
            parts.push(format!("task {task} {} ({} tokens)", c.text, c.tokens_text));
        }
    }
    if !parts.is_empty() {
        let _ = writeln!(out, "Cost so far: {}", parts.join(" · "));
    }
    Ok(())
}

/// Serve MCP over stdio until the client disconnects.
pub fn run() -> Result<()> {
    let home = Home::resolve()?;
    let project_dir = std::env::var_os("CLAUDE_PROJECT_DIR")
        .filter(|v| !v.is_empty())
        .map(PathBuf::from);
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()?;
    runtime.block_on(async move {
        let service = Bridge::new(home, project_dir)
            .serve(rmcp::transport::stdio())
            .await?;
        service.waiting().await?;
        Ok(())
    })
}

#[cfg(test)]
mod tests {
    use super::{Args, Bridge, NONCE_TTL, OwnerActionInput};
    use bridge_screen_core::paths::Home;
    use std::time::{Duration, Instant};

    #[test]
    fn an_expired_card_nonce_is_refused() {
        let home = tempfile::tempdir().unwrap();
        let bridge = Bridge::new(Home::at(home.path()), None);
        let fresh = bridge.mint_nonce().unwrap();
        assert!(bridge.nonce_valid(&fresh));
        let old = Instant::now()
            .checked_sub(NONCE_TTL + Duration::from_secs(1))
            .unwrap();
        bridge.nonces.lock().unwrap().insert("old".into(), old);
        let raw = serde_json::json!({
            "action": {"kind": "reset_drift", "repo": "r"},
            "nonce": "old"
        });
        let args: Args<OwnerActionInput> = serde_json::from_value(raw).unwrap();
        let refused = bridge.do_owner_action(&args).unwrap_err();
        assert!(refused.starts_with("refused"), "{refused}");
        // Nothing was even opened.
        assert!(!home.path().join("bridge-screen.db").exists());
    }
}
