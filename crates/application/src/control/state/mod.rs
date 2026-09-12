//! In-memory record working sets. Stored shapes and serialization defaults stay unchanged.
use crate::control::catalog::builtin_providers;
use crate::control::runs::journal::WorkspaceRunJournal;
use ait_contracts::{
    AgentProviderView, AgentView, CronView, MessageView, ProjectView, RunView, SessionView,
    SettingsDocument, default_settings,
};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

pub(in crate::control) mod codec;
pub(in crate::control) mod records;

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub(in crate::control) struct WorkingSet {
    #[serde(default)]
    pub(in crate::control) projects: Vec<ProjectView>,
    #[serde(default)]
    pub(in crate::control) agents: Vec<AgentView>,
    #[serde(default)]
    pub(in crate::control) providers: Vec<AgentProviderView>,
    #[serde(default)]
    pub(in crate::control) provider_credentials: HashMap<String, String>,
    #[serde(default)]
    pub(in crate::control) run_credentials: HashMap<String, String>,
    #[serde(default)]
    pub(in crate::control) sessions: Vec<SessionView>,
    #[serde(default)]
    pub(in crate::control) messages: Vec<MessageView>,
    #[serde(default)]
    pub(in crate::control) runs: Vec<RunView>,
    /// Durable completed Agent responses keyed by Run id. The production Codex
    /// adapter writes this checkpoint before publishing its isolated commit.
    #[serde(default)]
    pub(in crate::control) workspace_run_journals: HashMap<String, WorkspaceRunJournal>,
    #[serde(default)]
    pub(in crate::control) crons: Vec<CronView>,
    #[serde(default = "default_settings")]
    pub(in crate::control) settings: SettingsDocument,
    #[serde(default = "default_settings_revision")]
    pub(in crate::control) settings_revision: u64,
}

impl Default for WorkingSet {
    fn default() -> Self {
        Self {
            projects: Vec::new(),
            agents: Vec::new(),
            providers: builtin_providers(),
            provider_credentials: HashMap::new(),
            run_credentials: HashMap::new(),
            sessions: Vec::new(),
            messages: Vec::new(),
            runs: Vec::new(),
            workspace_run_journals: HashMap::new(),
            crons: Vec::new(),
            settings: default_settings(),
            settings_revision: default_settings_revision(),
        }
    }
}

pub(in crate::control) const fn default_settings_revision() -> u64 {
    1
}

pub(in crate::control) struct LoadedWorkingSet {
    pub(in crate::control) revision: u64,
    pub(in crate::control) original: WorkingSet,
}
