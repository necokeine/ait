use serde_json::json;

use super::{
    CAPABILITIES, WorktreeArchiveRequest, WorktreeArchiveScope, WorktreeCreateAction,
    WorktreeCreateRequest, WorktreeListRequest,
};

#[test]
fn methods_use_only_canonical_names() {
    assert_eq!(
        CAPABILITIES,
        [
            "workspace.worktree.list.request",
            "workspace.worktree.create.request",
            "workspace.worktree.archive.request"
        ]
    );
}

#[test]
fn list_accepts_either_location_and_ignores_unknown_fields() {
    let request: WorktreeListRequest = serde_json::from_value(json!({
        "cwd": "/repo/app",
        "repoRoot": "/repo",
        "legacy": true
    }))
    .expect("parse list request");
    assert_eq!(request.cwd.as_deref(), Some("/repo/app"));
    assert_eq!(request.repo_root.as_deref(), Some("/repo"));
}

#[test]
fn archive_omission_defaults_to_workspace_and_ignores_delete_flag() {
    let request: WorktreeArchiveRequest = serde_json::from_value(json!({
        "worktreePath": "/repo/app",
        "deleteWorktreeFromDisk": true,
        "extraField": "ignored"
    }))
    .expect("parse archive request");
    assert_eq!(request.scope, WorktreeArchiveScope::Workspace);
    assert!(request.delete_worktree_from_disk);
}

#[test]
fn archive_accepts_worktree_scope_and_strips_unknown_fields() {
    let request: WorktreeArchiveRequest = serde_json::from_value(json!({
        "repoRoot": "/repo",
        "worktreeSlug": "ignored-internal-command-field",
        "scope": "worktree"
    }))
    .expect("parse archive request");
    assert_eq!(request.scope, WorktreeArchiveScope::Worktree);
}

#[test]
fn create_normalizes_legacy_prompt_context() {
    let request: WorktreeCreateRequest = serde_json::from_value(json!({
        "cwd": "/repo",
        "nameContext": "Fix it",
        "attachments": [{"type":"text","mimeType":"text/plain","text":"context"}],
        "action": "branch-off"
    }))
    .expect("parse create request");
    assert_eq!(request.action, Some(WorktreeCreateAction::BranchOff));
    let context = request
        .normalized_first_agent_context()
        .expect("legacy context");
    assert_eq!(context.prompt.as_deref(), Some("Fix it"));
    assert_eq!(context.attachments.len(), 1);
}

#[test]
fn explicit_context_wins_over_legacy_fields() {
    let request: WorktreeCreateRequest = serde_json::from_value(json!({
        "cwd": "/repo",
        "nameContext": "legacy",
        "attachments": [{"type":"text","mimeType":"text/plain","text":"legacy"}],
        "firstAgentContext": {"prompt":"current","attachments":[]},
        "action": "checkout",
        "refName": "topic"
    }))
    .expect("parse create request");
    assert_eq!(request.action, Some(WorktreeCreateAction::Checkout));
    let context = request
        .normalized_first_agent_context()
        .expect("current context");
    assert_eq!(context.prompt.as_deref(), Some("current"));
    assert!(context.attachments.is_empty());
}

#[test]
fn change_request_numbers_must_be_positive() {
    for request in [
        json!({"cwd":"/repo","githubPrNumber":0}),
        json!({
            "cwd":"/repo",
            "checkoutSource":{"kind":"change_request","number":0}
        }),
    ] {
        assert!(serde_json::from_value::<WorktreeCreateRequest>(request).is_err());
    }
}

#[test]
fn malformed_attachment_collections_normalize_to_empty_arrays() {
    let request: WorktreeCreateRequest = serde_json::from_value(json!({
        "cwd":"/repo",
        "attachments":{"not":"an array"}
    }))
    .expect("Paseo attachment normalization");
    assert_eq!(request.attachments, Some(Vec::new()));

    let request: WorktreeCreateRequest = serde_json::from_value(json!({
        "cwd":"/repo",
        "firstAgentContext":{"attachments":"invalid"}
    }))
    .expect("nested attachment normalization");
    assert_eq!(
        request.first_agent_context.expect("context").attachments,
        Vec::<serde_json::Value>::new()
    );
}

#[test]
fn invalid_array_items_are_filtered_like_paseo() {
    let request: WorktreeCreateRequest = serde_json::from_value(json!({
        "cwd":"/repo",
        "attachments":[
            {"type":"text","mimeType":"text/plain","text":"kept"},
            {"type":"text","mimeType":"text/html","text":"dropped"},
            {"type":"unknown"}
        ]
    }))
    .expect("attachment normalization");
    assert_eq!(request.attachments.expect("present").len(), 1);
}
