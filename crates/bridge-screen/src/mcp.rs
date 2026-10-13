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
use bridge_screen_core::claim::{self, ClaimMode, ClaimRequest, ClaimTarget};
use bridge_screen_core::paths::Home;
use bridge_screen_core::plan::{EditContext, NodeKind, NodeStatus, PlanOp, get_node, load_tree};
use bridge_screen_core::session::{self, Session, SessionState};
use bridge_screen_core::store::{Actor, Store};
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
