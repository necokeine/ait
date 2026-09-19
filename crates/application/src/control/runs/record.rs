//! Persisted application Run aggregate and explicit transport projections.
use super::{NativeApprovalRecord, ToolInteractionRecord};
use ait_contracts::{ApiError, RunView, WorkerCommitReceipt};
use ait_domain::{
    AgentConfiguration, AgentProvider, LifecyclePhase, LifecycleStatus, RunPermissionProfile,
};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
#[derive(Clone, Debug, PartialEq)]
pub(in crate::control) struct RunRecord {
    pub auto_commit: Option<super::git_commit::AutoCommit>,
    pub compatibility_repair: bool,
    pub codex_input: Option<super::native::CodexPendingInput>,
    /// Canonical host runtime state for API Providers; absent for native harness Runs.
    pub lifecycle: RunLifecycle,

    pub id: String,
    pub project_id: String,
    pub base_message_id: String,
    pub session_id: Option<String>,
    pub agent_id: String,
    pub agent_revision: u64,
    pub config: AgentConfiguration,
    pub provider: AgentProvider,
    /// Effective non-secret permission policy fixed when this Run was created.
    pub permission_profile: RunPermissionProfile,
    /// Codex-native approval audit records. They are not Ait ToolUse/ToolResult.
    pub native_approvals: Vec<NativeApprovalRecord>,
    pub tool_approvals: Vec<ait_domain::ToolApprovalRecord>,
    pub tool_interactions: Vec<ToolInteractionRecord>,
    pub trigger: ait_domain::RunTrigger,
    pub cron_id: Option<String>,
    pub scheduled_at: Option<i64>,
    /// Git baseline authorized for a workspace-writing Run.
    pub workspace_base_commit: Option<String>,
    /// Exact Git index tree authorized with the workspace baseline.
    pub workspace_base_index_tree: Option<Box<str>>,
    /// Stable identity of the workspace side-effect operation for this Run.
    pub operation_id: Option<Box<str>>,
    /// Monotonic execution lease; late writers holding an older value are fenced.
    pub lease_epoch: u64,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub(in crate::control) struct ApiRunExecution {
    pub run: ait_domain::Run,
    pub attempts: Vec<ait_domain::RunAttempt>,
    pub tools: Vec<ait_domain::ToolExecution>,
    #[serde(default)]
    pub worker_instance_id: Option<String>,
    #[serde(default)]
    pub worker_receipts: std::collections::BTreeMap<String, WorkerCommitReceipt>,
}
#[derive(Clone, Debug, PartialEq)]
pub(in crate::control) enum RunLifecycle {
    Workspace {
        status: LifecycleStatus,
        phase: Option<LifecyclePhase>,
        last_message_id: Option<String>,
        error: Option<ait_domain::DomainError>,
    },
    Api {
        execution: Box<ApiRunExecution>,
        cancel_requested: bool,
    },
}
impl RunLifecycle {
    pub(in crate::control) fn queued() -> Self {
        Self::Workspace {
            status: LifecycleStatus::Queued,
            phase: Some(LifecyclePhase::Queued),
            last_message_id: None,
            error: None,
        }
    }
}
impl RunRecord {
    fn validate_execution(&self) -> Result<(), &'static str> {
        let Some(execution) = self.execution() else {
            return Ok(());
        };
        let run = &execution.run;
        let config_digest = format!(
            "{:x}",
            Sha256::digest(
                serde_json::to_vec(&self.config).map_err(|_| "invalid API Run configuration")?
            )
        );
        if run.agent_snapshot.config_digest != config_digest
            || run.agent_snapshot.driver_type != format!("{:?}", self.provider.kind)
            || self.provider.id != self.config.provider_id
            || run.trigger != self.trigger
            || run.id.as_str() != self.id
            || run.project_id.as_str() != self.project_id
            || run.base_message_id.to_string() != self.base_message_id
            || run
                .follow_session_id
                .as_ref()
                .map(ait_domain::SessionId::as_str)
                != self.session_id.as_deref()
            || run.agent_id.as_str() != self.agent_id
            || run.agent_revision != self.agent_revision
            || run.cron_id.as_ref().map(ait_domain::CronId::as_str) != self.cron_id.as_deref()
            || run.scheduled_at.map(|t| t.0) != self.scheduled_at
            || run.agent_snapshot.connection_name != self.config.provider_id
            || run.agent_snapshot.model != self.config.model
            || run.agent_snapshot.endpoint != self.provider.url
        {
            return Err("API Run fixed identity or configuration is inconsistent");
        }
        run.validate()
            .map_err(|_| "API Run lifecycle is inconsistent")
    }

    pub(in crate::control) fn last_message_id(&self) -> Option<String> {
        match &self.lifecycle {
            RunLifecycle::Workspace {
                last_message_id, ..
            } => last_message_id.clone(),
            RunLifecycle::Api { execution, .. } => {
                execution.run.last_message_id.map(|id| id.to_string())
            }
        }
    }
    pub(in crate::control) fn set_last_message_id(&mut self, id: Option<String>) {
        match &mut self.lifecycle {
            RunLifecycle::Workspace {
                last_message_id, ..
            } => *last_message_id = id,
            RunLifecycle::Api { .. } => {
                unreachable!("API Message pointer is committed through RunStore")
            }
        }
    }
    pub(in crate::control) fn error(&self) -> Option<ApiError> {
        match &self.lifecycle {
            RunLifecycle::Workspace { error, .. } => error.as_ref().map(|e| ApiError {
                code: e.code,
                message: e.message.clone(),
                retryable: e.retryable,
            }),
            RunLifecycle::Api { execution, .. } => execution.run.error.as_ref().map(|e| ApiError {
                code: e.code,
                message: e.message.clone(),
                retryable: e.retryable,
            }),
        }
    }
    pub(in crate::control) fn set_error(&mut self, error: Option<ApiError>) {
        let error = error.map(crate::control::errors::api_domain_error);
        match &mut self.lifecycle {
            RunLifecycle::Workspace { error: target, .. } => *target = error,
            RunLifecycle::Api { execution, .. } => execution.run.error = error,
        }
    }

    pub(in crate::control) fn status(&self) -> LifecycleStatus {
        match &self.lifecycle {
            RunLifecycle::Workspace { status, .. } => *status,
            RunLifecycle::Api {
                execution,
                cancel_requested,
            } => {
                if *cancel_requested && !execution.run.status.is_terminal() {
                    LifecycleStatus::Cancelling
                } else {
                    execution.run.status.into()
                }
            }
        }
    }
    pub(in crate::control) fn phase(&self) -> Option<LifecyclePhase> {
        match &self.lifecycle {
            RunLifecycle::Workspace { phase, .. } => *phase,
            RunLifecycle::Api { execution, .. } => Some(execution.run.phase.into()),
        }
    }
    pub(in crate::control) fn set_status(&mut self, status: LifecycleStatus) {
        use ait_domain::RunStopReason as R;
        match &mut self.lifecycle {
            RunLifecycle::Workspace { status: target, .. } => *target = status,
            RunLifecycle::Api {
                execution,
                cancel_requested,
            } => {
                if status == LifecycleStatus::Cancelling {
                    *cancel_requested = true;
                    return;
                }
                let reason = match status {
                    LifecycleStatus::Cancelled => R::Cancelled,
                    LifecycleStatus::LimitExceeded => R::RuntimeLimit,
                    LifecycleStatus::Failed | LifecycleStatus::Interrupted => R::Failed,
                    _ => unreachable!("API progress and completion are owned by RunCoordinator"),
                };
                let failure = execution.run.error.clone();
                execution
                    .run
                    .stop(
                        reason,
                        ait_domain::TimestampMs(crate::control::events::now()),
                        failure,
                    )
                    .expect("non-completion stop reason");
            }
        }
    }
    pub(in crate::control) fn set_phase(&mut self, phase: Option<LifecyclePhase>) {
        match &mut self.lifecycle {
            RunLifecycle::Workspace { phase: target, .. } => *target = phase,
            RunLifecycle::Api { execution, .. } => {
                if phase == Some(LifecyclePhase::Terminal) {
                    execution.run.phase = ait_domain::RunPhase::Terminal;
                }
            }
        }
    }
    pub(in crate::control) fn execution(&self) -> Option<&ApiRunExecution> {
        match &self.lifecycle {
            RunLifecycle::Api { execution, .. } => Some(execution),
            RunLifecycle::Workspace { .. } => None,
        }
    }
    pub(in crate::control) fn execution_mut(&mut self) -> Option<&mut ApiRunExecution> {
        match &mut self.lifecycle {
            RunLifecycle::Api { execution, .. } => Some(execution),
            RunLifecycle::Workspace { .. } => None,
        }
    }
    pub(in crate::control) fn install_execution(&mut self, execution: ApiRunExecution) {
        let cancel_requested = self.status() == LifecycleStatus::Cancelling;
        self.lifecycle = RunLifecycle::Api {
            execution: Box::new(execution),
            cancel_requested,
        };
    }
    pub(in crate::control) fn view(&self) -> RunView {
        self.projection(false)
    }
    fn record(&self) -> RunView {
        self.projection(true)
    }
    fn projection(&self, include_execution: bool) -> RunView {
        RunView {
            git_commit: self.auto_commit.as_ref().map(|commit| commit.view.clone()),
            id: self.id.clone(),
            project_id: self.project_id.clone(),
            base_message_id: self.base_message_id.clone(),
            last_message_id: self.last_message_id(),
            session_id: self.session_id.clone(),
            agent_id: self.agent_id.clone(),
            agent_revision: self.agent_revision,
            config: self.config.clone(),
            provider: self.provider.clone(),
            permission_profile: self.permission_profile,
            tool_approvals: self.tool_approvals.clone(),
            tool_interactions: self
                .tool_interactions
                .iter()
                .map(ToolInteractionRecord::view)
                .collect(),
            native_approvals: self
                .native_approvals
                .iter()
                .map(NativeApprovalRecord::view)
                .collect(),
            trigger: self.trigger.as_str().into(),
            cron_id: self.cron_id.clone(),
            scheduled_at: self.scheduled_at,
            workspace_base_commit: self.workspace_base_commit.clone(),
            workspace_base_index_tree: self.workspace_base_index_tree.clone(),
            operation_id: self.operation_id.clone(),
            lease_epoch: self.lease_epoch,
            error: self.error(),
            status: self.status().as_str().into(),
            phase: self.phase().map(|p| p.as_str().into()),
            execution: self.execution().filter(|_| include_execution).map(|e| {
                Box::new(ait_contracts::ApiRunExecution {
                    run: e.run.clone(),
                    attempts: e.attempts.clone(),
                    tools: e.tools.clone(),
                    worker_instance_id: e.worker_instance_id.clone(),
                    worker_receipts: e.worker_receipts.clone(),
                })
            }),
        }
    }
}
#[derive(Serialize, Deserialize)]
struct PersistedRun {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    auto_commit: Option<super::git_commit::AutoCommit>,
    #[serde(flatten)]
    view: RunView,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    codex_input: Option<super::native::CodexPendingInput>,
}

impl Serialize for RunRecord {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        self.validate_execution()
            .map_err(serde::ser::Error::custom)?;
        PersistedRun {
            auto_commit: self.auto_commit.clone(),
            view: self.record(),
            codex_input: self.codex_input.clone(),
        }
        .serialize(serializer)
    }
}
impl<'de> Deserialize<'de> for RunRecord {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let PersistedRun {
            view,
            codex_input,
            auto_commit,
        } = PersistedRun::deserialize(deserializer)?;
        let status: LifecycleStatus =
            serde_json::from_value(serde_json::Value::String(view.status))
                .map_err(|_| serde::de::Error::custom("unknown Run status"))?;
        let phase = view
            .phase
            .map(|p| serde_json::from_value(serde_json::Value::String(p.into())))
            .transpose()
            .map_err(|_| serde::de::Error::custom("unknown Run phase"))?;
        let legacy_error = view.error.clone();
        let legacy_last = view.last_message_id.clone();
        let lifecycle = match view.execution {
            Some(e) => RunLifecycle::Api {
                execution: Box::new(ApiRunExecution {
                    run: e.run,
                    attempts: e.attempts,
                    tools: e.tools,
                    worker_instance_id: e.worker_instance_id,
                    worker_receipts: e.worker_receipts,
                }),
                cancel_requested: status == LifecycleStatus::Cancelling,
            },
            None => RunLifecycle::Workspace {
                status,
                phase,
                last_message_id: view.last_message_id,
                error: view.error.map(crate::control::errors::api_domain_error),
            },
        };
        let mut state = Self {
            auto_commit,
            lifecycle,
            compatibility_repair: false,
            codex_input,
            id: view.id,
            project_id: view.project_id,
            base_message_id: view.base_message_id,
            session_id: view.session_id,
            agent_id: view.agent_id,
            agent_revision: view.agent_revision,
            config: view.config,
            provider: view.provider,
            permission_profile: view.permission_profile,
            tool_approvals: view.tool_approvals,
            tool_interactions: view.tool_interactions.into_iter().map(Into::into).collect(),
            native_approvals: view.native_approvals.into_iter().map(Into::into).collect(),
            trigger: serde_json::from_value(serde_json::Value::String(view.trigger))
                .map_err(|_| serde::de::Error::custom("unknown Run trigger"))?,
            cron_id: view.cron_id,
            scheduled_at: view.scheduled_at,
            workspace_base_commit: view.workspace_base_commit,
            workspace_base_index_tree: view.workspace_base_index_tree,
            operation_id: view.operation_id,
            lease_epoch: view.lease_epoch,
        };
        // Old host records could publish terminal state before repairing canonical execution.
        state.compatibility_repair = state.execution().is_some()
            && (state.status() != status
                || state.phase() != phase
                || state.last_message_id() != legacy_last
                || state.error() != legacy_error);
        if status.is_terminal() && !state.status().is_terminal() {
            state.set_status(if status == LifecycleStatus::Completed {
                LifecycleStatus::Failed
            } else {
                status
            });
            state.set_error(legacy_error);
        }
        state
            .validate_execution()
            .map_err(serde::de::Error::custom)?;
        Ok(state)
    }
}

pub(in crate::control) fn cancellation_requested(view: &RunView) -> bool {
    serde_json::from_value::<LifecycleStatus>(serde_json::Value::String(view.status.clone()))
        .is_ok_and(|status| {
            matches!(
                status,
                LifecycleStatus::Cancelled | LifecycleStatus::Cancelling
            )
        })
}

#[cfg(test)]
mod tests;
