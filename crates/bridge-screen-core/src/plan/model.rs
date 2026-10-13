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
    /// Distance from the root in a loaded tree (0 for single-node reads).
    pub depth: usize,
}
