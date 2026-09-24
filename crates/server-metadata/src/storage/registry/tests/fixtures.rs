use crate::model::registry::{
    PersistedProjectKind, PersistedProjectRecord, PersistedWorkspaceRecord,
};
use crate::ports::registry::ActiveProjectInput;

pub(super) fn project(id: &str) -> PersistedProjectRecord {
    serde_json::from_value(serde_json::json!({
        "projectId":id,"rootPath":"/repo","kind":"git","displayName":"repo",
        "createdAt":"2026-03-01T00:00:00.000Z",
        "updatedAt":"2026-03-01T00:00:00.000Z","archivedAt":null
    }))
    .unwrap()
}

pub(super) fn workspace(id: &str) -> PersistedWorkspaceRecord {
    serde_json::from_value(serde_json::json!({
        "workspaceId":id,"projectId":"prj_one","cwd":"/repo",
        "kind":"local_checkout","displayName":"main",
        "createdAt":"2026-03-01T00:00:00.000Z",
        "updatedAt":"2026-03-01T00:00:00.000Z","archivedAt":null
    }))
    .unwrap()
}

pub(super) fn input(root: &str) -> ActiveProjectInput {
    ActiveProjectInput {
        root_path: root.to_owned(),
        kind: PersistedProjectKind::Git,
        display_name: "repo".to_owned(),
        project_key: None,
        timestamp: "2026-03-01T00:00:00.000Z".to_owned(),
    }
}
