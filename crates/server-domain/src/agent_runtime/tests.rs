use serde_json::json;

use super::{AgentRuntimeStatus, PersistedAgentRuntimeRecord};

#[test]
fn decodes_paseo_stored_agent_defaults() {
    let record: PersistedAgentRuntimeRecord = serde_json::from_value(json!({
        "id": "agent-1",
        "provider": "codex",
        "cwd": "/tmp/project",
        "createdAt": "2026-09-20T10:00:00.000Z",
        "updatedAt": "2026-09-20T10:01:00.000Z"
    }))
    .expect("Paseo-compatible record should decode");

    assert_eq!(record.last_status, AgentRuntimeStatus::Closed);
    assert!(record.labels.is_empty());
    assert!(!record.requires_attention);
    assert!(!record.internal);
}

#[test]
fn preserves_provider_runtime_fields_on_round_trip() {
    let input = json!({
        "id": "agent-1",
        "provider": "codex",
        "cwd": "/tmp/project",
        "workspaceId": "wks_0123456789abcdef",
        "createdAt": "2026-09-20T10:00:00.000Z",
        "updatedAt": "2026-09-20T10:01:00.000Z",
        "lastStatus": "idle",
        "config": {"model": "gpt-6", "thinkingOptionId": "high"},
        "persistence": {"provider": "codex", "sessionId": "thread-1"},
        "features": [{"type": "toggle", "id": "search", "value": true}],
        "owner": {"kind": "daemon", "executionId": "run-1"}
    });
    let record: PersistedAgentRuntimeRecord =
        serde_json::from_value(input.clone()).expect("record should decode");

    let output = serde_json::to_value(record).expect("record should encode");
    assert_eq!(output["config"]["model"], "gpt-6");
    assert_eq!(output["persistence"]["sessionId"], "thread-1");
    assert_eq!(output["owner"], input["owner"]);
}
