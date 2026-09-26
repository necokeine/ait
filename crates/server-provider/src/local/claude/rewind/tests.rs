use super::*;

#[test]
fn fork_remaps_transcript_chain_and_preserves_provider_messages_without_editing_source() {
    let user = Uuid::new_v4().to_string();
    let progress = Uuid::new_v4().to_string();
    let assistant = Uuid::new_v4().to_string();
    let records = vec![
        json!({"type":"user","uuid":user,"parentUuid":null,"sessionId":"source","timestamp":"2026-09-26T00:00:00Z","message":{"content":"first"}}),
        json!({"type":"progress","uuid":progress,"parentUuid":user}),
        json!({"type":"assistant","uuid":assistant,"parentUuid":progress,"logicalParentUuid":user,"agentName":"private-child","message":{"id":"api-id","content":[{"type":"text","text":"reply"}]}}),
        json!({"type":"assistant","uuid":"child","isSidechain":true}),
    ];
    let original = records.clone();
    let output = fork_records(&records, "source", "fork").unwrap();
    assert_eq!(records, original);
    assert_eq!(output.len(), 2);
    assert_ne!(output[0]["uuid"], user);
    assert_eq!(output[1]["parentUuid"], output[0]["uuid"]);
    assert_eq!(output[1]["logicalParentUuid"], output[0]["uuid"]);
    assert_eq!(output[1]["message"]["id"], "api-id");
    assert_eq!(output[0]["forkedFrom"]["messageUuid"], user);
    assert!(output[1].get("agentName").is_none());
    assert!(
        fork_records(
            &[original[0].clone(), original[0].clone()],
            "source",
            "fork"
        )
        .is_err()
    );
}

#[test]
fn explicit_empty_branch_can_be_recovered_without_claiming_another_missing_session() {
    let root = tempfile::tempdir().unwrap();
    let spec = AgentSessionSpec {
        provider: "claude".into(),
        cwd: root.path().to_str().unwrap().into(),
        config: StoredAgentConfig::default(),
    };
    let id = Uuid::new_v4().to_string();
    let branch = fresh(&id, &spec);
    let mut handle = AgentPersistenceHandle {
        provider: "claude".into(),
        session_id: id,
        native_handle: None,
        metadata: Some(branch.resume_metadata),
    };
    assert!(
        fresh_history(&handle, &spec.cwd)
            .unwrap()
            .unwrap()
            .entries
            .is_empty()
    );
    assert!(fresh_history(&handle, "/different/missing").is_err());
    handle.metadata = None;
    assert!(fresh_history(&handle, &spec.cwd).unwrap().is_none());
}
