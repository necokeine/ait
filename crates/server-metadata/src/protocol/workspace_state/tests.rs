use serde_json::json;

use super::*;

#[test]
fn capabilities_use_only_canonical_workspace_state_names() {
    assert_eq!(
        CAPABILITIES,
        [
            "workspace.clear_attention.request",
            "workspace.mark_unread.request",
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
fn mark_unread_and_recovery_requests_strip_unknown_fields() {
    let unread: WorkspaceMarkUnreadRequest = serde_json::from_value(json!({
        "workspaceId":"wks-one",
        "requestId":"legacy-inner-id",
        "future":true
    }))
    .expect("mark unread");
    assert_eq!(unread.workspace_id, "wks-one");
}
