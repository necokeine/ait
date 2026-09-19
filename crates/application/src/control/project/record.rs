//! Persisted Project record and its public projection.
use ait_contracts::ProjectView;
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub(in crate::control) struct ProjectRecord {
    #[serde(default, skip_serializing)]
    pub owner: Option<ait_domain::ProjectOwner>,
    #[serde(default, skip_serializing)]
    pub execution_blocked: Option<String>,
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

impl ProjectRecord {
    pub(in crate::control) fn default_agent_id(&self) -> Option<&str> {
        self.defaults.agent().map(ait_domain::AgentId::as_str)
    }

    pub(in crate::control) fn revision(&self) -> u64 {
        self.defaults.revision()
    }

    pub(in crate::control) fn view(&self) -> ProjectView {
        ProjectView {
            owner: self.owner.clone().map(Box::new),
            execution_blocked: self.execution_blocked.clone(),
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

impl TryFrom<ProjectView> for ProjectRecord {
    type Error = serde_json::Error;

    fn try_from(view: ProjectView) -> Result<Self, Self::Error> {
        serde_json::from_value(serde_json::to_value(view)?)
    }
}
