use super::*;
use serde_json::json;

type Captured = Arc<Mutex<Vec<(SessionEventKind, Value)>>>;

fn capture(connection: &SessionConnection, notifications: bool) -> (SessionSubscription, Captured) {
    let events = Arc::new(Mutex::new(Vec::new()));
    let captured = events.clone();
    let subscription = connection
        .subscribe(
            EventsRequest {
                events: vec![
                    "agent_attention_required".to_owned(),
                    "status.server_info".to_owned(),
                ],
                notifications,
            },
            Arc::new(move |kind, payload| {
                captured.lock().unwrap().push((kind, payload));
                Ok(())
            }),
        )
        .unwrap();
    (subscription, events)
}

fn heartbeat(connection: &SessionConnection, focused: Option<&str>, visible: bool, timestamp: i64) {
    connection
        .heartbeat(
            serde_json::from_value(json!({
                "deviceType":"web", "focusedAgentId":focused,"appVisible":visible,
                "lastActivityAt": DateTime::from_timestamp_millis(timestamp).unwrap().to_rfc3339(),
            }))
            .unwrap(),
        )
        .unwrap();
}

#[test]
fn paused_observers_deliver_after_activation_filter_topics_and_release() {
    let service = SessionEvents::default();
    let connection = service.connect();
    assert!(!connection.id().is_empty());
    let (subscription, events) = capture(&connection, false);
    let id = subscription.id().to_owned();
    service.publish(SessionEventKind::DaemonConfig, &json!({"config":{}}));
    service.publish(SessionEventKind::ServerInfo, &json!({"order":1}));
    service.publish(SessionEventKind::ServerInfo, &json!({"order":2}));
    assert!(events.lock().unwrap().is_empty());
    service.publish(SessionEventKind::ServerInfo, &Value::Null);
    subscription.activate().unwrap();
    subscription.activate().unwrap();
    assert_eq!(
        events.lock().unwrap()[0].1,
        json!({"order":1,"subscriptionId":id})
    );
    assert_eq!(events.lock().unwrap()[1].1["order"], 2);
    service.publish(SessionEventKind::ServerInfo, &json!({"order":3}));
    assert_eq!(events.lock().unwrap().len(), 3);
    drop(subscription);
    service.publish(SessionEventKind::ServerInfo, &json!({"order":4}));
    assert_eq!(events.lock().unwrap().len(), 3);
    assert_eq!(
        SessionEventKind::AgentAttention.method(),
        "agent_attention_required"
    );
    assert_eq!(
        SessionEventKind::DaemonConfig.method(),
        "status.daemon_config_changed"
    );
    assert_eq!(SessionEventKind::ServerInfo.method(), "status.server_info");
}

#[test]
fn notifications_select_one_present_client_and_one_subscription_without_hiding_state() {
    let service = SessionEvents::default();
    let first = service.connect();
    let second = service.connect();
    let now = Utc::now().timestamp_millis();
    heartbeat(&first, None, true, now - 100);
    heartbeat(&second, None, false, now);
    let (one, first_events) = capture(&first, true);
    let (two, second_events) = capture(&second, true);
    let (duplicate, duplicate_events) = capture(&second, true);
    for subscription in [&one, &two, &duplicate] {
        subscription.activate().unwrap();
    }
    service.publish_at(
        SessionEventKind::AgentAttention,
        &json!({"agentId":"a"}),
        now,
    );
    assert_eq!(first_events.lock().unwrap()[0].1["shouldNotify"], false);
    assert_eq!(second_events.lock().unwrap()[0].1["shouldNotify"], true);
    assert_eq!(duplicate_events.lock().unwrap()[0].1["shouldNotify"], false);
    heartbeat(&first, Some("a"), true, now);
    service.publish_at(
        SessionEventKind::AgentAttention,
        &json!({"agentId":"a"}),
        now,
    );
    assert_eq!(second_events.lock().unwrap()[1].1["shouldNotify"], false);
    drop(first);
    service.publish_at(
        SessionEventKind::AgentAttention,
        &json!({"agentId":"a"}),
        now,
    );
    assert_eq!(second_events.lock().unwrap()[2].1["shouldNotify"], true);
    assert_eq!(first_events.lock().unwrap().len(), 2);
    service.publish_at(
        SessionEventKind::AgentAttention,
        &json!({"agentId":"a"}),
        now + PRESENCE_MS + 1,
    );
    assert_eq!(second_events.lock().unwrap()[3].1["shouldNotify"], false);
}

#[test]
fn absent_heartbeat_and_disabled_notifications_never_notify_and_future_activity_is_clamped() {
    let service = SessionEvents::default();
    let connection = service.connect();
    let (subscription, events) = capture(&connection, true);
    subscription.activate().unwrap();
    service.publish(SessionEventKind::AgentAttention, &json!({"agentId":"a"}));
    assert_eq!(events.lock().unwrap()[0].1["shouldNotify"], false);
    let now = Utc::now().timestamp_millis();
    heartbeat(&connection, None, true, now + 86_400_000);
    assert!(
        connection.presence.lock().unwrap().activity_ms.unwrap() <= Utc::now().timestamp_millis()
    );
    service.publish_at(
        SessionEventKind::AgentAttention,
        &json!({"agentId":"a"}),
        now + PRESENCE_MS + 100,
    );
    assert_eq!(events.lock().unwrap()[1].1["shouldNotify"], false);
    drop(subscription);
    let (subscription, events) = capture(&connection, false);
    subscription.activate().unwrap();
    service.publish(SessionEventKind::AgentAttention, &json!({"agentId":"a"}));
    assert_eq!(events.lock().unwrap()[0].1["shouldNotify"], false);
}

#[test]
fn invalid_heartbeats_and_unsupported_topics_do_not_mutate_presence_or_register() {
    let service = SessionEvents::default();
    let connection = service.connect();
    for patch in [
        json!({"lastActivityAt":"invalid"}),
        json!({"appVisibilityChangedAt":"invalid"}),
        json!({"focusedAgentId":"x".repeat(129)}),
        json!({"focusedTerminalId":"bad\n"}),
    ] {
        let mut value = json!({"deviceType":"mobile","focusedAgentId":null,"appVisible":true,"lastActivityAt":"2026-09-24T00:00:00Z"});
        value
            .as_object_mut()
            .unwrap()
            .extend(patch.as_object().unwrap().clone());
        let request = serde_json::from_value(value).unwrap();
        assert_eq!(connection.heartbeat(request), Err(SessionError::Invalid));
        assert!(connection.presence.lock().unwrap().activity_ms.is_none());
    }
    let sink: EventSink = Arc::new(|_, _| Ok(()));
    assert!(matches!(
        connection.subscribe(
            EventsRequest {
                events: vec!["activity_log".to_owned()],
                notifications: false
            },
            sink.clone()
        ),
        Err(SessionError::Unsupported)
    ));
    assert!(matches!(
        connection.subscribe(
            EventsRequest {
                events: vec!["status.server_info".to_owned(); 33],
                notifications: false
            },
            sink.clone()
        ),
        Err(SessionError::Invalid)
    ));
    let empty = connection
        .subscribe(
            EventsRequest {
                events: Vec::new(),
                notifications: false,
            },
            sink,
        )
        .unwrap();
    empty.activate().unwrap();
    assert_eq!(service.0.lock().unwrap().listeners.len(), 1);
}

#[test]
fn pending_count_and_byte_budgets_close_observers_and_sink_failure_does_not_fail_producer() {
    for oversized in [false, true] {
        let service = SessionEvents::default();
        let connection = service.connect();
        let (subscription, events) = capture(&connection, false);
        if oversized {
            service.publish(
                SessionEventKind::ServerInfo,
                &json!({"data":"x".repeat(MAX_PENDING_BYTES)}),
            );
        } else {
            for _ in 0..=MAX_PENDING_EVENTS {
                service.publish(SessionEventKind::ServerInfo, &json!({}));
            }
        }
        assert_eq!(subscription.activate(), Err(SessionError::Closed));
        assert!(events.lock().unwrap().is_empty());
        service.publish(SessionEventKind::ServerInfo, &json!({}));
    }
    let service = SessionEvents::default();
    let connection = service.connect();
    for paused in [true, false] {
        let subscription = connection
            .subscribe(
                EventsRequest {
                    events: vec!["status.server_info".to_owned()],
                    notifications: false,
                },
                Arc::new(|_, _| Err(SessionError::Closed)),
            )
            .unwrap();
        if !paused {
            subscription.activate().unwrap();
        }
        service.publish(SessionEventKind::ServerInfo, &json!({}));
        assert_eq!(subscription.activate(), Err(SessionError::Closed));
    }
}
