use serde_json::json;

use super::{
    WorkspaceLabelAffectedResult, WorkspaceLabelAssignmentSetRequest,
    WorkspaceLabelAssignmentSetResult, WorkspaceLabelListRequest, WorkspaceLabelListResult,
    WorkspaceLabelLiveUpdate, WorkspaceLabelUpdateRequest, WorkspaceLabelUpdateResult,
};

#[test]
fn parses_list_subscription_and_incremental_cursor() {
    let request: WorkspaceLabelListRequest = serde_json::from_value(json!({
        "subscribe": {"subscriptionId": "labels-1"},
        "sync": {"generation": "generation-1", "afterSeq": 4}
    }))
    .unwrap();
    assert_eq!(request.sync.unwrap().after_seq, 4);
    assert_eq!(
        request.subscribe.unwrap().subscription_id.as_deref(),
        Some("labels-1")
    );
}

#[test]
fn assignment_and_atomic_edits_use_paseo_camel_case() {
    let assignment: WorkspaceLabelAssignmentSetRequest = serde_json::from_value(json!({
        "workspaceId": "wks_one",
        "label": {"name": "Urgent", "color": "red"},
        "assigned": true
    }))
    .unwrap();
    assert_eq!(assignment.workspace_id, "wks_one");
    let edit: WorkspaceLabelUpdateRequest = serde_json::from_value(json!({
        "name": "Urgent", "newName": "Priority", "color": "sky"
    }))
    .unwrap();
    assert_eq!(edit.new_name.as_deref(), Some("Priority"));
    let color_only: WorkspaceLabelUpdateRequest = serde_json::from_value(json!({
        "name": "Urgent", "color": "amber"
    }))
    .unwrap();
    assert!(color_only.new_name.is_none());
    let name_only: WorkspaceLabelUpdateRequest = serde_json::from_value(json!({
        "name": "Urgent", "newName": "Priority"
    }))
    .unwrap();
    assert!(name_only.color.is_none());
}

#[test]
fn rejects_unknown_colors_and_negative_sequences() {
    assert!(
        serde_json::from_value::<WorkspaceLabelAssignmentSetRequest>(json!({
            "workspaceId": "wks_one",
            "label": {"name": "Urgent", "color": "chartreuse"},
            "assigned": true
        }))
        .is_err()
    );
    assert!(
        serde_json::from_value::<WorkspaceLabelListRequest>(json!({
            "sync": {"generation": "generation-1", "afterSeq": -1}
        }))
        .is_err()
    );
}

#[test]
fn live_updates_carry_subscription_generation_and_sequence() {
    let update: WorkspaceLabelLiveUpdate = serde_json::from_value(json!({
        "kind": "upsert",
        "subscriptionId": "subscription-1",
        "label": {"name": "Needs review", "color": "sky"},
        "generation": "generation-1",
        "seq": 5
    }))
    .unwrap();
    assert!(matches!(
        update,
        WorkspaceLabelLiveUpdate::Upsert { seq: 5, .. }
    ));
}

#[test]
fn sequenced_list_result_requires_its_sync_boundary() {
    let result: WorkspaceLabelListResult = serde_json::from_value(json!({
        "subscriptionId": "labels-1",
        "labels": [],
        "sync": {
            "mode": "changes",
            "generation": "generation-1",
            "headSeq": 4,
            "removals": []
        }
    }))
    .unwrap();
    assert_eq!(result.sync.head_seq, 4);
    assert!(serde_json::from_value::<WorkspaceLabelListResult>(json!({"labels": []})).is_err());
}

#[test]
fn response_shapes_require_every_promised_field() {
    assert!(
        serde_json::from_value::<WorkspaceLabelAssignmentSetResult>(json!({
            "label": {"name": "Urgent", "color": "red"}
        }))
        .is_err()
    );
    assert!(serde_json::from_value::<WorkspaceLabelAffectedResult>(json!({})).is_err());
    assert!(
        serde_json::from_value::<WorkspaceLabelUpdateResult>(json!({
            "label": {"name": "Priority", "color": "sky"}
        }))
        .is_err()
    );
}
