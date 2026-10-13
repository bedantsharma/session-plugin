//! `bridge-screen mcp`: the agent's tools over MCP stdio (spec §11.2).
//!
//! Claude Code starts one server per session but doesn't say which session it is. Each
//! call is matched to the binding the `PreToolUse` hook recorded moments earlier (see
//! `bridge_screen_core::binding`); without one, the newest live session inside
//! `CLAUDE_PROJECT_DIR` is used.

use std::borrow::Cow;
use std::fmt::Write as _;
use std::path::{Path, PathBuf};

use anyhow::Result;
use bridge_screen_core::binding;
use bridge_screen_core::checkpoint::{self, ClaimDoneRequest};
use bridge_screen_core::claim::{self, ClaimMode, ClaimRequest, ClaimTarget};
use bridge_screen_core::config::Config;
use bridge_screen_core::handoff::{self, LeftoverAction};
use bridge_screen_core::paths::Home;
use bridge_screen_core::plan::{EditContext, NodeKind, NodeStatus, PlanOp, get_node, load_tree};
use bridge_screen_core::recovery::{self, Caller, Resolution};
use bridge_screen_core::session::{self, Session, SessionState};
use bridge_screen_core::store::{Actor, Store};
use bridge_screen_core::tangent::{
    self, CloseRequest, DetourOutcome, Label, LabelRequest, LabelSource, StartRequest,
};
use rmcp::handler::server::router::tool::ToolRouter;
use rmcp::handler::server::wrapper::Parameters;
use rmcp::model::{CallToolResult, ContentBlock, Implementation, ServerCapabilities, ServerConfig};
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
    /// "drift" if it did neither.
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

#[derive(Clone)]
pub struct Bridge {
    home: Home,
    project_dir: Option<PathBuf>,
    tool_router: ToolRouter<Self>,
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
        }
    }

    fn open(&self) -> Result<Store, String> {
        self.home.ensure().map_err(msg)?;
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
        description = "Say a task is done when you finish it (verification level L0), with evidence: the tests you ran and their result. A checkpoint commit is then due before you stop."
    )]
    async fn claim_done(
        &self,
        Parameters(args): Parameters<Args<ClaimDoneInput>>,
    ) -> CallToolResult {
        text(self.do_claim_done(&args))
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
        description = "Open a detour before tangent work that isn't your task but blocks it (a broken build, a missing dependency). Give a one-line reason and a resume_cue: the next action on the work you are suspending and the file or test it concerns. Close it with close_detour when done."
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
        description = "Label your last unit of work when bridge-screen asks: on_task, detour or drift, with a one-line why. Be honest: drift is information for the owner, not blame. Calling it again corrects the label."
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
                actor: Actor::default(),
            },
        )
        .map_err(msg)?;
        Ok(format!(
            "Marked {task} claimed done. Commit a checkpoint before you stop, then leave a handoff_note if work remains in flight."
        ))
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
        )
        .map_err(msg)?;
        resolution_text(&store, &session.repo_id, &res).map_err(msg)
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
        Ok(match task {
            Some(t) => {
                let title = claim::task_title(store.conn(), &session.repo_id, &t)
                    .map_err(msg)?
                    .unwrap_or_default();
                format!("Handoff note saved for {t} {title}.")
            }
            None => "Handoff note saved (no active task).".to_string(),
        })
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

    fn do_status(&self, args: &Args<StatusInput>) -> Result<String, String> {
        let store = self.open()?;
        let session = self.caller(&store, "status", &args.raw)?;
        args.parsed.as_ref().map_err(Clone::clone)?;
        agent_status(&store, &session).map_err(msg)
    }
}

// The macro generates `async fn`s that return ready futures; that is rmcp's shape, not ours.
#[allow(clippy::unused_async_trait_impl)]
#[tool_handler(router = self.tool_router)]
impl ServerHandler for Bridge {
    fn get_info(&self) -> ServerConfig {
        ServerConfig::new(ServerCapabilities::builder().enable_tools().build())
            .with_server_info(Implementation::new("bridge-screen", bridge_screen_core::VERSION))
            .with_instructions(
                "bridge-screen tracks this session's task and time. Claim your task with claim_task before working; use plan_edit for plan changes; status shows where things stand.",
            )
    }
}

/// What a leftover resolution did, in one line.
pub fn resolution_text(
    store: &Store,
    repo_id: &str,
    res: &Resolution,
) -> bridge_screen_core::error::Result<String> {
    let n = res.ids.len();
    let what = format!("{n} leftover{}", if n == 1 { "" } else { "s" });
    let short = |sha: &str| sha.chars().take(8).collect::<String>();
    Ok(match res.action {
        LeftoverAction::Adopt => match &res.commit {
            Some(sha) => format!("Adopted {what}: committed {}.", short(sha)),
            None => format!("Adopted {what}: nothing was left uncommitted."),
        },
        LeftoverAction::Undo => match &res.stash {
            Some(st) => format!(
                "Undid {what}: stashed as {} (`git stash apply {}` in {} brings them back).",
                short(&st.sha),
                st.sha,
                res.worktree
            ),
            None => format!("Undid {what}: nothing was left uncommitted."),
        },
        LeftoverAction::Resume => match &res.claimed {
            Some(task) => {
                let title = claim::task_title(store.conn(), repo_id, task)?.unwrap_or_default();
                format!(
                    "Resumed {task} {title}: {what} in {} are yours to finish.",
                    res.worktree
                )
            }
            None => format!(
                "Marked {what} to resume: the next session in {} continues them.",
                res.worktree
            ),
        },
    })
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

/// Compact state for the agent.
fn agent_status(store: &Store, s: &Session) -> bridge_screen_core::error::Result<String> {
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
        }
        None => out.push_str("Active task: none — claim one with claim_task.\n"),
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
    match get_node(conn, &s.repo_id, "G")?.filter(|g| g.status != NodeStatus::Dropped) {
        Some(goal) => {
            let _ = writeln!(out, "Goal: {}", goal.title);
            let next: Vec<String> = load_tree(conn, &s.repo_id, false)?
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
    Ok(out)
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
