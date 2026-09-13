//! Record aggregates owned by application; contracts are constructed at boundaries.
use ait_contracts::{AgentView, CronView, MessageView, ProjectView, SessionView};
use ait_domain::{AgentConfiguration, MessageKind, MessageRole};
use serde::{Deserialize, Serialize};
use serde_json::Value;
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub(in crate::control) struct ProjectState {
    pub id: String,
    pub name: String,
    pub workdir: String,
    pub root_message_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub repo_url: Option<String>,
    /// Immutable repository HEAD captured when the Project was registered.
    #[serde(default)]
    pub base_commit: String,
    #[serde(flatten)]
    pub defaults: ait_domain::ProjectDefaults,
}
impl ProjectState {
    pub(in crate::control) fn default_agent_id(&self) -> Option<&str> {
        self.defaults.agent().map(ait_domain::AgentId::as_str)
    }
    pub(in crate::control) fn revision(&self) -> u64 {
        self.defaults.revision()
    }

    pub(in crate::control) fn view(&self) -> ProjectView {
        ProjectView {
            id: self.id.clone(),
            name: self.name.clone(),
            workdir: self.workdir.clone(),
            root_message_id: self.root_message_id.clone(),
            repo_url: self.repo_url.clone(),
            base_commit: self.base_commit.clone(),
            default_agent_id: self.default_agent_id().map(str::to_owned),
            revision: self.revision(),
        }
    }
}
impl TryFrom<ProjectView> for ProjectState {
    type Error = serde_json::Error;
    fn try_from(view: ProjectView) -> Result<Self, Self::Error> {
        serde_json::from_value(serde_json::to_value(view)?)
    }
}
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub(in crate::control) struct SessionState {
    #[serde(flatten)]
    pub reference: ait_domain::SessionReference,
    pub id: String,
    pub project_id: String,
    /// Absolute manager-owned linked worktree used by this Session.
    #[serde(default)]
    pub workdir: String,
    #[serde(default)]
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    #[serde(default)]
    pub description: String,
    #[serde(default)]
    pub title_generation_started: bool,
}
impl SessionState {
    pub(in crate::control) fn agent_id(&self) -> &str {
        self.reference.agent().as_str()
    }
    pub(in crate::control) fn current_message_id(&self) -> String {
        self.reference.head().to_string()
    }
    pub(in crate::control) fn active_run_id(&self) -> Option<&str> {
        self.reference.active_run().map(ait_domain::RunId::as_str)
    }
    pub(in crate::control) fn version(&self) -> u64 {
        self.reference.version()
    }

    pub(in crate::control) fn view(&self) -> SessionView {
        SessionView {
            id: self.id.clone(),
            project_id: self.project_id.clone(),
            workdir: self.workdir.clone(),
            name: self.name.clone(),
            title: self.title.clone(),
            description: self.description.clone(),
            title_generation_started: self.title_generation_started,
            agent_id: self.agent_id().to_owned(),
            current_message_id: self.current_message_id(),
            active_run_id: self.active_run_id().map(str::to_owned),
            version: self.version(),
        }
    }
}
impl TryFrom<SessionView> for SessionState {
    type Error = serde_json::Error;
    fn try_from(view: SessionView) -> Result<Self, Self::Error> {
        serde_json::from_value(serde_json::to_value(view)?)
    }
}
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub(in crate::control) struct MessageState {
    pub id: String,
    pub project_id: String,
    pub parent_message_id: Option<String>,
    pub role: MessageRole,
    pub kind: MessageKind,
    pub text: Option<String>,
    /// Creation time in Unix milliseconds; zero means an older record has no timestamp.
    #[serde(default)]
    pub created_at: i64,
    /// Clean repository HEAD captured with interactive human input.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub git_commit: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub data: Option<Value>,
}
impl MessageState {
    pub(in crate::control) fn view(&self) -> MessageView {
        MessageView {
            id: self.id.clone(),
            project_id: self.project_id.clone(),
            parent_message_id: self.parent_message_id.clone(),
            role: self.role.as_str().into(),
            kind: self.kind.as_str().into(),
            text: self.text.clone(),
            created_at: self.created_at,
            git_commit: self.git_commit.clone(),
            data: self.data.clone(),
        }
    }
}
impl TryFrom<MessageView> for MessageState {
    type Error = serde_json::Error;
    fn try_from(view: MessageView) -> Result<Self, Self::Error> {
        serde_json::from_value(serde_json::to_value(view)?)
    }
}
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub(in crate::control) struct AgentState {
    pub id: String,
    pub name: String,
    pub config: AgentConfiguration,
    #[serde(default)]
    pub owner_session_id: Option<String>,
    pub revision: u64,
    pub enabled: bool,
}
impl AgentState {
    pub(in crate::control) fn view(&self) -> AgentView {
        AgentView {
            id: self.id.clone(),
            name: self.name.clone(),
            config: self.config.clone(),
            owner_session_id: self.owner_session_id.clone(),
            revision: self.revision,
            enabled: self.enabled,
        }
    }
}
impl TryFrom<AgentView> for AgentState {
    type Error = serde_json::Error;
    fn try_from(view: AgentView) -> Result<Self, Self::Error> {
        serde_json::from_value(serde_json::to_value(view)?)
    }
}
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub(in crate::control) struct CronState {
    pub id: String,
    pub name: String,
    pub project_id: String,
    pub base_message_id: String,
    pub agent_id: String,
    pub schedule: String,
    pub timezone: String,
    pub enabled: bool,
}
impl CronState {
    pub(in crate::control) fn view(&self) -> CronView {
        CronView {
            id: self.id.clone(),
            name: self.name.clone(),
            project_id: self.project_id.clone(),
            base_message_id: self.base_message_id.clone(),
            agent_id: self.agent_id.clone(),
            schedule: self.schedule.clone(),
            timezone: self.timezone.clone(),
            enabled: self.enabled,
        }
    }
}
impl TryFrom<CronView> for CronState {
    type Error = serde_json::Error;
    fn try_from(view: CronView) -> Result<Self, Self::Error> {
        serde_json::from_value(serde_json::to_value(view)?)
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub(in crate::control) struct ProviderState {
    #[serde(flatten)]
    pub provider: ait_domain::AgentProvider,
    pub has_secret: bool,
}
impl ProviderState {
    pub(in crate::control) fn view(&self) -> ait_contracts::AgentProviderView {
        ait_contracts::AgentProviderView {
            provider: self.provider.clone(),
            has_secret: self.has_secret,
        }
    }
}
