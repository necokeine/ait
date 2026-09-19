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
    pub text: String,
    pub model_provider: String,
    pub state: InputState,
}

/// Committed Run paired with the still-owned native writer.
pub(in crate::control) struct NativeAdmission {
    pub run: RunRecord,
    pub connection: Box<dyn CodexThreadConnection>,
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
    ) -> Result<Option<NativeAdmission>, ApiError> {
        let Command::SendMessage { session_id, text } = command else {
            return Ok(None);
        };
        let initial = self.records().read_session_records(session_id).await?;
        let session = initial
            .original
            .sessions
            .iter()
            .find(|s| s.id == *session_id)
            .ok_or_else(|| error(ErrorCode::SessionNotFound, "session not found", false))?;
        let SessionSource::CodexThread(source) = &session.source else {
            return Ok(None);
        };
        if !matches!(&source.workspace_mode, ait_domain::CodexWorkspaceMode::NativeCwd { cwd } if cwd.as_path() == std::path::Path::new(&session.workdir))
        {
            return Err(error(
                ErrorCode::CodexThreadCapabilityUnsupported,
                "native continuation requires the bound NativeCwd",
                false,
            ));
        }
        crate::control::conversation::messages::validate_message_text(text)?;
        let cwd = std::path::Path::new(&session.workdir);
        let facts = self
            .project_workspace
            .path_facts(cwd, cwd)
            .await
            .map_err(project_error)?;
        if workspace_lease.is_none_or(|lease| lease.canonical_root() != facts.canonical_root) {
            return Err(error(
                ErrorCode::CodexThreadBindingConflict,
                "native cwd changed after workspace admission",
                false,
            ));
        }
        let _admission = self.admission.read().await;
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
        let agent = require_agent(&initial.original, session.agent_id())?.clone();
        let provider = validate_config(&initial.original, &agent.config)?.clone();
        if provider.id != source.provider_id || provider.kind != ait_contracts::AgentMode::Codex {
            return Err(error(
                ErrorCode::CodexThreadBindingConflict,
                "native Agent provider changed",
                false,
            ));
        }
        let permission_profile = effective_permission_profile(
            &initial.original.settings,
            &provider,
            self.permission_limits,
        )?;
        let run_id = uuid::Uuid::new_v4().to_string();
        // Capture the store revision before contacting the native writer.
        let loaded = self.native_records(&session.project_id).await?;
        let mut connection = writer
            .resume(CodexThreadInvocation {
                request_id: run_id.clone(),
                thread_id: source.thread_id.clone(),
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
        let mut run = RunRecord {
            compatibility_repair: false,
            codex_input: Some(CodexPendingInput {
                thread_id: source.thread_id.clone(),
                text: text.clone(),
                model_provider: connection.resumed().model_provider.clone(),
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
            .commit_native_admission(loaded, &mut run, connection.as_mut())
            .await;
        if let Err(failure) = result {
            connection.close().await;
            return Err(failure);
        }
        Ok(Some(NativeAdmission { run, connection }))
    }

    async fn commit_native_admission(
        &self,
        mut loaded: RecordTransaction<ConversationContext>,
        run: &mut RunRecord,
        connection: &mut dyn CodexThreadConnection,
    ) -> Result<(), ApiError> {
        let mut snapshot = connection.resumed().history.clone();
        for attempt in 0..4 {
            if attempt > 0 {
                loaded = self.native_records(&run.project_id).await?;
                snapshot = connection.read().await.map_err(project_error)?;
            }
            let mut state = loaded.original.clone();
            let index = state
                .sessions
                .iter()
                .position(|s| Some(&s.id) == run.session_id.as_ref())
                .ok_or_else(|| {
                    error(
                        ErrorCode::SessionNotFound,
                        "native Session disappeared",
                        false,
                    )
                })?;
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
                || !matches!(&session.source, SessionSource::CodexThread(source) if source.thread_id == snapshot.id && source.provider_id == provider.id)
            {
                return Err(error(
                    ErrorCode::CodexThreadBindingConflict,
                    "native admission configuration changed",
                    false,
                ));
            }
            if !snapshot.writer_confirmed || connection.resumed().model != run.config.model {
                return Err(error(
                    ErrorCode::CodexThreadNotSynced,
                    "native writer has not confirmed an idle history",
                    false,
                ));
            }
            let mut projection =
                publication::project(&snapshot, run, &state.sessions, &state.messages)?;
            attribute_inputs(&snapshot, &mut projection.messages, &mut state.runs)?;
            run.base_message_id = projection.session.current_message_id();
            run.config
                .reasoning_effort
                .clone_from(&connection.resumed().reasoning_effort);
            projection
                .session
                .reference
                .acquire(ait_domain::RunId::new(&run.id))
                .map_err(project_error)?;
            state.sessions[index] = projection.session;
            state.messages.extend(projection.messages);
            state.runs.push(run.clone());
            let events = run_updates(&loaded.original.runs, &state.runs);
            match self.persist_records(&loaded, &state, events).await {
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
                return Err(failure);
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
        self.finish_native_run(&run, result).await
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
            .resume(CodexThreadInvocation {
                request_id: run.id.clone(),
                thread_id: input.thread_id.clone(),
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
        let snapshot = connection.resumed().history.clone();
        connection.close().await;
        self.finish_native_run(run, Ok(snapshot)).await
    }
}
