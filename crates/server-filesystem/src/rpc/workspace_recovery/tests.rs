use super::*;
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
fn malformed_recovery_request_is_rejected() {
    assert!(decode::<WorkspaceRecoveryRequest>(serde_json::json!({"workspaceId":null})).is_err());
}
