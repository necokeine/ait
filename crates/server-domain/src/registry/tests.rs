use serde_json::{Value, json};

use super::*;

#[test]
fn records_match_pinned_paseo_zod_outputs_and_rejections() {
    let fixtures: Value =
        serde_json::from_str(include_str!("../../tests/fixtures/paseo-registry.json")).unwrap();
    for case in fixtures["cases"].as_array().unwrap() {
        let result = match case["schema"].as_str().unwrap() {
            "PersistedProjectRecord" => {
                serde_json::from_value::<PersistedProjectRecord>(case["input"].clone())
                    .and_then(serde_json::to_value)
            }
            "PersistedWorkspaceRecord" => {
                serde_json::from_value::<PersistedWorkspaceRecord>(case["input"].clone())
                    .and_then(serde_json::to_value)
            }
            schema => panic!("unexpected fixture schema {schema}"),
        };
        assert_eq!(
            result.is_ok(),
            case["valid"].as_bool().unwrap(),
            "{}: {result:?}",
            case["name"]
        );
        if let Ok(output) = result {
            assert_eq!(output, case["output"], "{}", case["name"]);
        }
    }
}

#[test]
fn explicit_names_override_derived_names_even_when_empty() {
    let mut project: PersistedProjectRecord = serde_json::from_value(json!({
        "projectId":"prj_one","rootPath":"/repo","kind":"git","displayName":"repo",
        "createdAt":"opaque","updatedAt":"opaque","archivedAt":null
    }))
    .unwrap();
    assert_eq!(project.display_name(), "repo");
    project.custom_name = Some(String::new());
    assert_eq!(project.display_name(), "");
    let mut workspace: PersistedWorkspaceRecord = serde_json::from_value(json!({
        "workspaceId":"wks_one","projectId":"prj_one","cwd":"/repo","kind":"local_checkout",
        "displayName":"main","createdAt":"opaque","updatedAt":"opaque","archivedAt":null
    }))
    .unwrap();
    assert_eq!(workspace.display_name(), "main");
    workspace.title = Some("Title".to_owned());
    assert_eq!(workspace.display_name(), "Title");
    workspace.title = Some(String::new());
    assert_eq!(workspace.display_name(), "");
}
