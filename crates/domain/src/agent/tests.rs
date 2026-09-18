use super::*;

fn revision() -> AgentRevision {
    AgentRevision {
        agent_id: AgentId::new("agent-1"),
        revision: 2,
        driver_type: "codex".into(),
        connection_name: "default".into(),
        model: "gpt-5".into(),
        endpoint: None,
        capabilities: BTreeSet::from([AgentCapability::Text, AgentCapability::ToolUse]),
        default_parameters: DomainMetadata::default(),
        tool_policy: ToolPolicy::default(),
        config_digest: "a".repeat(64),
        created_at: TimestampMs(1),
    }
}

#[test]
fn revision_snapshot_is_fixed_and_serializable() {
    let revision = revision();
    revision.validate().unwrap();
    let snapshot = revision.snapshot();
    let encoded = serde_json::to_string(&snapshot).unwrap();
    let decoded: AgentConfigSnapshot = serde_json::from_str(&encoded).unwrap();
    assert_eq!(decoded, snapshot);
    assert!(encoded.contains("\"tool_use\""));
}

#[test]
fn invalid_digest_is_rejected() {
    let mut revision = revision();
    revision.config_digest = "secret".into();
    assert_eq!(
        revision.validate().unwrap_err().code,
        ErrorCode::InvalidAgentConfiguration
    );
}
