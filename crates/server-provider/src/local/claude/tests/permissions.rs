use super::*;
use crate::local::claude::permissions;

fn request(name: &str, input: &Value) -> permissions::Pending {
    permissions::capture(&json!({"type":"control_request","request_id":"native",
        "request":{"subtype":"can_use_tool","tool_name":name,"input":input}}))
    .unwrap()
}

#[test]
fn approvals_do_not_expand_permissions_without_explicit_valid_updates() {
    let pending = request("Bash", &json!({"command":"pwd"}));
    let response = permissions::resolve(&pending, &json!({"behavior":"allow"})).unwrap();
    assert_eq!(response["response"]["request_id"], "native");
    assert_eq!(
        response["response"]["response"]["updatedInput"],
        json!({"command":"pwd"})
    );
    let denial = permissions::resolve(
        &pending,
        &json!({"behavior":"deny","message":"No","interrupt":true}),
    )
    .unwrap();
    assert_eq!(denial["response"]["response"]["interrupt"], true);
    for invalid in [
        json!({"behavior":"allow","updatedPermissions":[{}]}),
        json!({"behavior":"allow","updatedInput":[]}),
        json!({"behavior":"allow","selectedActionId":"deny"}),
        json!({"behavior":"deny","interrupt":"yes"}),
        json!({}),
    ] {
        assert_eq!(
            permissions::resolve(&pending, &invalid),
            Err(AgentSessionError::Rejected)
        );
    }
    assert!(
        permissions::capture(&json!({"request_id":"native","request":{"subtype":"unknown"}}))
            .is_err()
    );
}

#[test]
fn questions_normalize_header_answers_without_leaking_ui_metadata() {
    let input = json!({"questions":[{"header":"Color","question":"Which color?","options":[],"multiSelect":false}]});
    let pending = request("AskUserQuestion", &input);
    assert_eq!(pending.request["kind"], "question");
    assert_eq!(pending.request["input"]["questions"][0]["allowOther"], true);
    let mut input = pending.request["input"].clone();
    input["answers"] = json!({"Color":"Blue"});
    assert!(
        permissions::resolve(&pending, &json!({"behavior":"allow","updatedInput":input})).is_ok()
    );
    input["questions"][0]["question"] = json!("Altered question");
    assert!(
        permissions::resolve(&pending, &json!({"behavior":"allow","updatedInput":input})).is_err()
    );
    for key in ["Color", "Which color?"] {
        let result = permissions::resolve(
            &pending,
            &json!({"behavior":"allow","updatedInput":{"answers":{(key):"Blue"}}}),
        )
        .unwrap();
        assert_eq!(
            result["response"]["response"]["updatedInput"]["answers"],
            json!({"Which color?":"Blue"})
        );
        assert!(
            result["response"]["response"]["updatedInput"]["questions"][0]
                .get("allowOther")
                .is_none()
        );
    }
    assert!(
        permissions::resolve(
            &pending,
            &json!({"behavior":"allow","updatedInput":{"answers":{"wrong":"Blue"}}})
        )
        .is_err()
    );
}

#[test]
fn explicit_native_rule_selection_preserves_destination_and_exact_rule() {
    let update = json!({"type":"addRules","behavior":"allow","destination":"session",
        "rules":[{"toolName":"Bash","ruleContent":"git status"}]});
    let pending = permissions::capture(&json!({"request_id":"native","request":{
        "subtype":"can_use_tool","tool_name":"Bash","input":{"command":"git status"},
        "permission_suggestions":[update, {"type":"unknown"}]}}))
    .unwrap();
    assert_eq!(pending.request["suggestions"], json!([update]));
    let response = permissions::resolve(
        &pending,
        &json!({"behavior":"allow","selectedActionId":"allow-update-0"}),
    )
    .unwrap();
    assert_eq!(
        response["response"]["response"]["updatedPermissions"],
        json!([update])
    );
    for destination in [
        "userSettings",
        "projectSettings",
        "localSettings",
        "session",
    ] {
        let mut update = update.clone();
        update["destination"] = json!(destination);
        let result = permissions::resolve(&pending, &json!({"behavior":"allow","updatedPermissions":[update],"updatedInput":{"command":"git status --short"}})).unwrap();
        assert_eq!(
            result["response"]["response"]["updatedPermissions"][0]["destination"],
            destination
        );
        assert_eq!(
            result["response"]["response"]["updatedInput"]["command"],
            "git status --short"
        );
    }
    assert!(
        permissions::resolve(
            &pending,
            &json!({"behavior":"allow","selectedActionId":"allow-update-9"})
        )
        .is_err()
    );
    assert!(
        permissions::resolve(
            &pending,
            &json!({"behavior":"allow","selectedActionId":"allow-update-0","updatedPermissions":[]})
        )
        .is_err()
    );
    let plan = request("ExitPlanMode", &json!({"plan":"Implement"}));
    assert_eq!(plan.request["kind"], "plan");
}
