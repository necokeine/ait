use std::collections::BTreeSet;

use super::*;
use crate::{AgentCapability, DomainMetadata, ToolPolicy};

fn snapshot() -> AgentConfigSnapshot {
    AgentConfigSnapshot {
        agent_id: AgentId::new("agent-1"),
        revision: 3,
        driver_type: "codex".into(),
        connection_name: "default".into(),
        model: "gpt-5".into(),
        endpoint: None,
        capabilities: BTreeSet::from([AgentCapability::Text]),
        default_parameters: DomainMetadata::default(),
        tool_policy: ToolPolicy::default(),
        config_digest: "a".repeat(64),
    }
}

fn run() -> Run {
    Run {
        id: RunId::new("run-1"),
        project_id: ProjectId::new("project-1"),
        base_message_id: MessageId::from_u128(1),
        last_message_id: None,
        follow_session_id: Some(SessionId::new("session-1")),
        agent_id: AgentId::new("agent-1"),
        agent_revision: 3,
        agent_snapshot: snapshot(),
        trigger: RunTrigger::Manual,
        cron_id: None,
        scheduled_at: None,
        status: RunStatus::Queued,
        phase: RunPhase::Queued,
        stop_reason: None,
        error: None,
        step_count: 0,
        budget: RunBudget {
            max_steps: 10,
            token_budget: Some(1_000),
            cost_budget: None,
            max_runtime: None,
        },
        usage: RunUsage::default(),
        attempt_count: 0,
        compaction_count: 0,
        retry_policy: RetryPolicy {
            max_attempts: 3,
            initial_delay: DurationMs(100),
            max_delay: DurationMs(1_000),
        },
        next_retry_at: None,
        checkpoint_id: None,
        queue_version: 0,
        queue_cursor: 0,
        dedupe_key: None,
        started_at: None,
        ended_at: None,
        created_at: TimestampMs(10),
    }
}

#[test]
fn fixed_snapshot_and_trigger_shape_are_validated() {
    let mut candidate = run();
    candidate.validate().unwrap();

    candidate.agent_revision = 4;
    assert_eq!(
        candidate.validate().unwrap_err().code,
        ErrorCode::InvalidRun
    );

    candidate.agent_revision = 3;
    candidate.trigger = RunTrigger::Cron;
    assert_eq!(
        candidate.validate().unwrap_err().code,
        ErrorCode::InvalidRun
    );
}

#[test]
fn completion_barrier_detects_a_queue_race() {
    let ready = RunTerminationReadiness {
        blockers: BTreeSet::new(),
        observed_queue_version: 7,
        current_queue_version: 8,
    };
    assert!(!ready.can_complete());
    assert!(
        RunTerminationReadiness {
            current_queue_version: 7,
            ..ready.clone()
        }
        .can_complete()
    );
    assert!(
        !RunTerminationReadiness {
            blockers: BTreeSet::from([RunTerminationBlocker::ToolPending]),
            current_queue_version: 7,
            ..ready
        }
        .can_complete()
    );
}

#[test]
fn run_status_serializes_as_snake_case() {
    let encoded = serde_json::to_string(&RunStatus::WaitingApproval).unwrap();
    assert_eq!(encoded, "\"waiting_approval\"");
    assert_eq!(
        serde_json::from_str::<RunStatus>(&encoded).unwrap(),
        RunStatus::WaitingApproval
    );
}

#[test]
fn attempts_and_queue_items_reject_invalid_sequence_state() {
    let mut attempt = RunAttempt {
        id: RunAttemptId::new("attempt-1"),
        run_id: RunId::new("run-1"),
        number: 1,
        reason: RunAttemptReason::Initial,
        checkpoint_id: None,
        status: RunAttemptStatus::Running,
        error: None,
        started_at: TimestampMs(1),
        ended_at: None,
    };
    attempt.validate().unwrap();
    attempt.status = RunAttemptStatus::Completed;
    assert_eq!(attempt.validate().unwrap_err().code, ErrorCode::InvalidRun);

    let mut item = RunQueueItem {
        id: RunQueueItemId::new("item-1"),
        run_id: RunId::new("run-1"),
        sequence: 1,
        kind: RunQueueItemKind::new("user_input"),
        payload: serde_json::json!({"message": "continue"}),
        dedupe_key: Some("request-1".into()),
        status: RunQueueItemStatus::Pending,
        created_at: TimestampMs(1),
        consumed_at: None,
    };
    item.validate().unwrap();
    item.sequence = 0;
    assert_eq!(item.validate().unwrap_err().code, ErrorCode::InvalidRun);
}
