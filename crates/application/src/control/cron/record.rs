//! Persisted Cron configuration record.
use ait_contracts::CronView;
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub(in crate::control) struct CronRecord {
    pub id: String,
    pub name: String,
    pub project_id: String,
    pub base_message_id: String,
    pub agent_id: String,
    pub schedule: String,
    pub timezone: String,
    pub enabled: bool,
}

impl CronRecord {
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

impl TryFrom<CronView> for CronRecord {
    type Error = serde_json::Error;

    fn try_from(view: CronView) -> Result<Self, Self::Error> {
        serde_json::from_value(serde_json::to_value(view)?)
    }
}
