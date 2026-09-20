//! Application persistence contexts and optimistic record transactions.
use crate::control::catalog::{AgentRecord, ProviderRecord};
use crate::control::conversation::{MessageRecord, SessionRecord};
use crate::control::cron::CronRecord;
use crate::control::project::ProjectRecord;
use crate::control::runs::RunRecord;
use ait_contracts::SettingsDocument;
use std::collections::HashMap;
pub(in crate::control) mod access;
pub(in crate::control) mod codec;
pub(in crate::control) mod transaction;
pub(in crate::control) const fn default_settings_revision() -> u64 {
    1
}
pub(in crate::control) trait HasProjects {
    fn projects(&self) -> &Vec<ProjectRecord>;
    fn projects_mut(&mut self) -> &mut Vec<ProjectRecord>;
}
pub(in crate::control) trait HasAgents {
    fn agents(&self) -> &Vec<AgentRecord>;
    fn agents_mut(&mut self) -> &mut Vec<AgentRecord>;
}
pub(in crate::control) trait HasProviders {
    fn providers(&self) -> &Vec<ProviderRecord>;
}
pub(in crate::control) trait HasProviderCredentials {
    fn provider_credentials(&self) -> &HashMap<String, String>;
}
pub(in crate::control) trait HasRunCredentials {
    fn run_credentials(&self) -> &HashMap<String, String>;
    fn run_credentials_mut(&mut self) -> &mut HashMap<String, String>;
}
pub(in crate::control) trait HasSessions {
    fn sessions(&self) -> &Vec<SessionRecord>;
    fn sessions_mut(&mut self) -> &mut Vec<SessionRecord>;
}
pub(in crate::control) trait HasMessages {
    fn messages(&self) -> &Vec<MessageRecord>;
    fn messages_mut(&mut self) -> &mut Vec<MessageRecord>;
}
pub(in crate::control) trait HasRuns {
    fn runs(&self) -> &Vec<RunRecord>;
    fn runs_mut(&mut self) -> &mut Vec<RunRecord>;
}

pub(in crate::control) trait HasCrons {
    fn crons(&self) -> &Vec<CronRecord>;
    fn crons_mut(&mut self) -> &mut Vec<CronRecord>;
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
macro_rules! define_record_context {
    ($name:ident { $($field:ident : $ty:ty),* $(,)? } [$($key:literal => $kind:ident),* $(,)?]) => {
        #[derive(Clone, Debug, PartialEq, serde::Deserialize)]
        #[serde(default)]
        pub(in crate::control) struct $name { $(pub(in crate::control) $field: $ty),* }
        impl Default for $name {
            fn default() -> Self {
                Self {
                    $($field: $crate::control::persistence::field_default!($field)),*
                }
            }
        }
        $($crate::control::persistence::field_access!($name, $field, $ty);)*
        impl $crate::control::persistence::transaction::RecordContext for $name {
            const FIELDS: &'static [(&'static str, ait_ports::ControlRecordKind)] =
                &[$(($key, ait_ports::ControlRecordKind::$kind)),*];
            fn changes(
                &self,
                updated: &Self,
            ) -> Vec<$crate::control::persistence::transaction::TypedChange> {
                let mut changes = Vec::new();
                $($crate::control::persistence::field_changes!(
                    self,
                    updated,
                    changes,
                    $field
                );)*
                changes
            }
        }
    };
}
macro_rules! field_default {
    (providers) => {
        $crate::control::catalog::builtin_providers()
    };
    (settings) => {
        ait_contracts::default_settings()
    };
    (settings_revision) => {
        $crate::control::persistence::default_settings_revision()
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
            $changes.push(
                $crate::control::persistence::transaction::TypedChange::Settings(
                    $after.settings.clone(),
                    $after.settings_revision,
                ),
            );
        }
    };
    ($before:ident, $after:ident, $changes:ident, settings_revision) => {};
    ($before:ident, $after:ident, $changes:ident, provider_credentials) => {
        $crate::control::persistence::transaction::diff_map(
            &$before.provider_credentials,
            &$after.provider_credentials,
            ait_ports::ControlRecordKind::ProviderCredential,
            $crate::control::persistence::transaction::TypedChange::ProviderCredential,
            &mut $changes,
        );
    };
    ($before:ident, $after:ident, $changes:ident, run_credentials) => {
        $crate::control::persistence::transaction::diff_map(
            &$before.run_credentials,
            &$after.run_credentials,
            ait_ports::ControlRecordKind::RunCredential,
            $crate::control::persistence::transaction::TypedChange::RunCredential,
            &mut $changes,
        );
    };
    ($before:ident, $after:ident, $changes:ident, $field:ident) => {
        $crate::control::persistence::transaction::diff_records(
            &$before.$field,
            &$after.$field,
            &mut $changes,
        );
    };
}
macro_rules! field_access {
    ($context:ident, projects, $ty:ty) => {
        impl $crate::control::persistence::HasProjects for $context {
            fn projects(&self) -> &$ty {
                &self.projects
            }
            fn projects_mut(&mut self) -> &mut $ty {
                &mut self.projects
            }
        }
    };
    ($context:ident, messages, $ty:ty) => {
        impl $crate::control::persistence::HasMessages for $context {
            fn messages(&self) -> &$ty {
                &self.messages
            }
            fn messages_mut(&mut self) -> &mut $ty {
                &mut self.messages
            }
        }
    };
    ($context:ident, agents, $ty:ty) => {
        impl $crate::control::persistence::HasAgents for $context {
            fn agents(&self) -> &$ty {
                &self.agents
            }
            fn agents_mut(&mut self) -> &mut $ty {
                &mut self.agents
            }
        }
    };
    ($context:ident, providers, $ty:ty) => {
        impl $crate::control::persistence::HasProviders for $context {
            fn providers(&self) -> &$ty {
                &self.providers
            }
        }
    };
    ($context:ident, provider_credentials, $ty:ty) => {
        impl $crate::control::persistence::HasProviderCredentials for $context {
            fn provider_credentials(&self) -> &$ty {
                &self.provider_credentials
            }
        }
    };
    ($context:ident, sessions, $ty:ty) => {
        impl $crate::control::persistence::HasSessions for $context {
            fn sessions(&self) -> &$ty {
                &self.sessions
            }
            fn sessions_mut(&mut self) -> &mut $ty {
                &mut self.sessions
            }
        }
    };
    ($context:ident, runs, $ty:ty) => {
        impl $crate::control::persistence::HasRuns for $context {
            fn runs(&self) -> &$ty {
                &self.runs
            }
            fn runs_mut(&mut self) -> &mut $ty {
                &mut self.runs
            }
        }
    };
    ($context:ident, run_credentials, $ty:ty) => {
        impl $crate::control::persistence::HasRunCredentials for $context {
            fn run_credentials(&self) -> &$ty {
                &self.run_credentials
            }
            fn run_credentials_mut(&mut self) -> &mut $ty {
                &mut self.run_credentials
            }
        }
    };
    ($context:ident, settings, $ty:ty) => {
        impl $crate::control::persistence::HasSettings for $context {
            fn settings(&self) -> &$ty {
                &self.settings
            }
            fn settings_mut(&mut self) -> &mut $ty {
                &mut self.settings
            }
        }
    };
    ($context:ident, settings_revision, $ty:ty) => {
        impl $crate::control::persistence::HasSettingsRevision for $context {
            fn settings_revision(&self) -> &$ty {
                &self.settings_revision
            }
            fn settings_revision_mut(&mut self) -> &mut $ty {
                &mut self.settings_revision
            }
        }
    };
    ($context:ident, crons, $ty:ty) => {
        impl $crate::control::persistence::HasCrons for $context {
            fn crons(&self) -> &$ty {
                &self.crons
            }
            fn crons_mut(&mut self) -> &mut $ty {
                &mut self.crons
            }
        }
    };
}

pub(in crate::control) use define_record_context;
pub(in crate::control) use field_access;
pub(in crate::control) use field_changes;
pub(in crate::control) use field_default;

#[cfg(test)]
mod tests;
