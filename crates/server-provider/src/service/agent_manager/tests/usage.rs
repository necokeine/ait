use super::*;
use crate::protocol::usage::AgentUsage;

#[tokio::test]
async fn provider_identity_changes_commit_before_completion_and_retry_failed_registry_writes() {
    let (mut manager, registry, client) = make_manager();
    manager
        .create("agent-1", &spec(), AgentRegistration::default())
        .await
        .unwrap();
    manager.send("agent-1", "hello").await.unwrap();
    manager.poll().await.unwrap();
    client.0.lock().unwrap().handle_session_id = "native-rotated".into();
    client
        .0
        .lock()
        .unwrap()
        .events
        .push_back(AgentTurnEvent::Completed(Some("done".into())));
    registry.0.lock().unwrap().fail_update = true;
    assert_eq!(manager.poll().await, Err(AgentManagerError::Registry));
    assert!(manager.active_turn("agent-1").is_some());
    registry.0.lock().unwrap().fail_update = false;
    manager.poll().await.unwrap();
    let record = registry.get("agent-1").unwrap().unwrap();
    assert_eq!(record.persistence.unwrap().session_id, "native-rotated");
    assert_eq!(
        record.runtime_info.unwrap().session_id.as_deref(),
        Some("native-rotated")
    );
    assert!(manager.active_turn("agent-1").is_none());
    manager.close_all().await.unwrap();
}

#[tokio::test]
async fn native_permission_withdrawal_clears_durable_attention_without_ending_the_turn() {
    let (mut manager, registry, client) = make_manager();
    manager
        .create("agent-1", &spec(), AgentRegistration::default())
        .await
        .unwrap();
    manager.send("agent-1", "permission").await.unwrap();
    client
        .0
        .lock()
        .unwrap()
        .events
        .push_back(AgentTurnEvent::PermissionRequested(
            json!({"id":"permission"}),
        ));
    manager.poll().await.unwrap();
    assert!(registry.get("agent-1").unwrap().unwrap().requires_attention);
    client
        .0
        .lock()
        .unwrap()
        .events
        .push_back(AgentTurnEvent::PermissionResolved("permission".into()));
    manager.poll().await.unwrap();
    assert!(!registry.get("agent-1").unwrap().unwrap().requires_attention);
    assert!(manager.active_turn("agent-1").is_some());
    manager.close_all().await.unwrap();
}

#[tokio::test]
async fn usage_is_durable_before_completion_and_survives_resume_and_runtime_refresh() {
    let (mut manager, registry, client) = make_manager();
    manager
        .create("agent-1", &spec(), AgentRegistration::default())
        .await
        .unwrap();
    manager.send("agent-1", "measure").await.unwrap();
    let usage = AgentUsage {
        input_tokens: Some(100),
        context_window_used_tokens: Some(112),
        total_cost_usd: Some(0.03),
        ..Default::default()
    };
    client.0.lock().unwrap().events.extend([
        AgentTurnEvent::Usage(usage.clone()),
        AgentTurnEvent::Completed(Some("done".into())),
    ]);
    registry.0.lock().unwrap().fail_update = true;
    assert_eq!(manager.poll().await, Err(AgentManagerError::Registry));
    assert!(manager.active_turn("agent-1").is_some());
    registry.0.lock().unwrap().fail_update = false;
    manager.poll().await.unwrap();
    assert_eq!(manager.active_turn("agent-1"), None);
    let record = registry.get("agent-1").unwrap().unwrap();
    assert_eq!(
        crate::rpc::agent_runtime::snapshot(&record).last_usage,
        Some(usage.clone())
    );
    manager.close("agent-1").await.unwrap();
    manager.resume("agent-1").await.unwrap();
    manager.send("agent-1", "next").await.unwrap();
    manager.poll().await.unwrap();
    let record = registry.get("agent-1").unwrap().unwrap();
    assert_eq!(
        crate::rpc::agent_runtime::snapshot(&record).last_usage,
        Some(usage)
    );
    manager.close_all().await.unwrap();
}

#[tokio::test]
async fn invalid_native_usage_fails_without_persisting_or_advertising_nonfinite_facts() {
    let (mut manager, registry, client) = make_manager();
    manager
        .create("agent-1", &spec(), AgentRegistration::default())
        .await
        .unwrap();
    manager.send("agent-1", "measure").await.unwrap();
    client
        .0
        .lock()
        .unwrap()
        .events
        .push_back(AgentTurnEvent::Usage(AgentUsage {
            total_cost_usd: Some(f64::INFINITY),
            ..Default::default()
        }));
    manager.poll().await.unwrap();
    let record = registry.get("agent-1").unwrap().unwrap();
    assert_eq!(record.last_status, AgentRuntimeStatus::Error);
    assert!(
        crate::rpc::agent_runtime::snapshot(&record)
            .last_usage
            .is_none()
    );
}
