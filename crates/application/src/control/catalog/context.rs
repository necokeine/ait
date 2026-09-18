//! Typed persistence contexts owned by Agent catalog use cases.
use std::collections::HashMap;

use crate::control::catalog::{AgentRecord, ProviderRecord};
use crate::control::persistence::define_record_context;

define_record_context!(AgentsContext {
    agents: Vec<AgentRecord>,
} [ "agents" => Agent ]);

define_record_context!(AgentContext {
    agents: Vec<AgentRecord>,
    providers: Vec<ProviderRecord>,
} [ "agents" => Agent, "providers" => Provider ]);

define_record_context!(ProviderContext {
    agents: Vec<AgentRecord>,
    providers: Vec<ProviderRecord>,
    provider_credentials: HashMap<String, String>,
} [ "agents" => Agent, "providers" => Provider, "provider_credentials" => ProviderCredential ]);
