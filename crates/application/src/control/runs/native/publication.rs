//! Shared deterministic publication for native completion, sync and recovery.
use ait_contracts::ApiError;
use ait_domain::{
    ErrorCode, LifecyclePhase, LifecycleStatus, Message, MessageOrigin, MessageRole,
    NativeApprovalStatus, ProviderHistoryCompleteness, SessionSource,
};
use ait_ports::{CodexThreadSnapshot, ControlStoreError};
use serde_json::Value;

use super::{InputState, RunRecord, conflict};
use crate::control::{
    LocalControlService,
    approvals::expire_pending_native_approvals,
    codex_history::{CodexImportIdentity, CodexMaterialization, materialize_thread},
    conversation::{MessageRecord, SessionRecord},
    errors::{error, project_error, store_error},
    events::pending,
};

pub(super) fn project(
    snapshot: &CodexThreadSnapshot,
    run: &RunRecord,
    sessions: &[SessionRecord],
    messages: &[MessageRecord],
) -> Result<CodexMaterialization, ApiError> {
    let projection = materialize_thread(
        snapshot,
        CodexImportIdentity {
            provider_id: &run.provider.id,
            project_id: &run.project_id,
            agent_id: &run.agent_id,
        },
        sessions,
        messages,
    )
    .map_err(project_error)?;
    if !matches!(&projection.session.source, SessionSource::CodexThread(source) if source.history_completeness == ProviderHistoryCompleteness::Full)
    {
        return Err(error(
            ErrorCode::CodexThreadNotSynced,
            "native history is not a full terminal snapshot",
            false,
        ));
    }
    Ok(projection)
}

/// Applies provenance only to newly materialized nodes; immutable existing nodes are reused.
#[allow(
    clippy::too_many_lines,
    reason = "Correlation and immutable provenance must be validated before any publication"
)]
pub(in crate::control) fn attribute_inputs(
    snapshot: &CodexThreadSnapshot,
    messages: &mut [MessageRecord],
    runs: &mut [RunRecord],
) -> Result<(), ApiError> {
    for run in runs {
        let Some(input) = &run.codex_input else {
            continue;
        };
        if input.thread_id != snapshot.id
            || matches!(input.state, InputState::Published | InputState::Rejected)
        {
            continue;
        }
        let matches = snapshot
            .turns
            .iter()
            .flat_map(|turn| turn.items.iter().map(move |item| (turn, item)))
            .filter(|(_, item)| {
                item.get("type").and_then(Value::as_str) == Some("userMessage")
                    && item.get("clientId").and_then(Value::as_str) == Some(&run.id)
            })
            .collect::<Vec<_>>();
        if matches.len() > 1 {
            return Err(error(
                ErrorCode::CodexInputOutcomeUnknown,
                "native input correlation is ambiguous",
                false,
            ));
        }
        let Some((turn, _)) = matches.first() else {
            continue;
        };
        if !matches!(turn.status.as_str(), "completed" | "failed" | "interrupted") {
            continue;
        }
        let mut seq = 0;
        let mut last = None;
        for record in messages.iter_mut().filter(|message| {
            message
                .data
                .as_ref()
                .and_then(|data| data.pointer("/native_message/metadata/codex/turn_id"))
                .and_then(Value::as_str)
                == Some(&turn.id)
        }) {
            if seq == 0 && record.parent_message_id.as_deref() != Some(&run.base_message_id) {
                return Err(error(
                    ErrorCode::CodexInputOutcomeUnknown,
                    "native Turn no longer extends the admitted Run base",
                    false,
                ));
            }
            let mut message = Message::try_from(&*record).map_err(project_error)?;
            seq += 1;
            message.run_id = Some(ait_domain::RunId::new(&run.id));
            message.run_seq = Some(seq);
            message.origin = if message.role == MessageRole::User {
                MessageOrigin::Human
            } else {
                MessageOrigin::Agent
            };
            if let Some(codex) = message
                .metadata
                .0
                .get_mut("codex")
                .and_then(Value::as_object_mut)
            {
                codex.insert("submitted_via".into(), Value::String("ait".into()));
                codex.insert("workspace_mode".into(), Value::String("native_cwd".into()));
                codex.insert("thread_id".into(), Value::String(snapshot.id.clone()));
            }
            message
                .validate()
                .map_err(|failure| project_error(failure.into()))?;
            last = Some(record.id.clone());
            *record = MessageRecord::from(message);
        }
        // Partial/cold reads must not convert a pending intent into a published Run.
        if last.is_none() {
            continue;
        }
        run.codex_input
            .as_mut()
            .expect("matched native input")
            .state = InputState::Published;
        run.set_last_message_id(last);
        // History still publishes after a local ceiling, but cannot erase that Run outcome.
        if run.status() != LifecycleStatus::LimitExceeded {
            run.set_status(match turn.status.as_str() {
                "completed" | "interrupted" if run.status() == LifecycleStatus::Cancelling => {
                    LifecycleStatus::Cancelled
                }
                "completed" => LifecycleStatus::Completed,
                "interrupted" => LifecycleStatus::Interrupted,
                _ => LifecycleStatus::Failed,
            });
            run.set_phase(Some(LifecyclePhase::Terminal));
            run.set_error(if turn.status == "completed" {
                None
            } else {
                Some(error(
                    if turn.status == "interrupted" {
                        ErrorCode::RunCancelled
                    } else {
                        ErrorCode::ProviderFailed
                    },
                    "native Turn did not complete successfully",
                    false,
                ))
            });
        }
        expire_pending_native_approvals(
            run,
            if run.status() == LifecycleStatus::Cancelled {
                NativeApprovalStatus::Cancelled
            } else {
                NativeApprovalStatus::Expired
            },
        );
    }
    Ok(())
}

impl LocalControlService {
    #[allow(
        clippy::too_many_lines,
        reason = "Terminal history, approvals and Session release share one CAS transaction"
    )]
    pub(super) async fn finish_native_run(
        &self,
        admitted: &RunRecord,
        result: Result<CodexThreadSnapshot, ApiError>,
    ) -> Result<RunRecord, ApiError> {
        for _ in 0..4 {
            let loaded = self.native_records(&admitted.project_id).await?;
            let mut state = loaded.original.clone();
            let index = state
                .runs
                .iter()
                .position(|run| run.id == admitted.id)
                .ok_or_else(conflict)?;
            let mut run = state.runs[index].clone();
            if run.status().is_terminal() {
                return Ok(run);
            }
            let session_index = state
                .sessions
                .iter()
                .position(|s| Some(&s.id) == run.session_id.as_ref())
                .ok_or_else(conflict)?;
            if state.sessions[session_index].active_run_id() != Some(&run.id) {
                return Err(conflict());
            }
            state.sessions[session_index]
                .reference
                .release(&ait_domain::RunId::new(&run.id));
            let publication = match &result {
                Ok(snapshot) => (|| {
                    if !snapshot.writer_confirmed
                        || run
                            .codex_input
                            .as_ref()
                            .is_none_or(|input| input.thread_id != snapshot.id)
                    {
                        return Err(error(
                            ErrorCode::CodexInputOutcomeUnknown,
                            "native terminal history was not confirmed",
                            false,
                        ));
                    }
                    let mut projection = project(snapshot, &run, &state.sessions, &state.messages)?;
                    attribute_inputs(
                        snapshot,
                        &mut projection.messages,
                        std::slice::from_mut(&mut run),
                    )?;
                    if run
                        .codex_input
                        .as_ref()
                        .is_none_or(|input| input.state != InputState::Published)
                    {
                        return Err(error(
                            ErrorCode::CodexInputOutcomeUnknown,
                            "native input acceptance is unknown; input will not be replayed",
                            false,
                        ));
                    }
                    Ok(projection)
                })(),
                Err(failure) => Err(failure.clone()),
            };
            match publication {
                Ok(projection) => {
                    state.sessions[session_index] = projection.session;
                    state.messages.extend(projection.messages);
                }
                Err(failure) => {
                    if matches!(
                        failure.code,
                        ErrorCode::CodexInputNotAccepted
                            | ErrorCode::RunCancelled
                            | ErrorCode::CodexThreadActiveElsewhere
                            | ErrorCode::CodexThreadWriterBusy
                            | ErrorCode::CodexThreadCapabilityUnsupported
                    ) {
                        run.codex_input.as_mut().expect("native Run").state = InputState::Rejected;
                    }
                    run.set_status(if failure.code == ErrorCode::RunLimitExceeded {
                        LifecycleStatus::LimitExceeded
                    } else if failure.code == ErrorCode::RunCancelled {
                        LifecycleStatus::Cancelled
                    } else if run
                        .codex_input
                        .as_ref()
                        .is_some_and(|input| input.state == InputState::Rejected)
                    {
                        LifecycleStatus::Failed
                    } else {
                        LifecycleStatus::Interrupted
                    });
                    run.set_phase(Some(LifecyclePhase::Terminal));
                    run.set_error(Some(failure));
                }
            }
            if run
                .codex_input
                .as_ref()
                .is_some_and(|input| input.created_thread && input.state == InputState::Rejected)
            {
                // No first input was accepted, so this Thread may have no rollout.
                // Keep immutable native roots, but release its otherwise unusable binding.
                let root = state
                    .projects
                    .iter()
                    .find(|project| project.id == run.project_id)
                    .ok_or_else(conflict)?
                    .root_message_id
                    .clone();
                let session = &mut state.sessions[session_index];
                session
                    .reference
                    .reconcile(
                        session.reference.head(),
                        session.version(),
                        ait_domain::MessageId::parse(&root).map_err(|_| conflict())?,
                    )
                    .map_err(project_error)?;
                session.source = SessionSource::Managed;
            }
            if run.status() == LifecycleStatus::Completed
                && run
                    .auto_commit
                    .as_ref()
                    .is_some_and(super::super::git_commit::AutoCommit::pending)
            {
                run.set_status(LifecycleStatus::Settling);
                run.set_phase(Some(LifecyclePhase::Settling));
                state.sessions[session_index]
                    .reference
                    .acquire(ait_domain::RunId::new(&run.id))
                    .map_err(project_error)?;
            } else if run.status() != LifecycleStatus::Completed
                && let Some(commit) = run.auto_commit.as_mut()
                && commit.pending()
            {
                commit.view.status = ait_contracts::RunCommitStatus::Skipped;
                commit.view.reason = Some("Codex execution did not complete successfully".into());
            }
            let approval_status = if run.status() == LifecycleStatus::Cancelled {
                NativeApprovalStatus::Cancelled
            } else {
                NativeApprovalStatus::Expired
            };
            expire_pending_native_approvals(&mut run, approval_status);
            state.runs[index] = run.clone();
            match self
                .persist_records(
                    &loaded,
                    &state,
                    vec![pending("run.updated", Some(run.id.clone()), &run.view())],
                )
                .await
            {
                Ok(()) => {
                    self.notify_approval_waiters(&run.view());
                    let _ = self.store.clear_progress(&run.id).await;
                    return Ok(run);
                }
                Err(ControlStoreError::Conflict) => {}
                Err(failure) => return Err(store_error(failure)),
            }
        }
        Err(conflict())
    }
}
