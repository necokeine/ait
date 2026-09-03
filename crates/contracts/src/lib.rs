//! Versioned transport DTOs shared by HTTP, CLI, IPC, and future UI clients.
#![allow(missing_docs)]

use ait_domain::ErrorCode;
use serde::{Deserialize, Serialize};
use serde_json::Value;

/// Current command/event wire contract version.
pub const API_VERSION: u16 = 1;

/// Deterministic built-in driver used by the executable vertical slice.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AgentMode {
    Echo,
    Tool,
    Manual,
    ProviderFailure,
    ApprovalRequired,
}

/// Commands accepted by the shared application service.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Command {
    RegisterProject {
        id: String,
        name: String,
        workdir: String,
    },
    RegisterAgent {
        id: String,
        name: String,
        model: String,
        #[serde(default = "default_agent_mode")]
        mode: AgentMode,
    },
    CreateSession {
        id: String,
        project_id: String,
        agent_id: String,
        #[serde(default)]
        at_message_id: Option<String>,
    },
    SendMessage {
        session_id: String,
        text: String,
        #[serde(default)]
        expected_version: Option<u64>,
    },
    GetRun {
        run_id: String,
    },
    CancelRun {
        run_id: String,
    },
    CreateCron {
        id: String,
        name: String,
        project_id: String,
        base_message_id: String,
        agent_id: String,
        schedule: String,
        timezone: String,
    },
    SetCronEnabled {
        cron_id: String,
        enabled: bool,
    },
    TriggerCron {
        cron_id: String,
        scheduled_at: i64,
    },
    Snapshot,
}

const fn default_agent_mode() -> AgentMode {
    AgentMode::Echo
}

/// Stable API error envelope.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct ApiError {
    pub code: ErrorCode,
    pub message: String,
    pub retryable: bool,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct ProjectView {
    pub id: String,
    pub name: String,
    pub workdir: String,
    pub root_message_id: String,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct AgentView {
    pub id: String,
    pub name: String,
    pub model: String,
    pub mode: AgentMode,
    pub revision: u64,
    pub enabled: bool,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct SessionView {
    pub id: String,
    pub project_id: String,
    pub agent_id: String,
    pub current_message_id: String,
    pub active_run_id: Option<String>,
    pub version: u64,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct MessageView {
    pub id: String,
    pub project_id: String,
    pub parent_message_id: Option<String>,
    pub role: String,
    pub kind: String,
    pub text: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub data: Option<Value>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct RunView {
    pub id: String,
    pub project_id: String,
    pub base_message_id: String,
    pub last_message_id: Option<String>,
    pub session_id: Option<String>,
    pub agent_id: String,
    pub agent_revision: u64,
    pub trigger: String,
    pub cron_id: Option<String>,
    pub scheduled_at: Option<i64>,
    pub status: String,
    pub error: Option<ApiError>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct CronView {
    pub id: String,
    pub name: String,
    pub project_id: String,
    pub base_message_id: String,
    pub agent_id: String,
    pub schedule: String,
    pub timezone: String,
    pub enabled: bool,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct WorkspaceView {
    pub projects: Vec<ProjectView>,
    pub agents: Vec<AgentView>,
    pub sessions: Vec<SessionView>,
    pub messages: Vec<MessageView>,
    pub runs: Vec<RunView>,
    pub crons: Vec<CronView>,
}

/// Successful command payload.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", content = "value", rename_all = "snake_case")]
pub enum CommandResult {
    Project(ProjectView),
    Agent(AgentView),
    Session(SessionView),
    Run(RunView),
    Cron(CronView),
    Workspace(WorkspaceView),
}

/// Response shared by every transport.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Response {
    pub api_version: u16,
    pub ok: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub result: Option<CommandResult>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<ApiError>,
}

impl Response {
    #[must_use]
    pub const fn success(result: CommandResult) -> Self {
        Self {
            api_version: API_VERSION,
            ok: true,
            result: Some(result),
            error: None,
        }
    }
    #[must_use]
    pub const fn failure(error: ApiError) -> Self {
        Self {
            api_version: API_VERSION,
            ok: false,
            result: None,
            error: Some(error),
        }
    }
}

/// Reconnectable durable event.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Event {
    pub api_version: u16,
    pub cursor: u64,
    pub kind: String,
    pub entity_id: Option<String>,
    pub body: Value,
    pub created_at: i64,
}
