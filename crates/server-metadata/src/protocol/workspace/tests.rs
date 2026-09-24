use serde::de::DeserializeOwned;
use serde_json::Value;

use super::*;

fn parse<T: DeserializeOwned + Serialize>(input: &Value) -> serde_json::Result<Value> {
    serde_json::from_value::<T>(input.clone()).and_then(serde_json::to_value)
}

#[test]
fn workspace_wire_shapes_match_pinned_paseo_zod() {
    let fixtures: Value =
        serde_json::from_str(include_str!("../../../tests/fixtures/paseo-workspace.json")).unwrap();
    for case in fixtures["cases"].as_array().unwrap() {
        let result = match case["schema"].as_str().unwrap() {
            "ProjectCheckoutLitePayload" => parse::<ProjectCheckoutLitePayload>(&case["input"]),
            "ProjectPlacementPayload" => parse::<ProjectPlacementPayload>(&case["input"]),
            "WorkspaceScriptPayload" => parse::<WorkspaceScriptPayload>(&case["input"]),
            "WorkspaceDescriptorPayload" => parse::<WorkspaceDescriptorPayload>(&case["input"]),
            "WorkspaceProjectDescriptorPayload" => {
                parse::<WorkspaceProjectDescriptorPayload>(&case["input"])
            }
            "WorkspaceGitHubRuntimePayload" => {
                parse::<WorkspaceGitHubRuntimePayload>(&case["input"])
            }
            schema => panic!("unexpected fixture schema {schema}"),
        };
        assert_eq!(
            result.is_ok(),
            case["valid"].as_bool().unwrap(),
            "{} {}: {result:?}",
            case["schema"],
            case["name"]
        );
        if let Ok(output) = result {
            assert_eq!(
                output, case["output"],
                "{} {}",
                case["schema"], case["name"]
            );
        }
    }
}
