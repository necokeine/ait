//! In-memory protocol conformance tests for the Codex app-server adapter.

use std::{
    path::PathBuf,
    sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    },
    time::Duration,
};

use ait_agent_adapters::{
    AgentEvent, AgentRunRequest, AgentRunStatus, ApprovalDecision, ApprovalHandler, ApprovalPolicy,
    ApprovalRequest, SandboxMode,
    codex::{ClientInfo, drive_model_list_protocol, drive_protocol},
};
use ait_domain::{NativeApprovalFileChange, NativeApprovalFileChangeKind, NativeApprovalTarget};
use async_trait::async_trait;
use serde_json::{Value, json};
use tokio::{
    io::{AsyncBufReadExt, AsyncWriteExt, BufReader, split},
    sync::{Notify, mpsc},
};
use tokio_util::sync::CancellationToken;

fn request() -> AgentRunRequest {
    AgentRunRequest {
        request_id: "message-1".into(),
        model: Some("test-model".into()),
        reasoning_effort: Some("high".into()),
        project_instructions: Some("Use Python 3 for examples.".into()),
        prompt: "Inspect the project".into(),
        cwd: PathBuf::from("/workspace"),
        resume_thread_id: None,
        sandbox: SandboxMode::WorkspaceWrite,
        approval_policy: ApprovalPolicy::OnRequest,
        output_schema: Some(json!({"type":"object"})),
        approval_handler: None,
        cancellation: CancellationToken::new(),
    }
}

fn client() -> ClientInfo {
    ClientInfo {
        name: "test_client".into(),
        title: "Test Client".into(),
        version: "0.1.0".into(),
    }
}

async fn read_json<R: tokio::io::AsyncBufRead + Unpin>(lines: &mut tokio::io::Lines<R>) -> Value {
    serde_json::from_str(&lines.next_line().await.unwrap().unwrap()).unwrap()
}

async fn write_json<W: tokio::io::AsyncWrite + Unpin>(writer: &mut W, value: Value) {
    writer
        .write_all(value.to_string().as_bytes())
        .await
        .unwrap();
    writer.write_all(b"\n").await.unwrap();
    writer.flush().await.unwrap();
}

async fn observed_sandbox(mode: SandboxMode) -> String {
    let (client_io, server_io) = tokio::io::duplex(32 * 1024);
    let (client_read, client_write) = split(client_io);
    let (server_read, mut server_write) = split(server_io);
    let server = tokio::spawn(async move {
        let mut lines = BufReader::new(server_read).lines();
        read_json(&mut lines).await;
        write_json(&mut server_write, json!({"id":0,"result":{}})).await;
        read_json(&mut lines).await;
        let thread = read_json(&mut lines).await;
        write_json(
            &mut server_write,
            json!({"id":1,"result":{"thread":{"id":"thr-1"}}}),
        )
        .await;
        read_json(&mut lines).await;
        write_json(
            &mut server_write,
            json!({"id":2,"result":{"turn":{"id":"turn-1"}}}),
        )
        .await;
        write_json(
            &mut server_write,
            json!({"method":"turn/completed","params":{"turn":{"id":"turn-1","status":"completed"}}}),
        )
        .await;
        thread["params"]["sandbox"].as_str().unwrap().to_owned()
    });
    let mut run = request();
    run.sandbox = mode;
    let (sender, _receiver) = mpsc::channel(32);
    drive_protocol(
        client_read,
        client_write,
        run,
        client(),
        Arc::new(ait_agent_adapters::DenyAllApprovals),
        &sender,
    )
    .await
    .unwrap();
    server.await.unwrap()
}

#[tokio::test]
async fn sends_each_selected_sandbox_as_the_real_codex_run_argument() {
    assert_eq!(observed_sandbox(SandboxMode::ReadOnly).await, "read-only");
    assert_eq!(
        observed_sandbox(SandboxMode::WorkspaceWrite).await,
        "workspace-write"
    );
    assert_eq!(
        observed_sandbox(SandboxMode::DangerFullAccess).await,
        "danger-full-access"
    );
}

#[tokio::test]
async fn discovers_picker_visible_models_and_reasoning_efforts_across_pages() {
    let (client_io, server_io) = tokio::io::duplex(32 * 1024);
    let (client_read, client_write) = split(client_io);
    let (server_read, mut server_write) = split(server_io);
    let server = tokio::spawn(async move {
        let mut lines = BufReader::new(server_read).lines();
        assert_eq!(read_json(&mut lines).await["method"], "initialize");
        write_json(&mut server_write, json!({"id": 0, "result": {}})).await;
        assert_eq!(read_json(&mut lines).await["method"], "initialized");
        let first = read_json(&mut lines).await;
        assert_eq!(first["method"], "model/list");
        assert_eq!(first["params"]["includeHidden"], false);
        assert!(first["params"].get("cursor").is_none());
        write_json(
            &mut server_write,
            json!({"id": 1, "result": {
                "data": [{
                    "model": "gpt-new",
                    "displayName": "GPT New",
                    "supportedReasoningEfforts": [
                        {"reasoningEffort": "low"},
                        {"reasoningEffort": "high"}
                    ]
                }],
                "nextCursor": "page-2"
            }}),
        )
        .await;
        let second = read_json(&mut lines).await;
        assert_eq!(second["id"], 2);
        assert_eq!(second["params"]["cursor"], "page-2");
        write_json(
            &mut server_write,
            json!({"id": 2, "result": {
                "data": [{
                    "model": "gpt-fast",
                    "displayName": "",
                    "supportedReasoningEfforts": [{"reasoningEffort": "medium"}]
                }],
                "nextCursor": null
            }}),
        )
        .await;
    });

    let models = drive_model_list_protocol(client_read, client_write, client())
        .await
        .unwrap();
    server.await.unwrap();
    assert_eq!(models.len(), 2);
    assert_eq!(models[0].id, "gpt-new");
    assert_eq!(models[0].name, "GPT New");
    assert_eq!(models[0].reasoning_efforts, ["low", "high"]);
    assert_eq!(models[1].id, "gpt-fast");
    assert_eq!(models[1].name, "gpt-fast");
    assert_eq!(models[1].reasoning_efforts, ["medium"]);
}

#[tokio::test]
async fn maps_codex_jsonl_lifecycle_and_usage() {
    let (client_io, server_io) = tokio::io::duplex(32 * 1024);
    let (client_read, client_write) = split(client_io);
    let (server_read, mut server_write) = split(server_io);
    let server = tokio::spawn(async move {
        let mut lines = BufReader::new(server_read).lines();
        let initialize = read_json(&mut lines).await;
        assert_eq!(initialize["method"], "initialize");
        write_json(&mut server_write, json!({"id": 0, "result": {}})).await;
        assert_eq!(read_json(&mut lines).await["method"], "initialized");
        let thread = read_json(&mut lines).await;
        assert_eq!(thread["method"], "thread/start");
        assert_eq!(thread["params"]["sandbox"], "workspace-write");
        assert_eq!(
            thread["params"]["developerInstructions"],
            ait_tools::codex::CodexToolSet
                .developer_instructions(Some("Use Python 3 for examples.")),
        );
        assert!(
            !thread["params"]["developerInstructions"]
                .as_str()
                .unwrap()
                .contains("Inspect the project")
        );
        for key in ["baseInstructions", "tools", "dynamicTools"] {
            assert!(
                thread["params"].get(key).is_none(),
                "must preserve native core {key}"
            );
        }
        write_json(
            &mut server_write,
            json!({"id": 1, "result": {"thread": {"id": "thr-1"}}}),
        )
        .await;
        let turn = read_json(&mut lines).await;
        assert_eq!(turn["method"], "turn/start");
        assert_eq!(turn["params"]["input"][0]["text"], "Inspect the project");
        assert_eq!(turn["params"]["effort"], "high");
        assert_eq!(turn["params"]["outputSchema"]["type"], "object");
        write_json(
            &mut server_write,
            json!({"id": 2, "result": {"turn": {"id": "turn-1"}}}),
        )
        .await;
        write_json(
            &mut server_write,
            json!({"method":"item/agentMessage/delta","params":{"threadId":"thr-1","turnId":"turn-1","itemId":"item-1","delta":"done"}}),
        )
        .await;
        write_json(
            &mut server_write,
            json!({"method":"thread/tokenUsage/updated","params":{"threadId":"thr-1","turnId":"turn-1","tokenUsage":{"last":{"inputTokens":3,"cachedInputTokens":1,"outputTokens":2,"reasoningOutputTokens":0,"totalTokens":5},"total":{"inputTokens":3,"cachedInputTokens":1,"outputTokens":2,"reasoningOutputTokens":0,"totalTokens":5}}}}),
        )
        .await;
        write_json(
            &mut server_write,
            json!({"method":"turn/completed","params":{"threadId":"thr-1","turn":{"id":"turn-1","items":[],"status":"completed"}}}),
        )
        .await;
    });

    let (sender, mut receiver) = mpsc::channel(32);
    let drive = tokio::spawn(async move {
        drive_protocol(
            client_read,
            client_write,
            request(),
            client(),
            Arc::new(ait_agent_adapters::DenyAllApprovals),
            &sender,
        )
        .await
    });
    let mut events = Vec::new();
    while let Some(event) = receiver.recv().await {
        events.push(event.unwrap());
    }
    drive.await.unwrap().unwrap();
    server.await.unwrap();
    assert!(matches!(&events[0], AgentEvent::ThreadStarted { thread_id } if thread_id == "thr-1"));
    assert!(matches!(&events[1], AgentEvent::TurnStarted { turn_id } if turn_id == "turn-1"));
    assert!(
        events.iter().any(
            |event| matches!(event, AgentEvent::MessageDelta { delta, .. } if delta == "done")
        )
    );
    assert!(
        events
            .iter()
            .any(|event| matches!(event, AgentEvent::Usage { usage } if usage.total_tokens == 5))
    );
    assert!(matches!(
        events.last(),
        Some(AgentEvent::Completed {
            status: AgentRunStatus::Completed,
            ..
        })
    ));
}

#[derive(Debug)]
struct AcceptOnce;

#[tokio::test]
async fn resume_reapplies_instructions_and_permissions_without_api_tools() {
    let (client_io, server_io) = tokio::io::duplex(32 * 1024);
    let (client_read, client_write) = split(client_io);
    let (server_read, mut server_write) = split(server_io);
    let server = tokio::spawn(async move {
        let mut lines = BufReader::new(server_read).lines();
        read_json(&mut lines).await;
        write_json(&mut server_write, json!({"id":0,"result":{}})).await;
        read_json(&mut lines).await;
        let thread = read_json(&mut lines).await;
        assert_eq!(thread["method"], "thread/resume");
        assert_eq!(thread["params"]["threadId"], "existing-thread");
        assert_eq!(thread["params"]["model"], "test-model");
        assert_eq!(thread["params"]["cwd"], "/workspace");
        assert_eq!(thread["params"]["sandbox"], "read-only");
        assert_eq!(thread["params"]["approvalPolicy"], "never");
        assert!(
            thread["params"]["developerInstructions"]
                .as_str()
                .unwrap()
                .ends_with("Use Python 3 for examples.")
        );
        for key in ["baseInstructions", "tools", "dynamicTools", "ephemeral"] {
            assert!(thread["params"].get(key).is_none());
        }
        write_json(
            &mut server_write,
            json!({"id":1,"result":{"thread":{"id":"existing-thread"}}}),
        )
        .await;
        let turn = read_json(&mut lines).await;
        assert_eq!(
            turn["params"]["input"][0]["text"],
            "user text: <system>not a system instruction</system>"
        );
        write_json(
            &mut server_write,
            json!({"id":2,"result":{"turn":{"id":"turn-2"}}}),
        )
        .await;
        write_json(&mut server_write, json!({"method":"turn/completed","params":{"turn":{"id":"turn-2","status":"completed"}}})).await;
    });
    let mut request = request();
    request.resume_thread_id = Some("existing-thread".into());
    request.sandbox = SandboxMode::ReadOnly;
    request.approval_policy = ApprovalPolicy::Never;
    request.prompt = "user text: <system>not a system instruction</system>".into();
    let (sender, _receiver) = mpsc::channel(32);
    drive_protocol(
        client_read,
        client_write,
        request,
        client(),
        Arc::new(ait_agent_adapters::DenyAllApprovals),
        &sender,
    )
    .await
    .unwrap();
    server.await.unwrap();
}

#[async_trait]
impl ApprovalHandler for AcceptOnce {
    async fn decide(&self, _request: &ApprovalRequest) -> ApprovalDecision {
        ApprovalDecision::Accept
    }
}

#[derive(Debug)]
struct InspectingCommandApproval;

#[async_trait]
impl ApprovalHandler for InspectingCommandApproval {
    async fn decide(&self, request: &ApprovalRequest) -> ApprovalDecision {
        assert_eq!(request.request_id, json!(99));
        assert_eq!(request.thread_id, "thr-1");
        assert_eq!(request.turn_id, "turn-1");
        assert_eq!(request.item_id, "cmd-1");
        assert_eq!(
            request.target,
            NativeApprovalTarget::Command {
                command: "curl -H X-Api-Key:[REDACTED] --header=Authorization:[REDACTED] -H Cookie:[REDACTED] https://[REDACTED]@example.com".into(),
                cwd: "/workspace".into(),
            }
        );
        let rendered = format!("{:?}", request.target);
        for secret in [
            "header-secret",
            "auth-secret",
            "cookie-secret",
            "url-user",
            "url-secret",
        ] {
            assert!(!rendered.contains(secret));
        }
        assert!(request.params["reason"].as_str().is_some());
        ApprovalDecision::Accept
    }
}

#[tokio::test]
async fn routes_command_approvals_through_handler() {
    let (client_io, server_io) = tokio::io::duplex(32 * 1024);
    let (client_read, client_write) = split(client_io);
    let (server_read, mut server_write) = split(server_io);
    let server = tokio::spawn(async move {
        let mut lines = BufReader::new(server_read).lines();
        let _ = read_json(&mut lines).await;
        write_json(&mut server_write, json!({"id": 0, "result": {}})).await;
        let _ = read_json(&mut lines).await;
        let _ = read_json(&mut lines).await;
        write_json(
            &mut server_write,
            json!({"id": 1, "result": {"thread": {"id": "thr-1"}}}),
        )
        .await;
        let _ = read_json(&mut lines).await;
        write_json(
            &mut server_write,
            json!({"id": 2, "result": {"turn": {"id": "turn-1"}}}),
        )
        .await;
        write_json(
            &mut server_write,
            json!({"id":99,"method":"item/commandExecution/requestApproval","params":{"threadId":"thr-1","turnId":"turn-1","itemId":"cmd-1","command":"curl -H X-Api-Key:header-secret --header=\"Authorization: Bearer auth-secret\" -H \"Cookie: session=cookie-secret\" https://url-user:url-secret@example.com","cwd":"/workspace","reason":"needs permission"}}),
        )
        .await;
        let response = read_json(&mut lines).await;
        assert_eq!(response["id"], 99);
        assert_eq!(response["result"]["decision"], "accept");
        write_json(
            &mut server_write,
            json!({"method":"turn/completed","params":{"threadId":"thr-1","turn":{"id":"turn-1","items":[],"status":"completed"}}}),
        )
        .await;
    });
    let (sender, mut receiver) = mpsc::channel(32);
    let drive = tokio::spawn(async move {
        drive_protocol(
            client_read,
            client_write,
            request(),
            client(),
            Arc::new(InspectingCommandApproval),
            &sender,
        )
        .await
    });
    let mut saw_approval = false;
    while let Some(event) = receiver.recv().await {
        if matches!(event.unwrap(), AgentEvent::ApprovalRequested { .. }) {
            saw_approval = true;
        }
    }
    drive.await.unwrap().unwrap();
    server.await.unwrap();
    assert!(saw_approval);
}

#[derive(Debug)]
struct PermissionGrant;

#[async_trait]
impl ApprovalHandler for PermissionGrant {
    async fn decide(&self, request: &ApprovalRequest) -> ApprovalDecision {
        assert_eq!(request.request_id, json!("permission-7"));
        assert_eq!(request.thread_id, "thr-1");
        assert_eq!(request.turn_id, "turn-1");
        assert_eq!(request.item_id, "permission-item");
        assert_eq!(
            request.target,
            NativeApprovalTarget::Permissions {
                cwd: "/workspace".into()
            }
        );
        ApprovalDecision::Raw(json!({
            "permissions": request.params["permissions"].clone(),
            "scope": "session"
        }))
    }
}

#[tokio::test]
async fn permission_approval_answers_the_original_request_id_with_explicit_profile_and_scope() {
    let (client_io, server_io) = tokio::io::duplex(32 * 1024);
    let (client_read, client_write) = split(client_io);
    let (server_read, mut server_write) = split(server_io);
    let permissions = json!({
        "fileSystem": {"write": ["/workspace"]},
        "network": {"enabled": false}
    });
    let expected = permissions.clone();
    let server = tokio::spawn(async move {
        let mut lines = BufReader::new(server_read).lines();
        read_json(&mut lines).await;
        write_json(&mut server_write, json!({"id":0,"result":{}})).await;
        read_json(&mut lines).await;
        read_json(&mut lines).await;
        write_json(
            &mut server_write,
            json!({"id":1,"result":{"thread":{"id":"thr-1"}}}),
        )
        .await;
        read_json(&mut lines).await;
        write_json(
            &mut server_write,
            json!({"id":2,"result":{"turn":{"id":"turn-1"}}}),
        )
        .await;
        write_json(
            &mut server_write,
            json!({
                "id":"permission-7",
                "method":"item/permissions/requestApproval",
                "params":{
                    "threadId":"thr-1",
                    "turnId":"turn-1",
                    "itemId":"permission-item",
                    "cwd":"/workspace",
                    "permissions": permissions
                }
            }),
        )
        .await;
        let response = read_json(&mut lines).await;
        assert_eq!(response["id"], "permission-7");
        assert_eq!(response["result"]["permissions"], expected);
        assert_eq!(response["result"]["scope"], "session");
        assert!(response["result"].get("decision").is_none());
        write_json(
            &mut server_write,
            json!({"method":"turn/completed","params":{"turn":{"id":"turn-1","status":"completed"}}}),
        )
        .await;
    });
    let (sender, mut receiver) = mpsc::channel(32);
    let drive = tokio::spawn(async move {
        drive_protocol(
            client_read,
            client_write,
            request(),
            client(),
            Arc::new(PermissionGrant),
            &sender,
        )
        .await
    });
    while receiver.recv().await.is_some() {}
    drive.await.unwrap().unwrap();
    server.await.unwrap();
}

#[derive(Debug)]
struct CountingApprovals(Arc<AtomicUsize>);

#[async_trait]
impl ApprovalHandler for CountingApprovals {
    async fn decide(&self, _request: &ApprovalRequest) -> ApprovalDecision {
        self.0.fetch_add(1, Ordering::Relaxed);
        ApprovalDecision::Accept
    }
}

#[tokio::test]
async fn mismatched_approval_correlation_fails_closed_before_the_handler() {
    let (client_io, server_io) = tokio::io::duplex(32 * 1024);
    let (client_read, client_write) = split(client_io);
    let (server_read, mut server_write) = split(server_io);
    let server = tokio::spawn(async move {
        let mut lines = BufReader::new(server_read).lines();
        read_json(&mut lines).await;
        write_json(&mut server_write, json!({"id":0,"result":{}})).await;
        read_json(&mut lines).await;
        read_json(&mut lines).await;
        write_json(
            &mut server_write,
            json!({"id":1,"result":{"thread":{"id":"thr-1"}}}),
        )
        .await;
        read_json(&mut lines).await;
        write_json(
            &mut server_write,
            json!({"id":2,"result":{"turn":{"id":"turn-1"}}}),
        )
        .await;
        write_json(
            &mut server_write,
            json!({"id":91,"method":"item/fileChange/requestApproval","params":{"threadId":"other-thread","turnId":"turn-1","itemId":"patch-1"}}),
        )
        .await;
        let response = read_json(&mut lines).await;
        assert_eq!(response["id"], 91);
        assert_eq!(response["error"]["code"], -32602);
        write_json(
            &mut server_write,
            json!({"method":"turn/completed","params":{"turn":{"id":"turn-1","status":"completed"}}}),
        )
        .await;
    });
    let calls = Arc::new(AtomicUsize::new(0));
    let (sender, mut receiver) = mpsc::channel(32);
    let handler = Arc::clone(&calls);
    let drive = tokio::spawn(async move {
        drive_protocol(
            client_read,
            client_write,
            request(),
            client(),
            Arc::new(CountingApprovals(handler)),
            &sender,
        )
        .await
    });
    let mut saw_warning = false;
    while let Some(event) = receiver.recv().await {
        saw_warning |= matches!(event.unwrap(), AgentEvent::AdapterWarning { code: Some(code), .. } if code == "CODEX_APPROVAL_CORRELATION_INVALID");
    }
    drive.await.unwrap().unwrap();
    server.await.unwrap();
    assert_eq!(calls.load(Ordering::Relaxed), 0);
    assert!(saw_warning);
}

#[tokio::test]
async fn ambiguous_or_missing_approval_target_fails_closed_before_the_handler() {
    let (client_io, server_io) = tokio::io::duplex(32 * 1024);
    let (client_read, client_write) = split(client_io);
    let (server_read, mut server_write) = split(server_io);
    let server = tokio::spawn(async move {
        let mut lines = BufReader::new(server_read).lines();
        read_json(&mut lines).await;
        write_json(&mut server_write, json!({"id":0,"result":{}})).await;
        read_json(&mut lines).await;
        read_json(&mut lines).await;
        write_json(
            &mut server_write,
            json!({"id":1,"result":{"thread":{"id":"thr-1"}}}),
        )
        .await;
        read_json(&mut lines).await;
        write_json(
            &mut server_write,
            json!({"id":2,"result":{"turn":{"id":"turn-1"}}}),
        )
        .await;
        write_json(
            &mut server_write,
            json!({"id":92,"method":"item/commandExecution/requestApproval","params":{"threadId":"thr-1","turnId":"turn-1","itemId":"cmd-1","reason":"arguments intentionally absent"}}),
        )
        .await;
        let response = read_json(&mut lines).await;
        assert_eq!(response["id"], 92);
        assert_eq!(response["error"]["code"], -32602);
        for (id, command) in [
            (93, json!("grep -H Authorization: /etc/passwd")),
            (94, json!(["grep", "-H", "Authorization:", "/etc/passwd"])),
        ] {
            write_json(
                &mut server_write,
                json!({"id":id,"method":"item/commandExecution/requestApproval","params":{"threadId":"thr-1","turnId":"turn-1","itemId":format!("cmd-{id}"),"command":command,"cwd":"/workspace","reason":"ambiguous header boundary"}}),
            )
            .await;
            let response = read_json(&mut lines).await;
            assert_eq!(response["id"], id);
            assert_eq!(response["error"]["code"], -32602);
        }
        write_json(
            &mut server_write,
            json!({"method":"turn/completed","params":{"turn":{"id":"turn-1","status":"completed"}}}),
        )
        .await;
    });
    let calls = Arc::new(AtomicUsize::new(0));
    let (sender, mut receiver) = mpsc::channel(32);
    let handler = Arc::clone(&calls);
    let drive = tokio::spawn(async move {
        drive_protocol(
            client_read,
            client_write,
            request(),
            client(),
            Arc::new(CountingApprovals(handler)),
            &sender,
        )
        .await
    });
    let mut saw_warning = false;
    let mut saw_approval = false;
    while let Some(event) = receiver.recv().await {
        match event.unwrap() {
            AgentEvent::AdapterWarning {
                code: Some(code), ..
            } if code == "CODEX_APPROVAL_TARGET_INVALID" => saw_warning = true,
            AgentEvent::ApprovalRequested { .. } => saw_approval = true,
            _ => {}
        }
    }
    drive.await.unwrap().unwrap();
    server.await.unwrap();
    assert_eq!(calls.load(Ordering::Relaxed), 0);
    assert!(saw_warning);
    assert!(!saw_approval);
}

#[tokio::test]
async fn declines_mcp_elicitation_with_the_original_id_and_continues() {
    let (client_io, server_io) = tokio::io::duplex(32 * 1024);
    let (client_read, client_write) = split(client_io);
    let (server_read, mut server_write) = split(server_io);
    let server = tokio::spawn(async move {
        let mut lines = BufReader::new(server_read).lines();
        let initialize = read_json(&mut lines).await;
        assert_eq!(
            initialize["params"]["capabilities"]["experimentalApi"],
            false
        );
        write_json(&mut server_write, json!({"id":0,"result":{}})).await;
        read_json(&mut lines).await;
        read_json(&mut lines).await;
        write_json(
            &mut server_write,
            json!({"id":1,"result":{"thread":{"id":"thr-1"}}}),
        )
        .await;
        read_json(&mut lines).await;
        write_json(
            &mut server_write,
            json!({"id":2,"result":{"turn":{"id":"turn-1"}}}),
        )
        .await;
        write_json(
            &mut server_write,
            json!({
                "id": "elicit-1",
                "method": "mcpServer/elicitation/request",
                "params": {
                    "threadId": "thr-1",
                    "turnId": "turn-1",
                    "serverName": "example",
                    "request": {"mode": "form", "message": "Choose", "requestedSchema": {}}
                }
            }),
        )
        .await;
        let response = read_json(&mut lines).await;
        assert_eq!(response["id"], "elicit-1");
        assert_eq!(response["result"]["action"], "decline");
        assert!(response["result"]["content"].is_null());
        write_json(
            &mut server_write,
            json!({"method":"serverRequest/resolved","params":{"threadId":"thr-1","requestId":"elicit-1"}}),
        )
        .await;
        write_json(
            &mut server_write,
            json!({"method":"item/agentMessage/delta","params":{"threadId":"thr-1","turnId":"turn-1","itemId":"answer","delta":"continued"}}),
        )
        .await;
        write_json(
            &mut server_write,
            json!({"method":"turn/completed","params":{"turn":{"id":"turn-1","status":"completed"}}}),
        )
        .await;
    });

    let (sender, mut receiver) = mpsc::channel(32);
    let drive = tokio::spawn(async move {
        drive_protocol(
            client_read,
            client_write,
            request(),
            client(),
            Arc::new(ait_agent_adapters::DenyAllApprovals),
            &sender,
        )
        .await
    });
    let mut events = Vec::new();
    while let Some(event) = receiver.recv().await {
        events.push(event.unwrap());
    }
    drive.await.unwrap().unwrap();
    server.await.unwrap();
    assert!(
        !events
            .iter()
            .any(|event| matches!(event, AgentEvent::ApprovalRequested { .. }))
    );
    assert!(events.iter().any(|event| matches!(
        event,
        AgentEvent::AdapterWarning { code: Some(code), .. }
            if code == "CODEX_MCP_ELICITATION_DECLINED"
    )));
    assert!(events.iter().any(|event| matches!(
        event,
        AgentEvent::MessageDelta { delta, .. } if delta == "continued"
    )));
    assert!(matches!(
        events.last(),
        Some(AgentEvent::Completed {
            status: AgentRunStatus::Completed,
            ..
        })
    ));
}

#[tokio::test]
#[allow(clippy::too_many_lines)]
async fn rejects_unavailable_server_request_methods_without_leaking_params() {
    let (client_io, server_io) = tokio::io::duplex(32 * 1024);
    let (client_read, client_write) = split(client_io);
    let (server_read, mut server_write) = split(server_io);
    let server = tokio::spawn(async move {
        let mut lines = BufReader::new(server_read).lines();
        read_json(&mut lines).await;
        write_json(&mut server_write, json!({"id":0,"result":{}})).await;
        read_json(&mut lines).await;
        read_json(&mut lines).await;
        write_json(
            &mut server_write,
            json!({"id":1,"result":{"thread":{"id":"thr-1"}}}),
        )
        .await;
        read_json(&mut lines).await;
        write_json(
            &mut server_write,
            json!({"id":2,"result":{"turn":{"id":"turn-1"}}}),
        )
        .await;

        for (id, method, params) in [
            (
                json!("input-1"),
                "item/tool/requestUserInput",
                json!({"questions": []}),
            ),
            (json!(41), "item/tool/call", json!({"tool": "missing"})),
            (
                json!(42),
                "account/chatgptAuthTokens/refresh",
                json!({"accessToken": "never-log-this-secret"}),
            ),
            (
                json!(43),
                "future/serverRequest",
                json!({"secret": "also-hidden"}),
            ),
        ] {
            write_json(
                &mut server_write,
                json!({"id": id, "method": method, "params": params}),
            )
            .await;
            let response = read_json(&mut lines).await;
            assert_eq!(response["id"], id);
            assert_eq!(response["error"]["code"], -32601);
            assert!(
                response["error"]["message"]
                    .as_str()
                    .unwrap()
                    .contains(method)
            );
        }
        write_json(
            &mut server_write,
            json!({"method":"item/agentMessage/delta","params":{"threadId":"thr-1","turnId":"turn-1","itemId":"answer","delta":"still running"}}),
        )
        .await;
        write_json(
            &mut server_write,
            json!({"method":"turn/completed","params":{"turn":{"id":"turn-1","status":"completed"}}}),
        )
        .await;
    });

    let (sender, mut receiver) = mpsc::channel(32);
    let drive = tokio::spawn(async move {
        drive_protocol(
            client_read,
            client_write,
            request(),
            client(),
            Arc::new(ait_agent_adapters::DenyAllApprovals),
            &sender,
        )
        .await
    });
    let mut events = Vec::new();
    while let Some(event) = receiver.recv().await {
        events.push(event.unwrap());
    }
    drive.await.unwrap().unwrap();
    server.await.unwrap();
    assert!(
        !events
            .iter()
            .any(|event| matches!(event, AgentEvent::ApprovalRequested { .. }))
    );
    let rendered = format!("{events:?}");
    assert!(!rendered.contains("never-log-this-secret"));
    assert!(!rendered.contains("also-hidden"));
    assert!(events.iter().any(|event| matches!(
        event,
        AgentEvent::MessageDelta { delta, .. } if delta == "still running"
    )));
    assert!(matches!(
        events.last(),
        Some(AgentEvent::Completed {
            status: AgentRunStatus::Completed,
            ..
        })
    ));
}

#[derive(Debug, Default)]
struct DeclineThenCancel(AtomicUsize);

#[async_trait]
impl ApprovalHandler for DeclineThenCancel {
    async fn decide(&self, request: &ApprovalRequest) -> ApprovalDecision {
        if self.0.fetch_add(1, Ordering::Relaxed) == 0 {
            assert!(matches!(
                request.target,
                NativeApprovalTarget::Command { .. }
            ));
            ApprovalDecision::Decline
        } else {
            assert_eq!(
                request.target,
                NativeApprovalTarget::FileChange {
                    grant_root: None,
                    changes: vec![NativeApprovalFileChange {
                        path: "src/main.rs".into(),
                        kind: NativeApprovalFileChangeKind::Update,
                    }],
                }
            );
            ApprovalDecision::Cancel
        }
    }
}

#[tokio::test]
async fn decline_continues_but_cancel_interrupts_the_original_turn() {
    let (client_io, server_io) = tokio::io::duplex(32 * 1024);
    let (client_read, client_write) = split(client_io);
    let (server_read, mut server_write) = split(server_io);
    let server = tokio::spawn(async move {
        let mut lines = BufReader::new(server_read).lines();
        read_json(&mut lines).await;
        write_json(&mut server_write, json!({"id":0,"result":{}})).await;
        read_json(&mut lines).await;
        read_json(&mut lines).await;
        write_json(
            &mut server_write,
            json!({"id":1,"result":{"thread":{"id":"thr-1"}}}),
        )
        .await;
        read_json(&mut lines).await;
        write_json(
            &mut server_write,
            json!({"id":2,"result":{"turn":{"id":"turn-1"}}}),
        )
        .await;
        write_json(
            &mut server_write,
            json!({"id":71,"method":"item/commandExecution/requestApproval","params":{"threadId":"thr-1","turnId":"turn-1","itemId":"item-71","command":"git status","cwd":"/workspace"}}),
        )
        .await;
        let response = read_json(&mut lines).await;
        assert_eq!(response["id"], 71);
        assert_eq!(response["result"]["decision"], "decline");
        write_json(
            &mut server_write,
            json!({"method":"item/started","params":{"threadId":"thr-1","turnId":"turn-1","item":{"type":"fileChange","id":"item-72","status":"inProgress","changes":[{"path":"src/main.rs","kind":"update","diff":"never persist this patch"}]}}}),
        )
        .await;
        write_json(
            &mut server_write,
            json!({"id":72,"method":"item/fileChange/requestApproval","params":{"threadId":"thr-1","turnId":"turn-1","itemId":"item-72"}}),
        )
        .await;
        let interrupt = read_json(&mut lines).await;
        assert_eq!(interrupt["method"], "turn/interrupt");
        assert_eq!(interrupt["params"]["turnId"], "turn-1");
    });

    let (sender, mut receiver) = mpsc::channel(32);
    let drive = tokio::spawn(async move {
        drive_protocol(
            client_read,
            client_write,
            request(),
            client(),
            Arc::new(DeclineThenCancel::default()),
            &sender,
        )
        .await
    });
    let mut approvals = 0;
    while let Some(event) = receiver.recv().await {
        if matches!(event.unwrap(), AgentEvent::ApprovalRequested { .. }) {
            approvals += 1;
        }
    }
    let error = drive.await.unwrap().unwrap_err();
    server.await.unwrap();
    assert_eq!(approvals, 2);
    assert_eq!(error.kind, ait_agent_adapters::AdapterErrorKind::Cancelled);
}

#[derive(Debug, Default)]
struct PendingApproval {
    release: Notify,
}

#[derive(Debug)]
struct CancelApproval;

#[async_trait]
impl ApprovalHandler for CancelApproval {
    async fn decide(&self, _request: &ApprovalRequest) -> ApprovalDecision {
        ApprovalDecision::Cancel
    }
}

#[tokio::test]
async fn cancelling_a_permission_request_interrupts_instead_of_granting_empty_permissions() {
    let (client_io, server_io) = tokio::io::duplex(32 * 1024);
    let (client_read, client_write) = split(client_io);
    let (server_read, mut server_write) = split(server_io);
    let server = tokio::spawn(async move {
        let mut lines = BufReader::new(server_read).lines();
        read_json(&mut lines).await;
        write_json(&mut server_write, json!({"id":0,"result":{}})).await;
        read_json(&mut lines).await;
        read_json(&mut lines).await;
        write_json(
            &mut server_write,
            json!({"id":1,"result":{"thread":{"id":"thr-1"}}}),
        )
        .await;
        read_json(&mut lines).await;
        write_json(
            &mut server_write,
            json!({"id":2,"result":{"turn":{"id":"turn-1"}}}),
        )
        .await;
        write_json(
            &mut server_write,
            json!({
                "id": "permission-cancel",
                "method": "item/permissions/requestApproval",
                "params": {
                    "threadId": "thr-1",
                    "turnId": "turn-1",
                    "itemId": "permission-item",
                    "cwd": "/workspace",
                    "permissions": {"network": {"enabled": true}}
                }
            }),
        )
        .await;
        let interrupt = read_json(&mut lines).await;
        assert_eq!(interrupt["method"], "turn/interrupt");
        assert_eq!(interrupt["params"]["turnId"], "turn-1");
        assert!(interrupt.get("result").is_none());
    });
    let (sender, mut receiver) = mpsc::channel(32);
    let drive = tokio::spawn(async move {
        drive_protocol(
            client_read,
            client_write,
            request(),
            client(),
            Arc::new(CancelApproval),
            &sender,
        )
        .await
    });
    while receiver.recv().await.is_some() {}
    let failure = drive.await.unwrap().unwrap_err();
    server.await.unwrap();
    assert_eq!(
        failure.kind,
        ait_agent_adapters::AdapterErrorKind::Cancelled
    );
}

#[async_trait]
impl ApprovalHandler for PendingApproval {
    async fn decide(&self, _request: &ApprovalRequest) -> ApprovalDecision {
        self.release.notified().await;
        ApprovalDecision::Accept
    }
}

#[tokio::test]
async fn buffered_resolution_wins_over_immediately_completed_approvals() {
    let (client_io, server_io) = tokio::io::duplex(32 * 1024);
    let (client_read, client_write) = split(client_io);
    let (server_read, mut server_write) = split(server_io);
    let server = tokio::spawn(async move {
        let mut lines = BufReader::new(server_read).lines();
        read_json(&mut lines).await;
        write_json(&mut server_write, json!({"id":0,"result":{}})).await;
        read_json(&mut lines).await;
        read_json(&mut lines).await;
        write_json(
            &mut server_write,
            json!({"id":1,"result":{"thread":{"id":"thr-1"}}}),
        )
        .await;
        read_json(&mut lines).await;
        write_json(
            &mut server_write,
            json!({"id":2,"result":{"turn":{"id":"turn-1"}}}),
        )
        .await;
        // Repetition makes the regression fail reliably with an unbiased select.
        for request_id in 100..132 {
            write_json(
                &mut server_write,
                json!({"id":request_id,"method":"item/commandExecution/requestApproval","params":{"threadId":"thr-1","turnId":"turn-1","itemId":format!("cmd-{request_id}"),"command":"git status","cwd":"/workspace"}}),
            )
            .await;
            write_json(
                &mut server_write,
                json!({"method":"serverRequest/resolved","params":{"threadId":"thr-1","requestId":request_id}}),
            )
            .await;
        }
        write_json(
            &mut server_write,
            json!({"method":"item/agentMessage/delta","params":{"threadId":"thr-1","turnId":"turn-1","itemId":"answer","delta":"not blocked"}}),
        )
        .await;
        assert!(
            tokio::time::timeout(Duration::from_millis(50), lines.next_line())
                .await
                .is_err(),
            "a resolved request must not receive a late approval response"
        );
        write_json(
            &mut server_write,
            json!({"method":"turn/completed","params":{"turn":{"id":"turn-1","status":"completed"}}}),
        )
        .await;
    });

    let (sender, mut receiver) = mpsc::channel(32);
    let drive = tokio::spawn(async move {
        drive_protocol(
            client_read,
            client_write,
            request(),
            client(),
            Arc::new(AcceptOnce),
            &sender,
        )
        .await
    });
    let mut events = Vec::new();
    while let Some(event) = receiver.recv().await {
        events.push(event.unwrap());
    }
    drive.await.unwrap().unwrap();
    server.await.unwrap();
    let approval_index = events
        .iter()
        .position(|event| matches!(event, AgentEvent::ApprovalRequested { .. }))
        .unwrap();
    let delta_index = events
        .iter()
        .position(|event| {
            matches!(
                event,
                AgentEvent::MessageDelta { delta, .. } if delta == "not blocked"
            )
        })
        .unwrap();
    assert!(approval_index < delta_index);
    assert!(matches!(
        events.last(),
        Some(AgentEvent::Completed {
            status: AgentRunStatus::Completed,
            ..
        })
    ));
}

#[tokio::test]
async fn duplicate_ids_receive_at_most_one_response_with_immediate_approvals() {
    let (client_io, server_io) = tokio::io::duplex(32 * 1024);
    let (client_read, client_write) = split(client_io);
    let (server_read, mut server_write) = split(server_io);
    let server = tokio::spawn(async move {
        let mut lines = BufReader::new(server_read).lines();
        read_json(&mut lines).await;
        write_json(&mut server_write, json!({"id":0,"result":{}})).await;
        read_json(&mut lines).await;
        read_json(&mut lines).await;
        write_json(
            &mut server_write,
            json!({"id":1,"result":{"thread":{"id":"thr-1"}}}),
        )
        .await;
        read_json(&mut lines).await;
        write_json(
            &mut server_write,
            json!({"id":2,"result":{"turn":{"id":"turn-1"}}}),
        )
        .await;
        let answered = json!({"id":99,"method":"item/commandExecution/requestApproval","params":{"threadId":"thr-1","turnId":"turn-1","itemId":"cmd-99","command":"git status","cwd":"/workspace"}});
        write_json(&mut server_write, answered.clone()).await;
        let response = read_json(&mut lines).await;
        assert_eq!(response["id"], 99);
        assert_eq!(response["result"]["decision"], "accept");
        write_json(&mut server_write, answered).await;
        assert!(
            tokio::time::timeout(Duration::from_millis(50), lines.next_line())
                .await
                .is_err(),
            "an answered request must not receive a second response"
        );

        let pending = json!({"id":100,"method":"item/commandExecution/requestApproval","params":{"threadId":"thr-1","turnId":"turn-1","itemId":"cmd-100","command":"git status","cwd":"/workspace"}});
        write_json(&mut server_write, pending.clone()).await;
        write_json(&mut server_write, pending).await;
        let response = read_json(&mut lines).await;
        assert_eq!(response["id"], 100);
        assert_eq!(response["error"]["code"], -32600);
        assert!(
            tokio::time::timeout(Duration::from_millis(50), lines.next_line())
                .await
                .is_err(),
            "a duplicate pending request must receive exactly one response"
        );
        write_json(
            &mut server_write,
            json!({"method":"turn/completed","params":{"turn":{"id":"turn-1","status":"completed"}}}),
        )
        .await;
    });

    let (sender, mut receiver) = mpsc::channel(32);
    let drive = tokio::spawn(async move {
        drive_protocol(
            client_read,
            client_write,
            request(),
            client(),
            Arc::new(AcceptOnce),
            &sender,
        )
        .await
    });
    let mut events = Vec::new();
    while let Some(event) = receiver.recv().await {
        events.push(event.unwrap());
    }
    drive.await.unwrap().unwrap();
    server.await.unwrap();
    assert_eq!(
        events
            .iter()
            .filter(|event| matches!(event, AgentEvent::ApprovalRequested { .. }))
            .count(),
        2
    );
    assert!(events.iter().any(|event| matches!(
        event,
        AgentEvent::AdapterWarning { code: Some(code), .. }
            if code == "CODEX_SERVER_REQUEST_DUPLICATE"
    )));
    assert!(matches!(
        events.last(),
        Some(AgentEvent::Completed {
            status: AgentRunStatus::Completed,
            ..
        })
    ));
}

#[tokio::test]
async fn cancellation_interrupts_a_turn_while_approval_is_pending() {
    let (client_io, server_io) = tokio::io::duplex(32 * 1024);
    let (client_read, client_write) = split(client_io);
    let (server_read, mut server_write) = split(server_io);
    let server = tokio::spawn(async move {
        let mut lines = BufReader::new(server_read).lines();
        read_json(&mut lines).await;
        write_json(&mut server_write, json!({"id":0,"result":{}})).await;
        read_json(&mut lines).await;
        read_json(&mut lines).await;
        write_json(
            &mut server_write,
            json!({"id":1,"result":{"thread":{"id":"thr-1"}}}),
        )
        .await;
        read_json(&mut lines).await;
        write_json(
            &mut server_write,
            json!({"id":2,"result":{"turn":{"id":"turn-1"}}}),
        )
        .await;
        write_json(
            &mut server_write,
            json!({"id":99,"method":"item/commandExecution/requestApproval","params":{"threadId":"thr-1","turnId":"turn-1","itemId":"cmd-1","command":"git status","cwd":"/workspace"}}),
        )
        .await;
        let interrupt = read_json(&mut lines).await;
        assert_eq!(interrupt["method"], "turn/interrupt");
        assert_eq!(interrupt["params"]["turnId"], "turn-1");
    });

    let cancellation = CancellationToken::new();
    let mut run_request = request();
    run_request.cancellation = cancellation.clone();
    let (sender, mut receiver) = mpsc::channel(32);
    let drive = tokio::spawn(async move {
        drive_protocol(
            client_read,
            client_write,
            run_request,
            client(),
            Arc::new(PendingApproval::default()),
            &sender,
        )
        .await
    });
    while let Some(event) = receiver.recv().await {
        if matches!(event.unwrap(), AgentEvent::ApprovalRequested { .. }) {
            cancellation.cancel();
            break;
        }
    }
    let error = drive.await.unwrap().unwrap_err();
    server.await.unwrap();
    assert_eq!(error.kind, ait_agent_adapters::AdapterErrorKind::Cancelled);
}
