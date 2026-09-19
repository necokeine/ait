//! Native inputs are durable intents until an authoritative Turn can be published.
use std::sync::{Arc, atomic::Ordering};

use ait_contracts::{ApiError, Command};
use ait_domain::{ErrorCode, LifecyclePhase, LifecycleStatus, SessionSource};
use ait_ports::{
    CodexThreadConnection, CodexThreadInvocation, ControlFilter, ControlRecordKind,
    ControlStoreError,
};
use futures_util::FutureExt;
use serde::{Deserialize, Serialize};

use super::{RunLifecycle, RunRecord, finalization::RunControl, progress::ProgressPump};
use crate::control::{
    LocalControlService,
    catalog::{require_agent, validate_config},
    conversation::ConversationContext,
    errors::{error, project_error, store_error},
    events::pending,
    permissions::effective_permission_profile,
    persistence::transaction::RecordTransaction,
};

mod admission;
mod publication;
pub(in crate::control) use publication::attribute_inputs;

/// Durable delivery knowledge; unknown inputs must be reconciled without replay.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(in crate::control) enum InputState {
    Queued,
    SendUnknown,
    Published,
    Rejected,
}

/// Recovery uses this correlation record, never an optimistic immutable Message.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub(in crate::control) struct CodexPendingInput {
    pub thread_id: String,
    pub created_thread: bool,
    pub text: String,
    pub model_provider: String,
    pub state: InputState,
}

/// Committed Run paired with the still-owned native writer.
pub(in crate::control) struct NativeAdmission {
    pub run: RunRecord,
    pub connection: Box<dyn CodexThreadConnection>,
    pub execution_lease: Option<crate::control::admission::WorkspaceWriteLease>,
}

fn conflict() -> ApiError {
    error(
        ErrorCode::RunQueueConflict,
        "native Run transaction did not settle",
        true,
    )
}

/// Publishes reconciled Run state through the same outbox consumed by live clients.
pub(in crate::control) fn run_updates(
    before: &[RunRecord],
    after: &[RunRecord],
) -> Vec<ait_ports::PendingEvent> {
    after
        .iter()
        .filter(|run| before.iter().find(|old| old.id == run.id) != Some(*run))
        .map(|run| pending("run.updated", Some(run.id.clone()), &run.view()))
        .collect()
}

impl LocalControlService {
    async fn native_records(
        &self,
        project_id: &str,
    ) -> Result<RecordTransaction<ConversationContext>, ApiError> {
        self.records()
            .read_records(vec![
                ControlFilter::id(ControlRecordKind::Project, project_id),
                ControlFilter::all(ControlRecordKind::Agent),
                ControlFilter::all(ControlRecordKind::Provider),
                ControlFilter::all(ControlRecordKind::Session),
                ControlFilter::all(ControlRecordKind::Settings),
                ControlFilter::project(ControlRecordKind::Message, project_id),
                ControlFilter::project(ControlRecordKind::Run, project_id),
            ])
            .await
    }

    /// Acquires and verifies native context before atomically committing the input intent.
    #[allow(
        clippy::too_many_lines,
        reason = "Writer validation and durable Run admission share one ownership boundary"
    )]
    pub(in crate::control) async fn admit_native_command(
        &self,
        command: &Command,
        control: &Arc<RunControl>,
        workspace_lease: Option<&crate::control::admission::WorkspaceWriteLease>,
        derive_source_locked: bool,
    ) -> Result<Option<NativeAdmission>, ApiError> {
        let Some(plan) = self.native_plan(command, derive_source_locked).await? else {
            return Ok(None);
        };
        let session = &plan.session;
        let text = &plan.text;
        let session_id = &session.id;
        crate::control::admission::ensure_idle(session)?;
        crate::control::conversation::messages::validate_message_text(text)?;
        let source = match &session.source {
            SessionSource::CodexThread(source) => Some(source.as_ref()),
            SessionSource::Managed => None,
        };
        if source.is_none() {
            crate::control::project::worktrees::prepare_command_session_worktrees(
                self.project_workspace.as_ref(),
                workspace_lease.cloned(),
                &plan.loaded.original,
                command,
                &mut Vec::new(),
            )
            .await?;
        }
        let cwd = std::path::Path::new(&session.workdir);
        let facts = self
            .project_workspace
            .path_facts(cwd, cwd)
            .await
            .map_err(project_error)?;
        let expected_root = if source.is_some() {
            facts.canonical_root.as_path()
        } else {
            std::path::Path::new(
                &plan
                    .loaded
                    .original
                    .projects
                    .iter()
                    .find(|project| project.id == session.project_id)
                    .ok_or_else(conflict)?
                    .workdir,
            )
        };
        if workspace_lease.is_none_or(|lease| lease.canonical_root() != expected_root) {
            return Err(error(
                ErrorCode::CodexThreadBindingConflict,
                "native cwd changed after workspace admission",
                false,
            ));
        }
        let execution_lease = if workspace_lease
            .is_some_and(|lease| lease.canonical_root() != facts.canonical_root)
        {
            Some(
                self.project_workspace
                    .acquire_lease(&facts.canonical_root)
                    .await
                    .map_err(project_error)?,
            )
        } else {
            None
        };
        if self.draining.load(Ordering::Acquire) {
            return Err(error(ErrorCode::RunCancelled, "daemon is draining", false));
        }
        let writer = self.codex_thread_writer.as_ref().ok_or_else(|| {
            error(
                ErrorCode::CodexThreadCapabilityUnsupported,
                "native Codex writer is unavailable",
                false,
            )
        })?;
        let agent = plan.agent.clone();
        let provider = validate_config(&plan.loaded.original, &agent.config)?.clone();
        if source.is_some_and(|source| provider.id != source.provider_id) {
            return Err(error(
                ErrorCode::CodexThreadBindingConflict,
                "native Agent provider changed",
                false,
            ));
        }
        let permission_profile = effective_permission_profile(
            &plan.loaded.original.settings,
            &provider,
            self.permission_limits,
        )?;
        let run_id = uuid::Uuid::new_v4().to_string();
        let developer_instructions = if source.is_none() {
            plan.loaded
                .original
                .messages
                .iter()
                .find(|message| message.id == session.current_message_id())
                .and_then(|message| message.text.clone())
        } else {
            None
        };
        let mut connection = writer
            .open(CodexThreadInvocation {
                request_id: run_id.clone(),
                thread_id: source.map(|source| source.thread_id.clone()),
                developer_instructions,
                prompt: text.clone(),
                cwd: session.workdir.clone().into(),
                model: agent.config.model.clone(),
                reasoning_effort: agent.config.reasoning_effort.clone(),
                permission_profile,
                approvals: Arc::new(self.clone()),
                cancellation: control.cancellation.clone(),
            })
            .await
            .map_err(project_error)?;
        let auto_commit = self
            .capture_auto_commit(
                cwd,
                plan.loaded
                    .original
                    .settings
                    .0
                    .get("codex.auto_commit")
                    .and_then(serde_json::Value::as_bool)
                    .unwrap_or(false),
            )
            .await;
        let mut run = RunRecord {
            auto_commit,
            compatibility_repair: false,
            codex_input: Some(CodexPendingInput {
                thread_id: connection.prepared().history.id.clone(),
                created_thread: source.is_none(),
                text: text.clone(),
                model_provider: connection.prepared().model_provider.clone(),
                state: InputState::Queued,
            }),
            lifecycle: RunLifecycle::queued(),
            id: run_id,
            project_id: session.project_id.clone(),
            base_message_id: session.current_message_id(),
            session_id: Some(session_id.clone()),
            agent_id: agent.id.clone(),
            agent_revision: agent.revision,
            config: agent.config,
            provider,
            permission_profile,
            native_approvals: Vec::new(),
            tool_approvals: Vec::new(),
            tool_interactions: Vec::new(),
            trigger: ait_domain::RunTrigger::Manual,
            cron_id: None,
            scheduled_at: None,
            workspace_base_commit: None,
            workspace_base_index_tree: None,
            operation_id: None,
            lease_epoch: 0,
        };
        let result = self
            .commit_native_admission(
                plan,
                &mut run,
                connection.as_mut(),
                (command, derive_source_locked),
            )
            .await;
        if let Err(failure) = result {
            connection.close().await;
            return Err(failure);
        }
        Ok(Some(NativeAdmission {
            run,
            connection,
            execution_lease,
        }))
    }

    async fn commit_native_admission(
        &self,
        plan: admission::Plan,
        run: &mut RunRecord,
        connection: &mut dyn CodexThreadConnection,
        intent: (&Command, bool),
    ) -> Result<(), ApiError> {
        let (command, derive_source_locked) = intent;
        let mut loaded = plan.loaded;
        let mut snapshot = connection.prepared().history.clone();
        for attempt in 0..4 {
            if attempt > 0 {
                let refreshed = self
                    .native_plan(command, derive_source_locked)
                    .await?
                    .ok_or_else(conflict)?;
                if refreshed.session != plan.session
                    || refreshed.agent != plan.agent
                    || refreshed.new_session != plan.new_session
                {
                    return Err(conflict());
                }
                loaded = refreshed.loaded;
                snapshot = connection.read().await.map_err(project_error)?;
            }
            let mut state = loaded.original.clone();
            let index =
                admission::stage_session(&mut state, &plan.session, &plan.agent, plan.new_session)?;
            let session = &state.sessions[index];
            crate::control::admission::ensure_idle(session)?;
            let agent = require_agent(&state, session.agent_id())?.clone();
            let provider = validate_config(&state, &agent.config)?;
            if agent.id != run.agent_id
                || agent.revision != run.agent_revision
                || agent.config != run.config
                || provider != &run.provider
                || session.workdir != snapshot.cwd
                || effective_permission_profile(&state.settings, provider, self.permission_limits)?
                    != run.permission_profile
                || match &session.source {
                    SessionSource::CodexThread(source) => {
                        source.thread_id != snapshot.id || source.provider_id != provider.id
                    }
                    SessionSource::Managed => {
                        session != &plan.session || !snapshot.turns.is_empty()
                    }
                }
            {
                return Err(error(
                    ErrorCode::CodexThreadBindingConflict,
                    "native admission configuration changed",
                    false,
                ));
            }
            admission::validate_writer(&state, run, &snapshot, connection.prepared())?;
            if matches!(state.sessions[index].source, SessionSource::Managed) {
                let initial =
                    publication::project(&snapshot, run, &state.sessions, &state.messages)?;
                state.sessions[index].source = initial.session.source;
            }
            let mut projection =
                publication::project(&snapshot, run, &state.sessions, &state.messages)?;
            attribute_inputs(&snapshot, &mut projection.messages, &mut state.runs)?;
            run.base_message_id = projection.session.current_message_id();
            run.config
                .reasoning_effort
                .clone_from(&connection.prepared().reasoning_effort);
            projection
                .session
                .reference
                .acquire(ait_domain::RunId::new(&run.id))
                .map_err(project_error)?;
            state.sessions[index] = projection.session;
            state.messages.extend(projection.messages);
            state.runs.push(run.clone());
            let mut events = run_updates(&loaded.original.runs, &state.runs);
            events.push(pending(
                if plan.new_session {
                    "session.created"
                } else {
                    "session.updated"
                },
                Some(state.sessions[index].id.clone()),
                &state.sessions[index].view(),
            ));
            // Shutdown never waits on app-server I/O. Fence only durable admission.
            let admission = self.admission.read().await;
            if self.draining.load(Ordering::Acquire) {
                return Err(error(ErrorCode::RunCancelled, "daemon is draining", false));
            }
            let result = self.persist_records(&loaded, &state, events).await;
            drop(admission);
            match result {
                Ok(()) => return Ok(()),
                Err(ControlStoreError::Conflict) => {}
                Err(failure) => return Err(store_error(failure)),
            }
            // Retry validation against the configured effort, before adopting the effective value again.
            run.config
                .reasoning_effort
                .clone_from(&agent.config.reasoning_effort);
        }
        Err(conflict())
    }

    /// Sends once, drains progress and closes the writer before publishing terminal state.
    pub(in crate::control) async fn supervise_native_run(
        &self,
        run_id: String,
        control: Arc<RunControl>,
        mut connection: Box<dyn CodexThreadConnection>,
    ) -> Result<RunRecord, ApiError> {
        let run = match self.mark_native_send(&run_id).await {
            Ok(run) => run,
            Err(failure) => {
                connection.close().await;
                let loaded = self.read_run_records(&run_id).await?;
                let queued = loaded
                    .original
                    .runs
                    .iter()
                    .find(|run| run.id == run_id)
                    .ok_or_else(conflict)?;
                return self
                    .finish_native_run(
                        queued,
                        Err(error(
                            ErrorCode::CodexInputNotAccepted,
                            format!("native input was not sent: {}", failure.message),
                            false,
                        )),
                    )
                    .await;
            }
        };
        let pump = ProgressPump::start(Arc::clone(&self.store), &run);
        let result =
            if control.cancellation.is_cancelled() || run.status() == LifecycleStatus::Cancelling {
                Err(error(
                    ErrorCode::RunCancelled,
                    "native input cancelled before sending",
                    false,
                ))
            } else {
                std::panic::AssertUnwindSafe(connection.start(pump.reporter()))
                    .catch_unwind()
                    .await
                    .unwrap_or_else(|_| {
                        Err(ait_domain::DomainError::invariant(
                            ErrorCode::CodexInputOutcomeUnknown,
                            "native executor stopped after input admission",
                        ))
                    })
                    .map_err(project_error)
            };
        connection.close().await;
        let _ = pump.finish().await;
        let run = self.finish_native_run(&run, result).await?;
        self.finalize_native_git(run).await
    }

    async fn mark_native_send(&self, run_id: &str) -> Result<RunRecord, ApiError> {
        for _ in 0..4 {
            let loaded = self.read_run_records(run_id).await?;
            let mut state = loaded.original.clone();
            let run = state
                .runs
                .iter_mut()
                .find(|run| run.id == run_id)
                .ok_or_else(|| error(ErrorCode::InvalidRun, "native Run disappeared", false))?;
            let input = run.codex_input.as_mut().ok_or_else(conflict)?;
            if input.state != InputState::Queued {
                return Err(error(
                    ErrorCode::CodexInputOutcomeUnknown,
                    "native input must not be replayed",
                    false,
                ));
            }
            input.state = InputState::SendUnknown;
            if run.status() != LifecycleStatus::Cancelling {
                run.set_status(LifecycleStatus::Running);
                run.set_phase(Some(LifecyclePhase::CallingAgent));
            }
            let run = run.clone();
            match self
                .persist_records(
                    &loaded,
                    &state,
                    vec![pending("run.updated", Some(run.id.clone()), &run.view())],
                )
                .await
            {
                Ok(()) => return Ok(run),
                Err(ControlStoreError::Conflict) => {}
                Err(failure) => return Err(store_error(failure)),
            }
        }
        Err(conflict())
    }

    /// Recovery only reads/correlates; a clientId is not an idempotency key.
    pub(in crate::control) async fn recover_native_run(
        &self,
        run: &RunRecord,
    ) -> Result<RunRecord, ApiError> {
        crate::control::permissions::validate_run_permission_ceiling(
            run.permission_profile,
            self.permission_limits,
        )
        .map_err(project_error)?;
        let input = run.codex_input.as_ref().ok_or_else(conflict)?;
        if input.state == InputState::Queued {
            return self
                .finish_native_run(
                    run,
                    Err(error(
                        ErrorCode::CodexInputNotAccepted,
                        "daemon stopped before native input delivery; input was not replayed",
                        false,
                    )),
                )
                .await;
        }
        let state = self.read_run_records(&run.id).await?.original;
        let session = state
            .sessions
            .iter()
            .find(|s| Some(&s.id) == run.session_id.as_ref())
            .ok_or_else(conflict)?;
        let writer = self.codex_thread_writer.as_ref().ok_or_else(|| {
            error(
                ErrorCode::CodexThreadCapabilityUnsupported,
                "native writer unavailable for recovery",
                false,
            )
        })?;
        let mut connection = writer
            .open(CodexThreadInvocation {
                request_id: run.id.clone(),
                thread_id: Some(input.thread_id.clone()),
                developer_instructions: None,
                prompt: input.text.clone(),
                cwd: session.workdir.clone().into(),
                model: run.config.model.clone(),
                reasoning_effort: run.config.reasoning_effort.clone(),
                permission_profile: run.permission_profile,
                approvals: Arc::new(self.clone()),
                cancellation: tokio_util::sync::CancellationToken::new(),
            })
            .await
            .map_err(project_error)?;
        let snapshot = connection.prepared().history.clone();
        connection.close().await;
        let run = self.finish_native_run(run, Ok(snapshot)).await?;
        self.finalize_native_git(run).await
    }
}
