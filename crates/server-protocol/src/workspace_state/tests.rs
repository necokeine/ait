use serde_json::json;

use super::*;

#[test]
fn capabilities_use_only_canonical_workspace_state_names() {
    assert_eq!(
        CAPABILITIES,
        [
            "workspace.clear_attention.request",
            "workspace.mark_unread.request",
            "workspace.recovery.inspect.request",
            "workspace.recovery.restore.request",
        ]
    );
    assert!(!CAPABILITIES.contains(&"workspace_recovery_inspect_request"));
}

#[test]
fn clear_attention_accepts_singular_and_batch_workspace_ids() {
    let singular: WorkspaceClearAttentionRequest =
        serde_json::from_value(json!({"workspaceId":"wks-one"})).expect("singular");
    assert_eq!(
        singular.workspace_id,
        WorkspaceIdSelection::One("wks-one".to_owned())
    );
    let batch: WorkspaceClearAttentionRequest =
        serde_json::from_value(json!({"workspaceId":["wks-one","wks-two"]})).expect("batch");
    assert_eq!(
        batch.workspace_id,
        WorkspaceIdSelection::Many(vec!["wks-one".to_owned(), "wks-two".to_owned()])
    );
}

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
fn mark_unread_and_recovery_requests_strip_unknown_fields() {
    let unread: WorkspaceMarkUnreadRequest = serde_json::from_value(json!({
        "workspaceId":"wks-one",
        "requestId":"legacy-inner-id",
        "future":true
    }))
    .expect("mark unread");
    let recovery: WorkspaceRecoveryRequest = serde_json::from_value(json!({
        "workspaceId":"wks-two",
        "unknown":"ignored"
    }))
    .expect("recovery");
    assert_eq!(unread.workspace_id, "wks-one");
    assert_eq!(recovery.workspace_id, "wks-two");
}
