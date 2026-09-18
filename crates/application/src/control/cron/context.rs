//! Typed persistence contexts owned by Cron use cases.
use std::collections::HashMap;

use ait_contracts::SettingsDocument;

use crate::control::catalog::{AgentRecord, ProviderRecord};
use crate::control::conversation::{MessageRecord, SessionRecord};
use crate::control::cron::CronRecord;
use crate::control::persistence::define_record_context;
use crate::control::project::ProjectRecord;
use crate::control::runs::RunRecord;

define_record_context!(CronCreateContext {
    crons: Vec<CronRecord>,
    projects: Vec<ProjectRecord>,
    messages: Vec<MessageRecord>,
    agents: Vec<AgentRecord>,
    settings: SettingsDocument,
    settings_revision: u64,
} [
    "crons" => Cron,
    "projects" => Project,
    "messages" => Message,
    "agents" => Agent,
    "settings" => Settings
]);

define_record_context!(CronsContext {
    crons: Vec<CronRecord>,
} [ "crons" => Cron ]);

define_record_context!(CronTriggerContext {
    crons: Vec<CronRecord>,
    projects: Vec<ProjectRecord>,
    agents: Vec<AgentRecord>,
    providers: Vec<ProviderRecord>,
    provider_credentials: HashMap<String, String>,
    run_credentials: HashMap<String, String>,
    sessions: Vec<SessionRecord>,
    messages: Vec<MessageRecord>,
    runs: Vec<RunRecord>,
    settings: SettingsDocument,
    settings_revision: u64,
} [
    "crons" => Cron,
    "projects" => Project,
    "agents" => Agent,
    "providers" => Provider,
    "provider_credentials" => ProviderCredential,
    "run_credentials" => RunCredential,
    "sessions" => Session,
    "messages" => Message,
    "runs" => Run,
    "settings" => Settings
]);
