//! Paseo's native approval/question contracts, restricted to Rust's blocking-question support.

use super::*;

fn session() -> CodexSession {
    CodexSession {
        pending_plan: None,
        latest_plan: None,
        pending_goal_start: false,
        notes: crate::local::notes::Notes::default(),
        last_anchor: None,
        manual_compactions: 0,
        children: super::super::super::subagents::Live::default(),
        questions: super::super::super::async_questions::Questions::default(),
        transport: None,
        id: "thread".to_owned(),
        info: serde_json::from_value(json!({"provider":"codex","sessionId":"thread"})).unwrap(),
        active_turn: Some("turn".to_owned()),
        last_message: None,
        history: false,
        cwd: "/offline-fixture".to_owned(),
        permissions: std::collections::BTreeMap::new(),
        stream: crate::local::codex::streaming::Stream::default(),
        config: server_domain::agent_runtime::StoredAgentConfig::default(),
        client: crate::local::codex::CodexClient::new("unused".into()),
    }
}

#[test]
fn native_permission_resolution_retires_only_its_matching_public_request() {
    let mut session = session();
    let native = request(json!("scope"), "item/commandExecution/requestApproval");
    let AgentTurnEvent::PermissionRequested(public) = session.capture_permission(&native).unwrap()
    else {
        panic!();
    };
    assert_eq!(session.resolve_native_permission(&json!("other")), None);
    assert_eq!(session.permissions.len(), 1);
    assert_eq!(
        session
            .resolve_native_permission(&json!("scope"))
            .as_deref(),
        public["id"].as_str()
    );
    assert!(session.permissions.is_empty());
    assert_eq!(session.resolve_native_permission(&json!("scope")), None);
}

#[test]
fn persistent_native_decisions_require_an_advertised_action_and_preserve_exact_scope() {
    let mut session = session();
    let mut native = request(json!("scope"), "item/commandExecution/requestApproval");
    native["params"]["proposedExecpolicyAmendment"] = json!(["git", "status"]);
    native["params"]["proposedNetworkPolicyAmendments"] =
        json!([{"host":"example.com","action":"allow"}]);
    let AgentTurnEvent::PermissionRequested(public) = session.capture_permission(&native).unwrap()
    else {
        panic!("permission");
    };
    let pending = &session.permissions[public["id"].as_str().unwrap()];
    assert_eq!(
        resolution(
            pending,
            &json!({"behavior":"allow","selectedActionId":"allow-session"})
        )
        .unwrap(),
        json!({"decision":"acceptForSession"})
    );
    assert_eq!(
        resolution(
            pending,
            &json!({"behavior":"allow","selectedActionId":"allow-prefix"})
        )
        .unwrap(),
        json!({"decision":{"acceptWithExecpolicyAmendment":{"execpolicy_amendment":["git","status"]}}})
    );
    assert_eq!(
        resolution(
            pending,
            &json!({"behavior":"allow","selectedActionId":"network-0"})
        )
        .unwrap(),
        json!({"decision":{"applyNetworkPolicyAmendment":{"network_policy_amendment":{"host":"example.com","action":"allow"}}}})
    );
    assert!(
        resolution(
            pending,
            &json!({"behavior":"allow","selectedActionId":"network-1"})
        )
        .is_err()
    );
    assert!(
        resolution(
            pending,
            &json!({"behavior":"deny","selectedActionId":"allow-session"})
        )
        .is_err()
    );
}

#[test]
fn mcp_form_and_client_header_answers_are_normalized_without_changing_provider_identity() {
    let mut session = session();
    let native = json!({"id":77,"method":"mcpServer/elicitation/request","params":{
        "threadId":"thread","turnId":null,"serverName":"docs","mode":"form","message":"Authorize", "requestedSchema":{
            "type":"object","required":["confirm"],"properties":{"confirm":{"type":"boolean"}}}}});
    let AgentTurnEvent::PermissionRequested(public) = session.capture_permission(&native).unwrap()
    else {
        panic!("permission");
    };
    assert_eq!(public["kind"], "question");
    let pending = &session.permissions[public["id"].as_str().unwrap()];
    assert_eq!(
        resolution(
            pending,
            &json!({"behavior":"allow","updatedInput":{"answers":{"confirm":"true"}}})
        )
        .unwrap(),
        json!({"action":"accept","content":{"confirm":true},"_meta":null})
    );
    assert_eq!(
        resolution(pending, &json!({"behavior":"deny"})).unwrap()["action"],
        "decline"
    );
    let native = request(json!("question"), "item/tool/requestUserInput");
    let AgentTurnEvent::PermissionRequested(public) = session.capture_permission(&native).unwrap()
    else {
        panic!("permission");
    };
    let pending = &session.permissions[public["id"].as_str().unwrap()];
    let mut input = public["input"].clone();
    input["answers"] = json!({"Choice":"yes"});
    assert_eq!(
        resolution(pending, &json!({"behavior":"allow","updatedInput":input})).unwrap(),
        json!({"answers":{"choice":{"answers":["yes"]}}})
    );
}

fn request(id: Value, method: &str) -> Value {
    let mut request = json!({"method":method,"params":{
        "threadId":"thread","turnId":"turn","itemId":"call", "command":"pwd",
        "questions":[{"id":"choice","header":"Choice","question":"Choose"}]
    }});
    request["id"] = id;
    request
}

#[test]
fn command_approval_uses_a_public_request_id_without_exposing_native_identity_as_authority() {
    let mut session = session();
    let native = request(json!(7), "item/commandExecution/requestApproval");
    let AgentTurnEvent::PermissionRequested(public) = session.capture_permission(&native).unwrap()
    else {
        panic!("expected a permission request");
    };
    let public_id = public["id"].as_str().unwrap();
    assert!(Uuid::parse_str(public_id).is_ok());
    assert_eq!(public["kind"], "tool");
    assert_eq!(public["name"], "commandExecution");
    assert_eq!(public["input"], native["params"]);
    assert_eq!(session.permissions[public_id].native_id, 7);
    assert_eq!(public["actions"][0]["behavior"], "allow");
    assert_eq!(public["actions"][1]["behavior"], "deny");
}

#[test]
fn repeated_native_permission_id_is_rejected_without_adding_another_pending_request() {
    let mut session = session();
    let native = request(json!("native-id"), "item/commandExecution/requestApproval");
    session.capture_permission(&native).unwrap();
    assert_eq!(
        session.capture_permission(&native),
        Err(AgentSessionError::Failed)
    );
    assert_eq!(session.permissions.len(), 1);
}

#[test]
fn native_permission_ids_remain_distinct_between_integer_and_string_forms() {
    let mut session = session();
    session
        .capture_permission(&request(json!(7), "item/commandExecution/requestApproval"))
        .unwrap();
    session
        .capture_permission(&request(
            json!("7"),
            "item/commandExecution/requestApproval",
        ))
        .unwrap();
    assert_eq!(session.permissions.len(), 2);
}

#[test]
fn foreign_thread_approval_cannot_enter_the_current_session() {
    let mut session = session();
    let mut native = request(json!(7), "item/commandExecution/requestApproval");
    native["params"]["threadId"] = json!("foreign");
    assert_eq!(
        session.capture_permission(&native),
        Err(AgentSessionError::Failed)
    );
    assert!(session.permissions.is_empty());
}

#[test]
fn stale_turn_approval_cannot_enter_the_current_turn() {
    let mut session = session();
    let mut native = request(json!(7), "item/commandExecution/requestApproval");
    native["params"]["turnId"] = json!("previous");
    assert_eq!(
        session.capture_permission(&native),
        Err(AgentSessionError::Failed)
    );
    assert!(session.permissions.is_empty());
}

#[test]
fn approval_without_an_active_turn_is_not_exposed() {
    let mut session = session();
    session.active_turn = None;
    assert_eq!(
        session.capture_permission(&request(json!(7), "item/fileChange/requestApproval")),
        Err(AgentSessionError::Failed)
    );
    assert!(session.permissions.is_empty());
}

#[test]
fn pending_permission_budget_does_not_evict_existing_requests() {
    let mut session = session();
    for id in 0..32 {
        session
            .capture_permission(&request(json!(id), "item/commandExecution/requestApproval"))
            .unwrap();
    }
    assert_eq!(
        session.capture_permission(&request(json!(32), "item/commandExecution/requestApproval")),
        Err(AgentSessionError::Failed)
    );
    assert_eq!(session.permissions.len(), 32);
    assert!(
        session
            .permissions
            .values()
            .any(|pending| pending.native_id == 0)
    );
}

#[test]
fn malformed_native_request_ids_are_rejected_before_publication() {
    let mut session = session();
    for id in [Value::Null, json!({}), json!(1.5), json!(u64::MAX)] {
        assert_eq!(
            session.capture_permission(&request(id, "item/commandExecution/requestApproval")),
            Err(AgentSessionError::Failed)
        );
    }
    assert!(session.permissions.is_empty());
}

#[test]
fn oversized_native_approval_is_not_retained() {
    let mut session = session();
    let mut native = request(json!(7), "item/commandExecution/requestApproval");
    native["params"]["command"] = json!("x".repeat(65_536));
    assert_eq!(
        session.capture_permission(&native),
        Err(AgentSessionError::Failed)
    );
    assert!(session.permissions.is_empty());
}

#[test]
fn blocking_questions_preserve_question_identity_and_actions() {
    let mut session = session();
    let AgentTurnEvent::PermissionRequested(public) = session
        .capture_permission(&request(json!(7), "item/tool/requestUserInput"))
        .unwrap()
    else {
        panic!("expected a question request");
    };
    assert_eq!(public["kind"], "question");
    assert_eq!(public["name"], "request_user_input");
    assert_eq!(public["input"]["questions"][0]["id"], "choice");
}

#[test]
fn nonblocking_native_questions_are_explicitly_unsupported() {
    let mut session = session();
    let mut native = request(json!(7), "item/tool/requestUserInput");
    native["params"]["isBlocking"] = json!(false);
    assert_eq!(
        session.capture_permission(&native),
        Err(AgentSessionError::Unavailable)
    );
    assert!(session.permissions.is_empty());
}

#[test]
fn duplicate_or_missing_question_identity_does_not_create_pending_state() {
    let mut session = session();
    let mut native = request(json!(7), "item/tool/requestUserInput");
    for questions in [
        json!([]),
        json!([{"id":"same"},{"id":"same"}]),
        json!([{"header":"missing id"}]),
    ] {
        native["params"]["questions"] = questions;
        assert_eq!(
            session.capture_permission(&native),
            Err(AgentSessionError::Rejected)
        );
    }
    assert!(session.permissions.is_empty());
}

#[test]
fn multiple_question_answers_keep_native_ids_and_selected_values() {
    let mut pending = pending(Kind::Question);
    pending.request["input"]["questions"] = json!([{"id":"drink"},{"id":"snack"}]);
    let response = json!({"behavior":"allow","updatedInput":{"answers":{"drink":"tea","snack":["fruit","bread"]}}});
    assert_eq!(
        resolution(&pending, &response).unwrap(),
        json!({"answers":{"drink":{"answers":["tea"]},"snack":{"answers":["fruit","bread"]}}})
    );
}

#[test]
fn partial_question_submission_is_rejected_until_every_question_is_answered() {
    let mut pending = pending(Kind::Question);
    pending.request["input"]["questions"] = json!([{"id":"drink"},{"id":"snack"}]);
    assert_eq!(
        resolution(
            &pending,
            &json!({"behavior":"allow","updatedInput":{"answers":{"drink":"tea"}}})
        ),
        Err(AgentSessionError::Rejected)
    );
}

#[test]
fn question_answer_count_is_bounded_per_question() {
    let pending = pending(Kind::Question);
    let response = |count| json!({"behavior":"allow","updatedInput":{"answers":{"choice":vec!["answer";count]}}});
    assert!(resolution(&pending, &response(32)).is_ok());
    assert_eq!(
        resolution(&pending, &response(33)),
        Err(AgentSessionError::Rejected)
    );
}

#[test]
fn question_answer_size_counts_utf8_bytes() {
    let pending = pending(Kind::Question);
    let response =
        |text: &str| json!({"behavior":"allow","updatedInput":{"answers":{"choice":text}}});
    assert!(resolution(&pending, &response(&"x".repeat(4096))).is_ok());
    assert_eq!(
        resolution(&pending, &response(&"界".repeat(1366))),
        Err(AgentSessionError::Rejected)
    );
}

#[test]
fn question_input_cannot_smuggle_unrequested_tool_arguments() {
    let pending = pending(Kind::Question);
    assert_eq!(
        resolution(
            &pending,
            &json!({"behavior":"allow","updatedInput":{"answers":{"choice":"yes"},"command":"modified"}})
        ),
        Err(AgentSessionError::Rejected)
    );
}

#[test]
fn file_approval_without_grant_root_is_a_one_call_decision() {
    let pending = pending(Kind::File);
    assert_eq!(
        resolution(
            &pending,
            &json!({"behavior":"allow","selectedActionId":"allow"})
        )
        .unwrap(),
        json!({"decision":"accept"})
    );
    assert_eq!(
        resolution(&pending, &json!({"behavior":"deny","message":"declined"})).unwrap(),
        json!({"decision":"decline"})
    );
}

#[tokio::test]
async fn failed_native_answer_keeps_the_permission_pending_for_retry() {
    let mut session = session();
    session
        .permissions
        .insert("public".to_owned(), pending(Kind::Command));
    assert_eq!(
        session
            .answer_permission("public", &json!({"behavior":"allow"}))
            .await,
        Err(AgentSessionError::Failed)
    );
    assert!(session.permissions.contains_key("public"));
}

#[tokio::test]
async fn unknown_permission_response_cannot_consume_another_pending_request() {
    let mut session = session();
    session
        .permissions
        .insert("public".to_owned(), pending(Kind::Command));
    assert_eq!(
        session
            .answer_permission("unknown", &json!({"behavior":"allow"}))
            .await,
        Err(AgentSessionError::Rejected)
    );
    assert_eq!(session.permissions.len(), 1);
}
