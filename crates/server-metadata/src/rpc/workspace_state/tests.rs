use crate::service::workspace_state::WorkspaceAttentionResult;
use serde_json::json;

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
fn malformed_workspace_state_requests_are_rejected() {
    assert!(decode::<WorkspaceClearAttentionRequest>(json!({"workspaceId":42})).is_err());
}
