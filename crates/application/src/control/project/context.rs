//! Typed persistence contexts owned by Project use cases.
use crate::control::catalog::{AgentRecord, ProviderRecord};
use crate::control::conversation::{MessageRecord, SessionRecord};
use crate::control::persistence::define_record_context;
use crate::control::project::ProjectRecord;

define_record_context!(ProjectsContext {
    projects: Vec<ProjectRecord>,
} [ "projects" => Project ]);

define_record_context!(ProjectRegistrationContext {
    projects: Vec<ProjectRecord>,
    messages: Vec<MessageRecord>,
} [ "projects" => Project, "messages" => Message ]);

define_record_context!(ProjectAgentContext {
    projects: Vec<ProjectRecord>,
    agents: Vec<AgentRecord>,
} [ "projects" => Project, "agents" => Agent ]);

define_record_context!(ArchiveContext {
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
