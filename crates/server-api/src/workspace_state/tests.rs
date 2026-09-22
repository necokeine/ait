use serde_json::json;
use server_application::workspace_state::{
    WorkspaceAttentionResult, WorkspaceRecoveryState as ApplicationRecoveryState,
    WorkspaceRecoveryUnavailableReason as ApplicationUnavailableReason,
};

use super::*;

#[test]
fn clear_attention_projection_keeps_partial_batch_results() {
    let result = clear_attention_result(
        WorkspaceIdSelection::Many(vec!["one".to_owned(), "two".to_owned()]),
        WorkspaceAttentionBatch {
            cleared_agent_ids: vec!["agent-one".to_owned()],
            results: vec![
                WorkspaceAttentionResult {
                    workspace_id: "one".to_owned(),
                    cleared_agent_ids: vec!["agent-one".to_owned()],
                    success: true,
                    error: None,
                },
                WorkspaceAttentionResult {
                    workspace_id: "two".to_owned(),
                    cleared_agent_ids: Vec::new(),
                    success: false,
                    error: Some("Workspace not found: two".to_owned()),
                },
            ],
            success: false,
            error: Some("Workspace not found: two".to_owned()),
        },
    );

    assert_eq!(
        serde_json::to_value(result).expect("serialize"),
        json!({
            "workspaceId":["one","two"],
            "clearedAgentIds":["agent-one"],
            "results":[
                {"workspaceId":"one","clearedAgentIds":["agent-one"],"success":true,"error":null},
                {"workspaceId":"two","clearedAgentIds":[],"success":false,"error":"Workspace not found: two"}
            ],
            "success":false,
            "error":"Workspace not found: two"
        })
    );
}

#[test]
fn recovery_projection_preserves_every_stable_unavailable_reason() {
    let cases = [
        (
            ApplicationUnavailableReason::WorkspaceNotFound,
            "workspace_not_found",
        ),
        (
            ApplicationUnavailableReason::WorkspaceNotArchived,
            "workspace_not_archived",
        ),
        (
            ApplicationUnavailableReason::ProjectNotFound,
            "project_not_found",
        ),
        (
            ApplicationUnavailableReason::ProjectDirectoryMissing,
            "project_directory_missing",
        ),
        (
            ApplicationUnavailableReason::WorkspaceDirectoryMissing,
            "workspace_directory_missing",
        ),
        (
            ApplicationUnavailableReason::WorktreeBranchMissing,
            "worktree_branch_missing",
        ),
    ];
    for (reason, expected) in cases {
        let state = recovery_state(ApplicationRecoveryState::Unavailable {
            workspace_id: "wks-one".to_owned(),
            reason,
            message: "message".to_owned(),
        });
        assert_eq!(
            serde_json::to_value(state)
                .expect("serialize")
                .get("reason")
                .and_then(serde_json::Value::as_str),
            Some(expected)
        );
    }
}

#[test]
fn malformed_workspace_state_requests_are_rejected() {
    assert!(decode::<WorkspaceClearAttentionRequest>(json!({"workspaceId":42})).is_err());
    assert!(decode::<WorkspaceRecoveryRequest>(json!({"workspaceId":null})).is_err());
}
