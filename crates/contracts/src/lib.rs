//! Versioned transport DTOs shared by HTTP, CLI, IPC, and future UI clients.
#![allow(missing_docs)]

use ait_domain::ErrorCode;
use serde::{Deserialize, Serialize};
use serde_json::Value;

/// Current command/event wire contract version.
pub const API_VERSION: u16 = 1;

/// Current portable Project archive format.
pub const PROJECT_EXPORT_VERSION: u16 = 2;

/// Execution backend selected by an Agent revision.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AgentMode {
    Codex,
    Echo,
    Tool,
    Manual,
    ProviderFailure,
    ApprovalRequired,
}

/// Codex reasoning effort values supported by the built-in model catalog entry.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ReasoningEffort {
    Low,
    Medium,
    High,
    Xhigh,
    Max,
    Ultra,
}

impl ReasoningEffort {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Low => "low",
            Self::Medium => "medium",
            Self::High => "high",
            Self::Xhigh => "xhigh",
            Self::Max => "max",
            Self::Ultra => "ultra",
        }
    }
}

/// Commands accepted by the shared application service.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Command {
    RegisterProject {
        id: String,
        name: String,
        workdir: String,
        #[serde(default)]
        fork_repo_url: Option<String>,
    },
    SetProjectDefaultAgent {
        project_id: String,
        agent_id: String,
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
    SetSessionAgent {
        session_id: String,
        agent_id: String,
        #[serde(default)]
        expected_version: Option<u64>,
    },
    RenameSession {
        session_id: String,
        name: String,
    },
    SetSessionTitle {
        session_id: String,
        title: String,
    },
    SendMessage {
        session_id: String,
        text: String,
        #[serde(default)]
        expected_version: Option<u64>,
        #[serde(default)]
        reasoning_effort: Option<ReasoningEffort>,
    },
    ForkSession {
        id: String,
        project_id: String,
        agent_id: String,
        at_message_id: String,
        text: String,
        #[serde(default)]
        reasoning_effort: Option<ReasoningEffort>,
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
    ExportProject {
        project_id: String,
    },
    ImportProject {
        archive: ProjectExport,
        workdir: String,
    },
    GetSettings,
    SaveSettings {
        expected_revision: u64,
        values: desktop::SettingsDocument,
    },
    ResetSettings,
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
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fork_repo_url: Option<String>,
    /// Immutable repository HEAD captured when the Project was registered.
    #[serde(default)]
    pub base_commit: String,
    #[serde(default)]
    pub default_agent_id: Option<String>,
    #[serde(default = "default_revision")]
    pub revision: u64,
}

const fn default_revision() -> u64 {
    1
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
    #[serde(default)]
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    #[serde(default)]
    pub description: String,
    #[serde(default)]
    pub title_generation_started: bool,
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
    /// Clean repository HEAD captured with interactive human input.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub git_commit: Option<String>,
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
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reasoning_effort: Option<ReasoningEffort>,
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

/// Portable, credential-free Project and Session archive.
///
/// Runtime attempts, active Run bindings, Cron registrations, attachment
/// bytes, and provider credentials are deliberately outside this format.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProjectExport {
    pub format_version: u16,
    pub source_revision: u64,
    pub project: ProjectView,
    pub agents: Vec<AgentView>,
    pub sessions: Vec<SessionView>,
    pub messages: Vec<MessageView>,
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
    ProjectExport(ProjectExport),
    Settings(desktop::SettingsView),
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

/// Versioned desktop workspace, settings, and branch-operation DTOs.
pub mod desktop;

pub use desktop::{
    AgentSummary, DESKTOP_PROTOCOL_VERSION, DesktopMessage, DesktopMessagePart, DesktopProject,
    DesktopSession, DesktopSnapshot, ForkFromMessageRequest, SaveSettingsRequest, SettingCategory,
    SettingDefinition, SettingKind, SettingsDocument, SettingsSchema, SettingsView,
    default_settings, settings_schema,
};
