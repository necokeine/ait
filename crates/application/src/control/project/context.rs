//! Typed persistence contexts owned by Project use cases.
use crate::control::catalog::AgentRecord;
use crate::control::conversation::MessageRecord;
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
