//! Typed persistence context owned by Settings use cases.
use ait_contracts::SettingsDocument;

use crate::control::catalog::AgentRecord;
use crate::control::persistence::define_record_context;

define_record_context!(SettingsContext {
    agents: Vec<AgentRecord>,
    settings: SettingsDocument,
    settings_revision: u64,
} [ "agents" => Agent, "settings" => Settings ]);
