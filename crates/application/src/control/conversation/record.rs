//! Persisted Session and Message records used by conversation use cases.
use ait_contracts::{MessageView, SessionView};
use ait_domain::{MessageKind, MessageRole};
use serde::{Deserialize, Serialize};
use serde_json::Value;

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub(in crate::control) struct SessionRecord {
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

impl SessionRecord {
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

impl TryFrom<SessionView> for SessionRecord {
    type Error = serde_json::Error;

    fn try_from(view: SessionView) -> Result<Self, Self::Error> {
        serde_json::from_value(serde_json::to_value(view)?)
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub(in crate::control) struct MessageRecord {
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

impl MessageRecord {
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

impl TryFrom<MessageView> for MessageRecord {
    type Error = serde_json::Error;

    fn try_from(view: MessageView) -> Result<Self, Self::Error> {
        serde_json::from_value(serde_json::to_value(view)?)
    }
}
