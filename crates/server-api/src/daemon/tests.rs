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
    assert!(ErrorCode::DaemonIo.retryable());
    assert!(!ErrorCode::DaemonConfigInvalid.retryable());
}

#[test]
fn restart_reason_defaults_and_empty_requests_follow_canonical_params() {
    let request: RestartRequest = decode(json!({})).unwrap();
    assert_eq!(request.reason, None);
    let request: RestartRequest = decode(json!({"reason":"settings_changed"})).unwrap();
    assert_eq!(request.reason.as_deref(), Some("settings_changed"));
    let _: EmptyRequest = decode(json!({"future":true})).unwrap();
}
