use super::*;
use ait_domain::*;
use serde_json::json;

fn api_state() -> RunRecord {
    let config = AgentConfiguration {
        provider_id: "api".into(),
        model: "fixture".into(),
        reasoning_effort: None,
        system_prompt: None,
    };
    let mut state: RunRecord = serde_json::from_value(json!({
        "id": "run", "project_id": "project", "base_message_id": MessageId::from_u128(1),
        "last_message_id": null, "session_id": "session", "agent_id": "agent", "agent_revision": 1,
        "config": config,
        "provider": {
            "id": "api", "name": "API", "kind": "openai", "url": null, "models": [],
        },
        "trigger": "manual", "cron_id": null, "scheduled_at": null,
        "status": "queued", "phase": "queued", "error": null,
    }))
    .unwrap();
    state.install_execution(ApiRunExecution {
        run: Run {
            id: RunId::new("run"),
            project_id: ProjectId::new("project"),
            base_message_id: MessageId::from_u128(1),
            last_message_id: None,
            follow_session_id: Some(SessionId::new("session")),
            agent_id: AgentId::new("agent"),
            agent_revision: 1,
            agent_snapshot: AgentConfigSnapshot {
                agent_id: AgentId::new("agent"),
                revision: 1,
                driver_type: "OpenAI".into(),
                connection_name: "api".into(),
                model: "fixture".into(),
                endpoint: None,
                capabilities: std::collections::BTreeSet::default(),
                default_parameters: DomainMetadata::default(),
                tool_policy: ToolPolicy::default(),
                config_digest: format!(
                    "{:x}",
                    Sha256::digest(serde_json::to_vec(&config).unwrap())
                ),
            },
            trigger: RunTrigger::Manual,
            cron_id: None,
            scheduled_at: None,
            status: RunStatus::Queued,
            phase: RunPhase::Queued,
            stop_reason: None,
            error: None,
            step_count: 0,
            budget: RunBudget {
                max_steps: 128,
                token_budget: None,
                cost_budget: None,
                max_runtime: None,
            },
            usage: RunUsage::default(),
            attempt_count: 0,
            compaction_count: 0,
            retry_policy: RetryPolicy {
                max_attempts: 3,
                initial_delay: DurationMs(100),
                max_delay: DurationMs(1000),
            },
            next_retry_at: None,
            checkpoint_id: None,
            queue_version: 0,
            queue_cursor: 0,
            dedupe_key: None,
            started_at: None,
            ended_at: None,
            created_at: TimestampMs(0),
        },
        attempts: Vec::new(),
        tools: Vec::new(),
        worker_instance_id: None,
        worker_receipts: std::collections::BTreeMap::default(),
    });
    state
}

#[test]
fn canonical_run_is_the_only_mutable_projection_source() {
    let mut state = api_state();
    let run = &mut state.execution_mut().unwrap().run;
    run.status = RunStatus::Running;
    run.phase = RunPhase::CallingAgent;
    run.last_message_id = Some(MessageId::from_u128(2));
    let value = serde_json::to_value(&state).unwrap();
    assert_eq!(value["status"], value["execution"]["run"]["status"]);
    assert_eq!(value["phase"], value["execution"]["run"]["phase"]);
    assert_eq!(
        value["last_message_id"],
        value["execution"]["run"]["last_message_id"]
    );
    let decoded: RunRecord = serde_json::from_value(value).unwrap();
    assert_eq!(decoded, state);
    assert!(decoded.view().execution.is_none());
    let event = crate::control::events::pending("run.updated", Some("run".into()), &decoded);
    assert!(event.body.get("execution").is_none());
}

#[test]
fn cancellation_intent_does_not_replace_coordinator_state() {
    let mut state = api_state();
    state.set_status(LifecycleStatus::Cancelling);
    assert_eq!(state.execution().unwrap().run.status, RunStatus::Queued);
    assert_eq!(state.view().status, "cancelling");
    let restored: RunRecord =
        serde_json::from_value(serde_json::to_value(&state).unwrap()).unwrap();
    assert_eq!(restored, state);
    state.set_status(LifecycleStatus::Cancelled);
    assert_eq!(state.view().status, "cancelled");
    state.validate_execution().unwrap();
}

#[test]
fn legacy_redundant_projection_is_rewritten_by_the_record_transaction() {
    use crate::control::persistence::codec::decode_records;
    use crate::control::runs::RunsContext;
    use ait_ports::{ControlChange, ControlRead, ControlRecord, ControlRecordKind};
    let mut value = serde_json::to_value(api_state()).unwrap();
    value["status"] = json!("running");
    value["phase"] = json!("calling_agent");
    let read = ControlRead {
        revision: 4,
        records: vec![ControlRecord {
            kind: ControlRecordKind::Run,
            id: "run".into(),
            project_id: Some("project".into()),
            value,
        }],
    };
    let loaded = decode_records::<RunsContext>(&read).unwrap();
    assert_eq!(loaded.original.runs[0].status(), LifecycleStatus::Queued);
    let changes = loaded.changes(&loaded.original).unwrap();
    assert_eq!(changes.len(), 1);
    let ControlChange::Put(record) = &changes[0] else {
        panic!("expected migration Put")
    };
    assert_eq!(record.value["status"], "queued");
    assert!(
        !serde_json::from_value::<RunRecord>(record.value.clone())
            .unwrap()
            .compatibility_repair
    );
}

#[test]
fn invalid_identity_and_unknown_lifecycle_fail_without_echoing_payload() {
    let mut value = serde_json::to_value(api_state()).unwrap();
    value["execution"]["run"]["project_id"] = json!("PRIVATE_MISMATCH");
    let failure = serde_json::from_value::<RunRecord>(value).unwrap_err();
    assert!(!failure.to_string().contains("PRIVATE_MISMATCH"));
    let mut state = api_state();
    state.agent_id = "another".into();
    assert!(serde_json::to_value(&state).is_err());
}

#[test]
fn canonical_terminal_state_wins_over_stale_outer_terminal_projection() {
    let mut state = api_state();
    let run = &mut state.execution_mut().unwrap().run;
    run.status = RunStatus::Completed;
    run.phase = RunPhase::Terminal;
    run.stop_reason = Some(RunStopReason::Completed);
    run.ended_at = Some(TimestampMs(3));
    let mut value = serde_json::to_value(&state).unwrap();
    value["status"] = json!("failed");
    let migrated: RunRecord = serde_json::from_value(value).unwrap();
    assert_eq!(migrated.execution(), state.execution());
    assert_eq!(migrated.view().status, "completed");
    assert!(migrated.compatibility_repair);
}

#[test]
fn legacy_completion_cannot_grant_coordinator_completion() {
    let mut value = serde_json::to_value(api_state()).unwrap();
    value["status"] = json!("completed");
    value["phase"] = json!("terminal");
    let migrated: RunRecord = serde_json::from_value(value).unwrap();
    assert_eq!(migrated.execution().unwrap().run.status, RunStatus::Failed);
    assert_eq!(migrated.view().status, "failed");
    assert!(migrated.compatibility_repair);
}

#[test]
fn unknown_lifecycle_values_are_rejected_without_echoing_input() {
    for field in ["status", "phase", "trigger"] {
        let mut value = serde_json::to_value(api_state()).unwrap();
        value[field] = json!("PRIVATE_UNKNOWN");
        let failure = serde_json::from_value::<RunRecord>(value).unwrap_err();
        assert!(!failure.to_string().contains("PRIVATE_UNKNOWN"));
    }
}

#[test]
fn fixed_configuration_cannot_drift_from_the_execution_snapshot() {
    let mut value = serde_json::to_value(api_state()).unwrap();
    value["config"]["reasoning_effort"] = json!("PRIVATE_EFFORT");
    let failure = serde_json::from_value::<RunRecord>(value).unwrap_err();
    assert!(!failure.to_string().contains("PRIVATE_EFFORT"));
    let mut value = serde_json::to_value(api_state()).unwrap();
    value["provider"]["kind"] = json!("deepseek");
    assert!(serde_json::from_value::<RunRecord>(value).is_err());
}
