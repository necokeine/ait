//! Lifecycle/stream contracts adapted from Paseo's `AgentManager` regression suites.

use crate::protocol::timeline::NativeItem;
use crate::storage::timeline::Timeline;

use super::*;

async fn running() -> (AgentManager, MemoryRegistry, FakeClient) {
    let (mut manager, registry, client) = make_manager();
    manager
        .create("agent-1", &spec(), AgentRegistration::default())
        .await
        .unwrap();
    manager.send("agent-1", "start").await.unwrap();
    manager.poll().await.unwrap();
    (manager, registry, client)
}

fn progress(observation: &str, text: &str) -> AgentTurnEvent {
    AgentTurnEvent::Progress {
        observation: observation.to_owned(),
        entry: NativeItem {
            key: "native:native-turn:message".to_owned(),
            turn_id: Some("native-turn".to_owned()),
            timestamp: "2026-09-26T00:00:00Z".to_owned(),
            item: json!({"type":"assistant_message","messageId":"message","text":text}),
        },
    }
}

#[tokio::test]
async fn steering_rejection_keeps_the_current_turn_and_does_not_start_another() {
    let (mut manager, registry, client) = running().await;
    let original = registry.get("agent-1").unwrap().unwrap();
    assert_eq!(
        manager.send_steering("agent-1", "continue").await,
        Err(AgentManagerError::Busy)
    );
    assert_eq!(manager.active_turn("agent-1"), Some("native-turn"));
    assert_eq!(registry.get("agent-1").unwrap().unwrap(), original);
    let state = client.0.lock().unwrap();
    assert_eq!(state.start_calls, 1);
    assert_eq!(state.close_calls, 0);
    assert_eq!(
        state.steer_calls,
        [("native-turn".to_owned(), "continue".to_owned())]
    );
}

#[tokio::test]
async fn accepted_steer_does_not_become_rejected_when_activity_persistence_fails() {
    let (mut manager, registry, client) = running().await;
    client.0.lock().unwrap().steer_result = Ok(());
    registry.0.lock().unwrap().fail_update = true;
    manager.send_steering("agent-1", "continue").await.unwrap();
    let accepted_at = manager.live["agent-1"].pending_input_at.clone().unwrap();
    client
        .0
        .lock()
        .unwrap()
        .events
        .push_back(AgentTurnEvent::Completed(Some("answer".to_owned())));
    assert_eq!(manager.poll().await, Err(AgentManagerError::Registry));
    assert_eq!(manager.active_turn("agent-1"), Some("native-turn"));
    assert_eq!(client.0.lock().unwrap().events.len(), 1);
    registry.0.lock().unwrap().fail_update = false;
    registry
        .update("agent-1", &|record| {
            let mut updated = record.clone();
            updated.title = Some("Concurrent title".to_owned());
            updated.labels.insert("keep".to_owned(), "yes".to_owned());
            updated
        })
        .unwrap();
    manager.poll().await.unwrap();
    let latest = registry.get("agent-1").unwrap().unwrap();
    assert_eq!(
        latest.last_user_message_at.as_deref(),
        Some(accepted_at.as_str())
    );
    assert_eq!(latest.title.as_deref(), Some("Concurrent title"));
    assert_eq!(latest.labels["keep"], "yes");
    assert_eq!(manager.last_message("agent-1"), Some("answer"));
    assert!(manager.live["agent-1"].pending_input_at.is_none());
    assert_eq!(client.0.lock().unwrap().steer_calls.len(), 1);
}

#[tokio::test]
async fn ambiguous_steer_failure_closes_the_writer_and_preserves_native_identity() {
    let (mut manager, registry, client) = running().await;
    let persistence = registry.get("agent-1").unwrap().unwrap().persistence;
    client.0.lock().unwrap().steer_result = Err(AgentSessionError::Failed);
    assert_eq!(
        manager.send_steering("agent-1", "continue").await,
        Err(AgentManagerError::Session)
    );
    assert!(manager.live_snapshot("agent-1").is_none());
    let record = registry.get("agent-1").unwrap().unwrap();
    assert_eq!(record.last_status, AgentRuntimeStatus::Error);
    assert_eq!(record.persistence, persistence);
    assert_eq!(
        record.last_error.as_deref(),
        Some("Provider execution failed")
    );
    assert_eq!(client.0.lock().unwrap().start_calls, 1);
    assert_eq!(client.0.lock().unwrap().close_calls, 1);
}

#[tokio::test]
async fn unavailable_steer_fails_closed_without_replacing_the_native_turn() {
    let (mut manager, registry, client) = running().await;
    client.0.lock().unwrap().steer_result = Err(AgentSessionError::Unavailable);
    assert_eq!(
        manager.send_steering("agent-1", "continue").await,
        Err(AgentManagerError::Session)
    );
    assert_eq!(
        registry.get("agent-1").unwrap().unwrap().last_status,
        AgentRuntimeStatus::Error
    );
    assert_eq!(client.0.lock().unwrap().start_calls, 1);
    assert_eq!(client.0.lock().unwrap().close_calls, 1);
}

#[tokio::test]
async fn idle_steering_uses_normal_turn_admission() {
    let (mut manager, _, client) = make_manager();
    manager
        .create("agent-1", &spec(), AgentRegistration::default())
        .await
        .unwrap();
    manager.send_steering("agent-1", "first").await.unwrap();
    assert_eq!(manager.active_turn("agent-1"), Some("native-turn"));
    assert_eq!(client.0.lock().unwrap().start_calls, 1);
    assert!(client.0.lock().unwrap().steer_calls.is_empty());
}

#[tokio::test]
async fn archived_agent_does_not_admit_steering_to_its_still_live_writer() {
    let (mut manager, registry, client) = running().await;
    registry
        .update("agent-1", &|record| {
            let mut archived = record.clone();
            archived.archived_at = Some("2026-09-26T00:00:00Z".to_owned());
            archived
        })
        .unwrap();
    assert_eq!(
        manager.send_steering("agent-1", "continue").await,
        Err(AgentManagerError::Busy)
    );
    assert!(client.0.lock().unwrap().steer_calls.is_empty());
    manager.reconcile().await.unwrap();
    assert!(manager.live_snapshot("agent-1").is_none());
}

#[tokio::test]
async fn deleted_agent_cannot_be_recreated_by_a_late_steer() {
    let (mut manager, registry, client) = running().await;
    registry.remove("agent-1").unwrap();
    assert_eq!(
        manager.send_steering("agent-1", "continue").await,
        Err(AgentManagerError::NotFound("agent-1".to_owned()))
    );
    assert!(client.0.lock().unwrap().steer_calls.is_empty());
    manager.reconcile().await.unwrap();
    assert!(registry.get("agent-1").unwrap().is_none());
    assert!(manager.live_snapshot("agent-1").is_none());
}

#[tokio::test]
async fn invalid_steer_text_is_rejected_before_native_admission() {
    let (mut manager, registry, client) = running().await;
    let before = registry.get("agent-1").unwrap();
    for text in [" \n\t".to_owned(), "界".repeat(21_846)] {
        assert_eq!(
            manager.send_steering("agent-1", &text).await,
            Err(AgentManagerError::InvalidRequest)
        );
    }
    assert!(client.0.lock().unwrap().steer_calls.is_empty());
    assert_eq!(registry.get("agent-1").unwrap(), before);
}

#[tokio::test]
async fn stream_drain_budget_defers_completion_until_every_queued_item_is_committed() {
    let (mut manager, registry, client) = running().await;
    let timeline = Timeline::memory().unwrap();
    manager = manager.with_timeline(timeline.clone());
    {
        let mut state = client.0.lock().unwrap();
        for index in 0..129 {
            state.events.push_back(progress(&index.to_string(), "x"));
        }
        state
            .events
            .push_back(AgentTurnEvent::Completed(Some("done".to_owned())));
    }
    manager.poll().await.unwrap();
    assert_eq!(timeline.read("agent-1").unwrap().1.len(), 128);
    assert_eq!(manager.active_turn("agent-1"), Some("native-turn"));
    assert_eq!(
        registry.get("agent-1").unwrap().unwrap().last_status,
        AgentRuntimeStatus::Running
    );
    assert_eq!(manager.last_message("agent-1"), None);
    manager.poll().await.unwrap();
    assert_eq!(timeline.read("agent-1").unwrap().1.len(), 129);
    assert_eq!(manager.active_turn("agent-1"), None);
    assert_eq!(manager.last_message("agent-1"), Some("done"));
}

#[tokio::test]
async fn conflicting_progress_observation_fails_the_turn_without_corrupting_history() {
    let (mut manager, registry, client) = running().await;
    let timeline = Timeline::memory().unwrap();
    manager = manager.with_timeline(timeline.clone());
    {
        let mut state = client.0.lock().unwrap();
        state.events.push_back(progress("same", "accepted"));
        state.events.push_back(progress("same", "changed"));
        state
            .events
            .push_back(AgentTurnEvent::Completed(Some("must not win".to_owned())));
    }
    manager.poll().await.unwrap();
    let rows = timeline.read("agent-1").unwrap().1;
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].entry.item["text"], "accepted");
    assert_eq!(
        registry.get("agent-1").unwrap().unwrap().last_status,
        AgentRuntimeStatus::Error
    );
    assert!(manager.live_snapshot("agent-1").is_none());
    assert_eq!(client.0.lock().unwrap().close_calls, 1);
}

#[tokio::test]
async fn oversized_progress_closes_the_turn_before_any_partial_publication() {
    let (mut manager, registry, client) = running().await;
    let timeline = Timeline::memory().unwrap();
    manager = manager.with_timeline(timeline.clone());
    client
        .0
        .lock()
        .unwrap()
        .events
        .push_back(progress("large", &"x".repeat(256 * 1024)));
    manager.poll().await.unwrap();
    assert!(timeline.read("agent-1").unwrap().1.is_empty());
    assert_eq!(
        registry.get("agent-1").unwrap().unwrap().last_status,
        AgentRuntimeStatus::Error
    );
    assert_eq!(client.0.lock().unwrap().close_calls, 1);
}

#[tokio::test]
async fn provider_poll_failure_is_terminal_and_keeps_safe_error_metadata() {
    let (mut manager, registry, client) = running().await;
    client.0.lock().unwrap().poll_error = Some(AgentSessionError::Failed);
    manager.poll().await.unwrap();
    assert!(manager.live_snapshot("agent-1").is_none());
    let record = registry.get("agent-1").unwrap().unwrap();
    assert_eq!(record.last_status, AgentRuntimeStatus::Error);
    assert!(record.requires_attention);
    assert_eq!(
        record.last_error.as_deref(),
        Some("Provider execution failed")
    );
}

#[tokio::test]
async fn failed_terminal_cleanup_retains_pending_failure_and_blocks_new_writer() {
    let (mut manager, registry, client) = running().await;
    {
        let mut state = client.0.lock().unwrap();
        state.events.push_back(AgentTurnEvent::Failed);
        state.fail_close = true;
    }
    assert_eq!(manager.poll().await, Err(AgentManagerError::Session));
    assert_eq!(manager.active_turn("agent-1"), Some("native-turn"));
    assert_eq!(
        manager.send("agent-1", "new").await,
        Err(AgentManagerError::Busy)
    );
    assert_eq!(
        registry.get("agent-1").unwrap().unwrap().last_status,
        AgentRuntimeStatus::Running
    );
    client.0.lock().unwrap().fail_close = false;
    manager.poll().await.unwrap();
    assert!(manager.live_snapshot("agent-1").is_none());
    assert_eq!(
        registry.get("agent-1").unwrap().unwrap().last_status,
        AgentRuntimeStatus::Error
    );
}

#[tokio::test]
async fn explicit_close_during_turn_preserves_persistence_for_later_resume() {
    let (mut manager, registry, client) = running().await;
    let identity = registry.get("agent-1").unwrap().unwrap().persistence;
    manager.close("agent-1").await.unwrap();
    let closed = registry.get("agent-1").unwrap().unwrap();
    assert_eq!(closed.persistence, identity);
    assert_eq!(closed.last_status, AgentRuntimeStatus::Closed);
    assert!(manager.live_snapshot("agent-1").is_none());
    manager.resume("agent-1").await.unwrap();
    assert_eq!(
        client.0.lock().unwrap().resume_purposes,
        [AgentResumePurpose::Interactive]
    );
}

#[tokio::test]
async fn idle_poll_and_idempotent_cancel_do_not_evict_the_session() {
    let (mut manager, _, client) = make_manager();
    manager
        .create("agent-1", &spec(), AgentRegistration::default())
        .await
        .unwrap();
    manager.poll().await.unwrap();
    manager.cancel("agent-1").await.unwrap();
    manager.cancel("unknown").await.unwrap();
    manager.poll().await.unwrap();
    assert!(manager.live_snapshot("agent-1").is_some());
    assert_eq!(client.0.lock().unwrap().close_calls, 0);
    assert!(client.0.lock().unwrap().events.is_empty());
}
