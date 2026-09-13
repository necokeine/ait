//! Typed command contexts. Reducers declare capabilities instead of accessing a workspace bag.
use crate::control::model::ProviderState;
use crate::control::model::RunState;
use crate::control::model::{AgentState, CronState, MessageState, ProjectState, SessionState};

use crate::control::catalog::builtin_providers;
use crate::control::runs::journal::WorkspaceRunJournal;
use crate::control::state::transaction::{RecordContext, TypedChange, diff_map, diff_records};
use ait_contracts::{SettingsDocument, default_settings};
use ait_ports::ControlRecordKind as Kind;
use serde::Deserialize;
use std::collections::HashMap;
pub(in crate::control) mod codec;
pub(in crate::control) mod commands;
pub(in crate::control) mod read_plan;
pub(in crate::control) mod records;
pub(in crate::control) mod transaction;
pub(in crate::control) const fn default_settings_revision() -> u64 {
    1
}
pub(in crate::control) trait HasProjects {
    fn projects(&self) -> &Vec<ProjectState>;
    fn projects_mut(&mut self) -> &mut Vec<ProjectState>;
}
pub(in crate::control) trait HasAgents {
    fn agents(&self) -> &Vec<AgentState>;
    fn agents_mut(&mut self) -> &mut Vec<AgentState>;
}
pub(in crate::control) trait HasProviders {
    fn providers(&self) -> &Vec<ProviderState>;
    fn providers_mut(&mut self) -> &mut Vec<ProviderState>;
}
pub(in crate::control) trait HasProviderCredentials {
    fn provider_credentials(&self) -> &HashMap<String, String>;
}
pub(in crate::control) trait HasRunCredentials {
    fn run_credentials(&self) -> &HashMap<String, String>;
    fn run_credentials_mut(&mut self) -> &mut HashMap<String, String>;
}
pub(in crate::control) trait HasSessions {
    fn sessions(&self) -> &Vec<SessionState>;
    fn sessions_mut(&mut self) -> &mut Vec<SessionState>;
}
pub(in crate::control) trait HasMessages {
    fn messages(&self) -> &Vec<MessageState>;
    fn messages_mut(&mut self) -> &mut Vec<MessageState>;
}
pub(in crate::control) trait HasRuns {
    fn runs(&self) -> &Vec<RunState>;
    fn runs_mut(&mut self) -> &mut Vec<RunState>;
}
pub(in crate::control) trait HasWorkspaceRunJournals {
    fn workspace_run_journals_mut(&mut self) -> &mut HashMap<String, WorkspaceRunJournal>;
}
pub(in crate::control) trait HasCrons {
    fn crons(&self) -> &Vec<CronState>;
    fn crons_mut(&mut self) -> &mut Vec<CronState>;
}
pub(in crate::control) trait HasSettings {
    fn settings(&self) -> &SettingsDocument;
    fn settings_mut(&mut self) -> &mut SettingsDocument;
}
pub(in crate::control) trait HasSettingsRevision {
    fn settings_revision(&self) -> &u64;
    fn settings_revision_mut(&mut self) -> &mut u64;
}

// The macro shares only construction/codec plumbing. Each invocation declares a
// distinct struct and exactly the capabilities available to its reducers.
macro_rules! context {
    ($name:ident { $($field:ident : $ty:ty),* $(,)? } [$($key:literal => $kind:ident),* $(,)?]) => {
        #[derive(Clone, Debug, PartialEq, Deserialize)]
        #[serde(default)]
        pub(in crate::control) struct $name { $(pub(in crate::control) $field: $ty),* }
        impl Default for $name {
            fn default() -> Self { Self { $($field: field_default!($field)),* } }
        }
        $(field_access!($name, $field, $ty);)*
        impl RecordContext for $name {
            const FIELDS: &'static [(&'static str, Kind)] = &[$(($key, Kind::$kind)),*];
            fn changes(&self, updated: &Self) -> Vec<TypedChange> {
                let mut changes = Vec::new();
                $(field_changes!(self, updated, changes, $field);)*
                changes
            }
        }
    };
}
macro_rules! field_default {
    (providers) => {
        builtin_providers()
    };
    (settings) => {
        default_settings()
    };
    (settings_revision) => {
        default_settings_revision()
    };
    ($field:ident) => {
        Default::default()
    };
}
macro_rules! field_changes {
    ($before:ident, $after:ident, $changes:ident, settings) => {
        if $before.settings != $after.settings
            || $before.settings_revision != $after.settings_revision
        {
            $changes.push(TypedChange::Settings(
                $after.settings.clone(),
                $after.settings_revision,
            ));
        }
    };
    ($before:ident, $after:ident, $changes:ident, settings_revision) => {};
    ($before:ident, $after:ident, $changes:ident, provider_credentials) => {
        diff_map(
            &$before.provider_credentials,
            &$after.provider_credentials,
            Kind::ProviderCredential,
            TypedChange::ProviderCredential,
            &mut $changes,
        );
    };
    ($before:ident, $after:ident, $changes:ident, run_credentials) => {
        diff_map(
            &$before.run_credentials,
            &$after.run_credentials,
            Kind::RunCredential,
            TypedChange::RunCredential,
            &mut $changes,
        );
    };
    ($before:ident, $after:ident, $changes:ident, workspace_run_journals) => {
        diff_map(
            &$before.workspace_run_journals,
            &$after.workspace_run_journals,
            Kind::WorkspaceRunJournal,
            TypedChange::WorkspaceRunJournal,
            &mut $changes,
        );
    };
    ($before:ident, $after:ident, $changes:ident, $field:ident) => {
        diff_records(&$before.$field, &$after.$field, &mut $changes);
    };
}
macro_rules! field_access {
    ($context:ident, projects, $ty:ty) => {
        impl HasProjects for $context {
            fn projects(&self) -> &$ty {
                &self.projects
            }
            fn projects_mut(&mut self) -> &mut $ty {
                &mut self.projects
            }
        }
    };
    ($context:ident, messages, $ty:ty) => {
        impl HasMessages for $context {
            fn messages(&self) -> &$ty {
                &self.messages
            }
            fn messages_mut(&mut self) -> &mut $ty {
                &mut self.messages
            }
        }
    };
    ($context:ident, agents, $ty:ty) => {
        impl HasAgents for $context {
            fn agents(&self) -> &$ty {
                &self.agents
            }
            fn agents_mut(&mut self) -> &mut $ty {
                &mut self.agents
            }
        }
    };
    ($context:ident, providers, $ty:ty) => {
        impl HasProviders for $context {
            fn providers(&self) -> &$ty {
                &self.providers
            }
            fn providers_mut(&mut self) -> &mut $ty {
                &mut self.providers
            }
        }
    };
    ($context:ident, provider_credentials, $ty:ty) => {
        impl HasProviderCredentials for $context {
            fn provider_credentials(&self) -> &$ty {
                &self.provider_credentials
            }
        }
    };
    ($context:ident, sessions, $ty:ty) => {
        impl HasSessions for $context {
            fn sessions(&self) -> &$ty {
                &self.sessions
            }
            fn sessions_mut(&mut self) -> &mut $ty {
                &mut self.sessions
            }
        }
    };
    ($context:ident, runs, $ty:ty) => {
        impl HasRuns for $context {
            fn runs(&self) -> &$ty {
                &self.runs
            }
            fn runs_mut(&mut self) -> &mut $ty {
                &mut self.runs
            }
        }
    };
    ($context:ident, run_credentials, $ty:ty) => {
        impl HasRunCredentials for $context {
            fn run_credentials(&self) -> &$ty {
                &self.run_credentials
            }
            fn run_credentials_mut(&mut self) -> &mut $ty {
                &mut self.run_credentials
            }
        }
    };
    ($context:ident, settings, $ty:ty) => {
        impl HasSettings for $context {
            fn settings(&self) -> &$ty {
                &self.settings
            }
            fn settings_mut(&mut self) -> &mut $ty {
                &mut self.settings
            }
        }
    };
    ($context:ident, settings_revision, $ty:ty) => {
        impl HasSettingsRevision for $context {
            fn settings_revision(&self) -> &$ty {
                &self.settings_revision
            }
            fn settings_revision_mut(&mut self) -> &mut $ty {
                &mut self.settings_revision
            }
        }
    };
    ($context:ident, workspace_run_journals, $ty:ty) => {
        impl HasWorkspaceRunJournals for $context {
            fn workspace_run_journals_mut(&mut self) -> &mut $ty {
                &mut self.workspace_run_journals
            }
        }
    };
    ($context:ident, crons, $ty:ty) => {
        impl HasCrons for $context {
            fn crons(&self) -> &$ty {
                &self.crons
            }
            fn crons_mut(&mut self) -> &mut $ty {
                &mut self.crons
            }
        }
    };
}

context!(ProjectsContext {
    projects: Vec<ProjectState>,
} [ "projects" => Project ]);

context!(ProjectRegistrationContext {
    projects: Vec<ProjectState>,
    messages: Vec<MessageState>,
} [ "projects" => Project, "messages" => Message ]);

context!(ProjectAgentContext {
    projects: Vec<ProjectState>,
    agents: Vec<AgentState>,
} [ "projects" => Project, "agents" => Agent ]);

context!(AgentsContext {
    agents: Vec<AgentState>,
} [ "agents" => Agent ]);

context!(AgentContext {
    agents: Vec<AgentState>,
    providers: Vec<ProviderState>,
} [ "agents" => Agent, "providers" => Provider ]);

context!(ProviderContext {
    agents: Vec<AgentState>,
    providers: Vec<ProviderState>,
    provider_credentials: HashMap<String, String>,
} [ "agents" => Agent, "providers" => Provider, "provider_credentials" => ProviderCredential ]);

context!(SessionsContext {
    sessions: Vec<SessionState>,
} [ "sessions" => Session ]);

context!(SessionConfigContext {
    sessions: Vec<SessionState>,
    agents: Vec<AgentState>,
    providers: Vec<ProviderState>,
} [ "sessions" => Session, "agents" => Agent, "providers" => Provider ]);

context!(MessagesContext {
    messages: Vec<MessageState>,
} [ "messages" => Message ]);

context!(RunsContext {
    runs: Vec<RunState>,
} [ "runs" => Run ]);

context!(ConversationContext {
    projects: Vec<ProjectState>,
    agents: Vec<AgentState>,
    providers: Vec<ProviderState>,
    provider_credentials: HashMap<String, String>,
    run_credentials: HashMap<String, String>,
    sessions: Vec<SessionState>,
    messages: Vec<MessageState>,
    runs: Vec<RunState>,
    settings: SettingsDocument,
    settings_revision: u64,
} [ "projects" => Project, "agents" => Agent, "providers" => Provider, "provider_credentials" => ProviderCredential, "run_credentials" => RunCredential, "sessions" => Session, "messages" => Message, "runs" => Run, "settings" => Settings ]);

context!(RunContext {
    projects: Vec<ProjectState>,
    sessions: Vec<SessionState>,
    messages: Vec<MessageState>,
    runs: Vec<RunState>,
    workspace_run_journals: HashMap<String, WorkspaceRunJournal>,
    run_credentials: HashMap<String, String>,
    settings: SettingsDocument,
    settings_revision: u64,
} [ "projects" => Project, "sessions" => Session, "messages" => Message, "runs" => Run, "workspace_run_journals" => WorkspaceRunJournal, "run_credentials" => RunCredential, "settings" => Settings ]);

context!(RunControlContext {
    projects: Vec<ProjectState>,
    sessions: Vec<SessionState>,
    runs: Vec<RunState>,
    workspace_run_journals: HashMap<String, WorkspaceRunJournal>,
} [ "projects" => Project, "sessions" => Session, "runs" => Run, "workspace_run_journals" => WorkspaceRunJournal ]);

context!(CronCreateContext {
    crons: Vec<CronState>,
    messages: Vec<MessageState>,
    agents: Vec<AgentState>,
} [ "crons" => Cron, "messages" => Message, "agents" => Agent ]);

context!(CronsContext {
    crons: Vec<CronState>,
} [ "crons" => Cron ]);

context!(CronTriggerContext {
    crons: Vec<CronState>,
    projects: Vec<ProjectState>,
    agents: Vec<AgentState>,
    providers: Vec<ProviderState>,
    provider_credentials: HashMap<String, String>,
    run_credentials: HashMap<String, String>,
    messages: Vec<MessageState>,
    runs: Vec<RunState>,
    settings: SettingsDocument,
    settings_revision: u64,
} [ "crons" => Cron, "projects" => Project, "agents" => Agent, "providers" => Provider, "provider_credentials" => ProviderCredential, "run_credentials" => RunCredential, "messages" => Message, "runs" => Run, "settings" => Settings ]);

context!(ArchiveContext {
    projects: Vec<ProjectState>,
    agents: Vec<AgentState>,
    providers: Vec<ProviderState>,
    sessions: Vec<SessionState>,
    messages: Vec<MessageState>,
} [ "projects" => Project, "agents" => Agent, "providers" => Provider, "sessions" => Session, "messages" => Message ]);

context!(SettingsContext {
    settings: SettingsDocument,
    settings_revision: u64,
} [ "settings" => Settings ]);

context!(ApiRunContext {
    runs: Vec<RunState>,
    sessions: Vec<SessionState>,
    messages: Vec<MessageState>,
} [ "runs" => Run, "sessions" => Session, "messages" => Message ]);

context!(NewSessionContext {
    projects: Vec<ProjectState>,
    sessions: Vec<SessionState>,
    agents: Vec<AgentState>,
    messages: Vec<MessageState>,
} [ "projects" => Project, "sessions" => Session, "agents" => Agent, "messages" => Message ]);

context!(SessionBindingContext {
    sessions: Vec<SessionState>,
    agents: Vec<AgentState>,
} [ "sessions" => Session, "agents" => Agent ]);

context!(SessionTitleContext {
    projects: Vec<ProjectState>,
    sessions: Vec<SessionState>,
    messages: Vec<MessageState>,
    runs: Vec<RunState>,
} [ "projects" => Project, "sessions" => Session, "messages" => Message, "runs" => Run ]);

#[cfg(test)]
mod tests;
