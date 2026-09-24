use serde_json::json;

use super::*;

#[test]
fn config_validation_maps_invalid_persisted_documents_to_stable_errors() {
    assert_eq!(
        config(json!({"relay":{"enabled":false}})),
        Err(ErrorCode::DaemonConfigInvalid)
    );
    assert_eq!(
        error(DaemonError::InvalidConfig),
        ErrorCode::DaemonConfigInvalid
    );
    assert_eq!(error(DaemonError::ConfigIo), ErrorCode::DaemonIo);
}

#[test]
fn restart_reason_defaults_and_empty_requests_follow_canonical_params() {
    let request: RestartRequest = decode(json!({})).unwrap();
    assert_eq!(request.reason, None);
    let request: RestartRequest = decode(json!({"reason":"settings_changed"})).unwrap();
    assert_eq!(request.reason.as_deref(), Some("settings_changed"));
    let _: EmptyRequest = decode(json!({"future":true})).unwrap();
}

#[test]
fn lifecycle_requests_return_host_intents_with_compatible_acknowledgements() {
    let restart = lifecycle("server.restart.request", &json!({"reason":"  "}))
        .unwrap()
        .unwrap();
    assert_eq!(
        restart.intent,
        LifecycleIntent::Restart {
            reason: "websocket_request".to_owned()
        }
    );
    assert_eq!(
        restart.value,
        json!({"status":"restart_requested","reason":"websocket_request"})
    );
    let shutdown = lifecycle("server.shutdown.request", &json!({}))
        .unwrap()
        .unwrap();
    assert_eq!(shutdown.intent, LifecycleIntent::Shutdown);
    assert_eq!(shutdown.value["status"], "shutdown_requested");
    assert!(
        lifecycle("daemon.config.get.request", &json!({}))
            .unwrap()
            .is_none()
    );
    assert!(matches!(
        lifecycle("server.restart.request", &json!({"reason":5})),
        Err(ErrorCode::InvalidMessage)
    ));
}
