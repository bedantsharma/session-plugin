use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum NodeKind {
    Goal,
    Milestone,
    Task,
    Subtask,
}

impl NodeKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Goal => "goal",
            Self::Milestone => "milestone",
            Self::Task => "task",
            Self::Subtask => "subtask",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "goal" => Some(Self::Goal),
            "milestone" => Some(Self::Milestone),
            "task" => Some(Self::Task),
            "subtask" => Some(Self::Subtask),
            _ => None,
        }
    }

    /// Goal → milestone | task; milestone → task; task → subtask.
    pub fn allows_child(self, child: Self) -> bool {
        matches!(
            (self, child),
            (Self::Goal, Self::Milestone | Self::Task)
                | (Self::Milestone, Self::Task)
                | (Self::Task, Self::Subtask)
        )
    }

    /// Goals and milestones carry a commander's-intent statement (spec §3).
    pub fn requires_intent(self) -> bool {
        matches!(self, Self::Goal | Self::Milestone)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum NodeStatus {
    Todo,
    Active,
    Claimed,
    L1,
    L2,
    L3,
    L4,
    Failed,
    Dropped,
}

impl NodeStatus {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Todo => "todo",
            Self::Active => "active",
            Self::Claimed => "claimed",
            Self::L1 => "l1",
            Self::L2 => "l2",
            Self::L3 => "l3",
            Self::L4 => "l4",
            Self::Failed => "failed",
            Self::Dropped => "dropped",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "todo" => Some(Self::Todo),
            "active" => Some(Self::Active),
            "claimed" => Some(Self::Claimed),
            "l1" => Some(Self::L1),
            "l2" => Some(Self::L2),
            "l3" => Some(Self::L3),
            "l4" => Some(Self::L4),
            "failed" => Some(Self::Failed),
            "dropped" => Some(Self::Dropped),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum Hill {
    Uphill,
    Downhill,
}

impl Hill {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Uphill => "uphill",
            Self::Downhill => "downhill",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "uphill" => Some(Self::Uphill),
            "downhill" => Some(Self::Downhill),
            _ => None,
        }
    }
}

/// One node of the plan tree, as read from the views.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct PlanNode {
    pub id: String,
    pub parent_id: Option<String>,
    pub kind: NodeKind,
    pub title: String,
    pub intent: Option<String>,
    pub estimate_min: Option<u32>,
    pub status: NodeStatus,
    pub hill: Option<Hill>,
    pub expected_files: Vec<String>,
    pub depends_on: Vec<String>,
    /// Plan links to tasks in other repos (spec §9.1).
    pub external_deps: Vec<ExternalDep>,
    /// Where the node came from: by hand, drafted at setup, or a plan file.
    pub source: NodeSource,
    /// The branch of the session that last claimed this task.
    pub branch: Option<String>,
    /// Distance from the root in a loaded tree (0 for single-node reads).
    pub depth: usize,
}

/// A plan link to a task in another repo.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ExternalDep {
    pub repo_id: String,
    /// The other repo's folder name (`repos.name`); its id when it isn't registered.
    pub repo_name: String,
    pub id: String,
}

/// Where a plan node came from (spec §5.3 `source`). Stored as `manual`, `init`, or JSON
/// `{"file": "<repo-relative path>", "task": N, "step": k}`.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub enum NodeSource {
    /// Added by hand (`plan_edit`, the CLI) — every node from before imports existed.
    #[default]
    Manual,
    /// Drafted by `init_plan` when the repo was set up.
    Init,
    /// Imported from a plan file: its task `task`, and step `step` for a subtask.
    File {
        file: String,
        task: u32,
        step: Option<u32>,
    },
}

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
enum NamedSource {
    Manual,
    Init,
}

#[derive(Serialize, Deserialize)]
struct FileSource {
    file: String,
    task: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    step: Option<u32>,
}

#[derive(Serialize, Deserialize)]
#[serde(untagged)]
enum SourceRepr {
    Named(NamedSource),
    File(FileSource),
}

impl Serialize for NodeSource {
    fn serialize<S: serde::Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        let repr = match self {
            Self::Manual => SourceRepr::Named(NamedSource::Manual),
            Self::Init => SourceRepr::Named(NamedSource::Init),
            Self::File { file, task, step } => SourceRepr::File(FileSource {
                file: file.clone(),
                task: *task,
                step: *step,
            }),
        };
        repr.serialize(s)
    }
}

impl<'de> Deserialize<'de> for NodeSource {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        Ok(match SourceRepr::deserialize(d)? {
            SourceRepr::Named(NamedSource::Manual) => Self::Manual,
            SourceRepr::Named(NamedSource::Init) => Self::Init,
            SourceRepr::File(f) => Self::File {
                file: f.file,
                task: f.task,
                step: f.step,
            },
        })
    }
}

impl NodeSource {
    /// The `plan_nodes.source` column text.
    pub fn to_column(&self) -> String {
        match self {
            Self::Manual => "manual".into(),
            Self::Init => "init".into(),
            Self::File { .. } => serde_json::to_string(self).unwrap_or_else(|_| "manual".into()),
        }
    }

    /// Read the column back; anything unreadable is manual.
    pub fn from_column(text: &str) -> Self {
        match text {
            "manual" => Self::Manual,
            "init" => Self::Init,
            other => serde_json::from_str(other).unwrap_or_default(),
        }
    }
}
