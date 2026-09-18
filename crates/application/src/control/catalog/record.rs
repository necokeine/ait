//! Persisted Agent and Provider catalog records.
use ait_contracts::AgentView;
use ait_domain::AgentConfiguration;
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub(in crate::control) struct AgentRecord {
    pub id: String,
    pub name: String,
    pub config: AgentConfiguration,
    #[serde(default)]
    pub owner_session_id: Option<String>,
    pub revision: u64,
    pub enabled: bool,
}

impl AgentRecord {
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

impl TryFrom<AgentView> for AgentRecord {
    type Error = serde_json::Error;

    fn try_from(view: AgentView) -> Result<Self, Self::Error> {
        serde_json::from_value(serde_json::to_value(view)?)
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub(in crate::control) struct ProviderRecord {
    #[serde(flatten)]
    pub provider: ait_domain::AgentProvider,
    pub has_secret: bool,
}

impl ProviderRecord {
    pub(in crate::control) fn view(&self) -> ait_contracts::AgentProviderView {
        ait_contracts::AgentProviderView {
            provider: self.provider.clone(),
            has_secret: self.has_secret,
        }
    }
}
