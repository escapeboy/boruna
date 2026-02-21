use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[derive(Default)]
pub enum NodeStatus {
    #[default]
    Pending,
    Ready,
    Running,
    Blocked,
    Failed,
    Passed,
}

impl std::fmt::Display for NodeStatus {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Pending => write!(f, "pending"),
            Self::Ready => write!(f, "ready"),
            Self::Running => write!(f, "running"),
            Self::Blocked => write!(f, "blocked"),
            Self::Failed => write!(f, "failed"),
            Self::Passed => write!(f, "passed"),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Role {
    Planner,
    Implementer,
    Reviewer,
    RedTeam,
}

impl std::fmt::Display for Role {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Planner => write!(f, "planner"),
            Self::Implementer => write!(f, "implementer"),
            Self::Reviewer => write!(f, "reviewer"),
            Self::RedTeam => write!(f, "red-team"),
        }
    }
}

impl std::str::FromStr for Role {
    type Err = String;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "planner" => Ok(Self::Planner),
            "implementer" => Ok(Self::Implementer),
            "reviewer" => Ok(Self::Reviewer),
            "red-team" | "redteam" => Ok(Self::RedTeam),
            _ => Err(format!("unknown role: {s}")),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ReviewResult {
    Approved,
    Rejected { reason: String },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WorkNode {
    pub id: String,
    pub description: String,
    pub inputs: Vec<String>,
    pub outputs: Vec<String>,
    pub dependencies: Vec<String>,
    pub owner_role: Role,
    pub tags: Vec<String>,
    #[serde(default)]
    pub status: NodeStatus,
    pub assigned_to: Option<String>,
    pub patch_bundle: Option<String>,
    pub review_result: Option<ReviewResult>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WorkGraph {
    #[serde(default = "default_schema_version")]
    pub schema_version: u32,
    pub id: String,
    pub description: String,
    pub nodes: Vec<WorkNode>,
}

fn default_schema_version() -> u32 {
    1
}

impl WorkGraph {
    pub fn node(&self, id: &str) -> Option<&WorkNode> {
        self.nodes.iter().find(|n| n.id == id)
    }

    pub fn node_mut(&mut self, id: &str) -> Option<&mut WorkNode> {
        self.nodes.iter_mut().find(|n| n.id == id)
    }
}
