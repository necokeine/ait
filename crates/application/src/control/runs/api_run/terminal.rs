//! Terminal repair and abandoned tool-result projection.
use uuid::Uuid;

use ait_domain::{
    DomainError, ErrorCode, Message, MessageId, MessageKind, MessageOrigin, MessageRole, RunStatus,
};
use ait_ports::RunStoreError;

use crate::control::events::now;
use crate::control::persistence::{HasMessages, HasSessions};
use crate::control::runs::{RunRecord, is_terminal_run_status};

use super::store::{append_projection, status, store_failure};

/// Keep the public projection and canonical aggregate consistent on policy-driven recovery stops.
pub(in crate::control) fn interrupt(view: &mut RunRecord) {
    crate::control::tool_approvals::expire(
        view,
        if view.status() == ait_domain::LifecycleStatus::Cancelled {
            ait_domain::ToolApprovalState::Cancelled
        } else {
            ait_domain::ToolApprovalState::Expired
        },
    );
    if view.status() == ait_domain::LifecycleStatus::Completed && !needs_terminal_repair(view) {
        return;
    }
    let (status, reason) = if view.status() == ait_domain::LifecycleStatus::Cancelled {
        (RunStatus::Cancelled, ait_domain::RunStopReason::Cancelled)
    } else if view.status() == ait_domain::LifecycleStatus::LimitExceeded {
        (
            RunStatus::LimitExceeded,
            ait_domain::RunStopReason::RuntimeLimit,
        )
    } else {
        (RunStatus::Failed, ait_domain::RunStopReason::Failed)
    };
    let failure = view
        .error()
        .as_ref()
        .map(|error| DomainError::invariant(error.code, &error.message));
    let Some(execution) = view.execution_mut() else {
        return;
    };
    let reason = if execution.run.status == status {
        execution.run.stop_reason.unwrap_or(reason)
    } else {
        reason
    };
    execution
        .run
        .stop(reason, ait_domain::TimestampMs(now()), failure)
        .expect("interruption cannot complete a Run");
    for attempt in &mut execution.attempts {
        if attempt.status == ait_domain::RunAttemptStatus::Running {
            attempt.status = if status == RunStatus::Cancelled {
                ait_domain::RunAttemptStatus::Cancelled
            } else {
                ait_domain::RunAttemptStatus::Failed
            };
            attempt.ended_at = execution.run.ended_at;
            attempt.error.clone_from(&execution.run.error);
        }
    }
    for tool in &mut execution.tools {
        if !tool.status.is_terminal() {
            tool.status = if status == RunStatus::Cancelled {
                ait_domain::ToolExecutionStatus::Cancelled
            } else {
                ait_domain::ToolExecutionStatus::Failed
            };
            if tool.approval_status == ait_domain::ToolApprovalStatus::Pending {
                tool.approval_status = ait_domain::ToolApprovalStatus::NotRequired;
            }
            tool.ended_at = execution.run.ended_at;
            tool.error = Some(DomainError::invariant(
                if status == RunStatus::Cancelled {
                    ErrorCode::RunCancelled
                } else {
                    ErrorCode::RunRecoveryFailed
                },
                if tool.started_at.is_some() {
                    "execution interrupted; effect unknown; automatic replay refused"
                } else {
                    "execution stopped before dispatch"
                },
            ));
        }
    }
}

pub(in crate::control) fn needs_terminal_repair(view: &RunRecord) -> bool {
    is_terminal_run_status(view.status())
        && view.execution().is_some_and(|execution| {
            !execution.run.status.is_terminal()
                || status(&execution.run) != view.status()
                || execution
                    .attempts
                    .iter()
                    .any(|attempt| attempt.status == ait_domain::RunAttemptStatus::Running)
                || execution
                    .tools
                    .iter()
                    .any(|tool| !tool.status.is_terminal())
        })
}

/// Complete already known or abandoned results in the same terminal CAS.
///
/// This never executes or reconciles an effect and never modifies an existing Message.
pub(in crate::control) fn append_terminal_results(
    state: &mut (impl HasMessages + HasSessions),
    view: &mut RunRecord,
) -> Result<(), RunStoreError> {
    let Some(mut execution) = view.execution().cloned() else {
        return Ok(());
    };
    execution.tools.sort_by_key(|tool| {
        let sequence = state
            .messages()
            .iter()
            .find(|message| message.id == tool.assistant_message_id.as_uuid().to_string())
            .and_then(|message| message.data.as_ref())
            .and_then(|data| data["native_message"]["run_seq"].as_u64())
            .unwrap_or(0);
        (sequence, tool.tool_use_index, tool.attempt)
    });
    let run = &mut execution.run;
    for tool in &mut execution.tools {
        if tool.tool_result_message_id.is_some() || !tool.status.is_terminal() {
            continue;
        }
        if run.step_count >= run.budget.max_steps {
            break;
        }
        let result_status = match tool.status {
            ait_domain::ToolExecutionStatus::Succeeded => ait_domain::ToolResultStatus::Succeeded,
            ait_domain::ToolExecutionStatus::Failed => ait_domain::ToolResultStatus::Failed,
            ait_domain::ToolExecutionStatus::Denied => ait_domain::ToolResultStatus::Denied,
            ait_domain::ToolExecutionStatus::Cancelled => ait_domain::ToolResultStatus::Cancelled,
            _ => unreachable!("terminal tool"),
        };
        let expected = run.clone();
        let id = MessageId::new(Uuid::new_v4());
        run.step_count += 1;
        run.last_message_id = Some(id);
        let message = Message {
            id,
            project_id: run.project_id.clone(),
            parent_message_id: Some(expected.last_message_id.unwrap_or(expected.base_message_id)),
            role: MessageRole::User,
            kind: MessageKind::ToolResult,
            origin: MessageOrigin::Tool,
            sub_messages: Vec::new(),
            created_by_session_id: run.follow_session_id.clone(),
            run_id: Some(run.id.clone()),
            run_seq: Some(run.step_count),
            tool_result: Some(ait_domain::ToolResult {
                call_id: tool.call_id.clone(),
                status: result_status,
                output: tool
                    .result
                    .as_ref()
                    .map(serde_json::to_string)
                    .transpose()
                    .map_err(store_failure)?,
                error: tool.error.as_ref().map(ToString::to_string),
            }),
            git_commit: None,
            metadata: ait_domain::DomainMetadata::default(),
            created_at: run.ended_at.unwrap_or(ait_domain::TimestampMs(now())),
        };
        tool.validate_result_message(&message)
            .map_err(store_failure)?;
        append_projection(state, view, run, &expected, &message)?;
        tool.tool_result_message_id = Some(id);
    }
    run.validate().map_err(store_failure)?;
    view.install_execution(execution);
    Ok(())
}
