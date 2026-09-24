use serde_json::json;

use super::*;
#[test]
fn recovery_states_match_paseo_discriminators_and_camel_case_fields() {
    let recoverable = serde_json::to_value(WorkspaceRecoveryState::Recoverable {
        workspace_id: "wks-one".to_owned(),
        workspace_name: "Feature".to_owned(),
        action: WorkspaceRecoveryAction::Restore,
        branch: Some("feature".to_owned()),
    })
    .expect("recoverable");
    assert_eq!(
        recoverable,
        json!({
            "kind":"recoverable",
            "workspaceId":"wks-one",
            "workspaceName":"Feature",
            "action":"restore",
            "branch":"feature"
        })
    );

    let unavailable = serde_json::to_value(WorkspaceRecoveryState::Unavailable {
        workspace_id: "missing".to_owned(),
        reason: WorkspaceRecoveryUnavailableReason::WorkspaceNotFound,
        message: "gone".to_owned(),
    })
    .expect("unavailable");
    assert_eq!(
        unavailable,
        json!({
            "kind":"unavailable",
            "workspaceId":"missing",
            "reason":"workspace_not_found",
            "message":"gone"
        })
    );
}

#[test]
fn recovery_request_strips_unknown_fields() {
    let recovery: WorkspaceRecoveryRequest = serde_json::from_value(json!({
        "workspaceId":"wks-two",
        "unknown":"ignored"
    }))
    .expect("recovery");
    assert_eq!(recovery.workspace_id, "wks-two");
}
