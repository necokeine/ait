//! Typed persistence contexts owned by conversation use cases.
use std::collections::HashMap;

use ait_contracts::SettingsDocument;

use crate::control::catalog::{AgentRecord, ProviderRecord};
use crate::control::conversation::{MessageRecord, SessionRecord};
use crate::control::persistence::define_record_context;
use crate::control::project::ProjectRecord;
use crate::control::runs::RunRecord;

define_record_context!(SessionsContext {
    sessions: Vec<SessionRecord>,
} [ "sessions" => Session ]);

define_record_context!(SessionConfigContext {
    sessions: Vec<SessionRecord>,
    agents: Vec<AgentRecord>,
    providers: Vec<ProviderRecord>,
} [ "sessions" => Session, "agents" => Agent, "providers" => Provider ]);

define_record_context!(MessagesContext {
    messages: Vec<MessageRecord>,
} [ "messages" => Message ]);

define_record_context!(ConversationContext {
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

define_record_context!(NewSessionContext {
    projects: Vec<ProjectRecord>,
    sessions: Vec<SessionRecord>,
    agents: Vec<AgentRecord>,
    messages: Vec<MessageRecord>,
    settings: SettingsDocument,
    settings_revision: u64,
} [
    "projects" => Project,
    "sessions" => Session,
    "agents" => Agent,
    "messages" => Message,
    "settings" => Settings
]);

define_record_context!(SessionBindingContext {
    sessions: Vec<SessionRecord>,
    agents: Vec<AgentRecord>,
} [ "sessions" => Session, "agents" => Agent ]);

define_record_context!(SessionTitleContext {
    projects: Vec<ProjectRecord>,
    agents: Vec<AgentRecord>,
    providers: Vec<ProviderRecord>,
    provider_credentials: HashMap<String, String>,
    sessions: Vec<SessionRecord>,
    messages: Vec<MessageRecord>,
    runs: Vec<RunRecord>,
    settings: SettingsDocument,
    settings_revision: u64,
} [
    "projects" => Project,
    "agents" => Agent,
    "providers" => Provider,
    "provider_credentials" => ProviderCredential,
    "sessions" => Session,
    "messages" => Message,
    "runs" => Run,
    "settings" => Settings
]);

define_record_context!(CodexImportContext {
    projects: Vec<ProjectRecord>,
    agents: Vec<AgentRecord>,
    providers: Vec<ProviderRecord>,
    sessions: Vec<SessionRecord>,
    messages: Vec<MessageRecord>,
} [
    "projects" => Project,
    "agents" => Agent,
    "providers" => Provider,
    "sessions" => Session,
    "messages" => Message
]);
