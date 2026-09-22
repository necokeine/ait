use serde_json::json;

use super::{
    ProjectCreateDirectoryRequest, ProjectRenameRequest, WorkspaceCreateRequest,
    WorkspaceCreateSource, WorkspaceListRequest, WorkspacePinSetRequest, WorkspaceTitleSetRequest,
};

#[test]
fn parses_paseo_camel_case_mutation_payloads() {
    assert_eq!(
        serde_json::from_value::<ProjectRenameRequest>(json!({
            "projectId": "prj_1",
            "customName": null
        }))
        .unwrap(),
        ProjectRenameRequest {
            project_id: "prj_1".to_owned(),
            custom_name: None,
        }
    );
    assert_eq!(
        serde_json::from_value::<WorkspaceTitleSetRequest>(json!({
            "workspaceId": "wks_1",
            "title": "  next  "
        }))
        .unwrap()
        .title,
        Some("  next  ".to_owned())
    );
    assert!(
        serde_json::from_value::<WorkspacePinSetRequest>(json!({
            "workspaceId": "wks_1",
            "pinned": "yes"
        }))
        .is_err()
    );
}

#[test]
fn parses_paseo_project_and_workspace_creation_payloads() {
    let request: ProjectCreateDirectoryRequest = serde_json::from_value(json!({
        "parentPath": "/Users/example/dev",
        "name": "new-project"
    }))
    .unwrap();
    assert_eq!(request.name, "new-project");

    let request: WorkspaceCreateRequest = serde_json::from_value(json!({
        "workspaceId": "wks_0123456789abcdef",
        "idempotencyKey": "create-1",
        "title": "Review",
        "source": {"kind":"directory", "path":"/repo", "projectId":"prj_1"}
    }))
    .unwrap();
    assert!(matches!(
        request.source,
        WorkspaceCreateSource::Directory { project_id: Some(project), .. } if project == "prj_1"
    ));

    for invalid in [
        json!({"workspaceId":"wks_UPPERCASE000000", "source":{"kind":"directory","path":"/repo"}}),
        json!({"idempotencyKey":"", "source":{"kind":"directory","path":"/repo"}}),
        json!({"source":{"kind":"unknown","path":"/repo"}}),
    ] {
        assert!(serde_json::from_value::<WorkspaceCreateRequest>(invalid).is_err());
    }
}

#[test]
fn validates_workspace_page_shape_during_deserialization() {
    let request: WorkspaceListRequest = serde_json::from_value(json!({
        "filter": {"projectId": "prj_1", "idPrefix": "ignored"},
        "sort": [{"key": "name", "direction": "asc"}],
        "page": {"limit": 20, "cursor": "10"}
    }))
    .unwrap();
    assert_eq!(request.page.unwrap().limit, 20);
    assert_eq!(
        request.filter.unwrap().id_prefix.as_deref(),
        Some("ignored")
    );
}
