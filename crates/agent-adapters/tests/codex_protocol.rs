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
            json!({"id":99,"method":"item/commandExecution/requestApproval","params":{"threadId":"thr-1","turnId":"turn-1","itemId":"cmd-1","reason":"needs permission"}}),
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
            Arc::new(AcceptOnce),
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
            json!({"method":"item/agentMessage/delta","params":{"itemId":"answer","delta":"continued"}}),
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
            json!({"method":"item/agentMessage/delta","params":{"itemId":"answer","delta":"still running"}}),
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
    async fn decide(&self, _request: &ApprovalRequest) -> ApprovalDecision {
        match self.0.fetch_add(1, Ordering::Relaxed) {
            0 => ApprovalDecision::Decline,
            _ => ApprovalDecision::Cancel,
        }
    }
}

#[tokio::test]
async fn preserves_decline_and_cancel_approval_decisions() {
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
        for (id, method, expected) in [
            (71, "item/commandExecution/requestApproval", "decline"),
            (72, "item/fileChange/requestApproval", "cancel"),
        ] {
            write_json(
                &mut server_write,
                json!({"id":id,"method":method,"params":{"itemId":format!("item-{id}")}}),
            )
            .await;
            let response = read_json(&mut lines).await;
            assert_eq!(response["id"], id);
            assert_eq!(response["result"]["decision"], expected);
        }
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
    drive.await.unwrap().unwrap();
    server.await.unwrap();
    assert_eq!(approvals, 2);
}

#[derive(Debug, Default)]
struct PendingApproval {
    release: Notify,
}

#[async_trait]
impl ApprovalHandler for PendingApproval {
    async fn decide(&self, _request: &ApprovalRequest) -> ApprovalDecision {
        self.release.notified().await;
        ApprovalDecision::Accept
    }
}

#[tokio::test]
async fn dispatches_events_while_an_approval_is_pending_and_honors_resolution() {
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
            json!({"id":99,"method":"item/commandExecution/requestApproval","params":{"itemId":"cmd-1"}}),
        )
        .await;
        write_json(
            &mut server_write,
            json!({"method":"item/agentMessage/delta","params":{"itemId":"answer","delta":"not blocked"}}),
        )
        .await;
        write_json(
            &mut server_write,
            json!({"method":"serverRequest/resolved","params":{"threadId":"thr-1","requestId":99}}),
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
            Arc::new(PendingApproval::default()),
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
async fn rejects_duplicate_request_ids_without_reinvoking_approval() {
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
        let approval = json!({"id":99,"method":"item/commandExecution/requestApproval","params":{"itemId":"cmd-1"}});
        write_json(&mut server_write, approval.clone()).await;
        write_json(&mut server_write, approval).await;
        let response = read_json(&mut lines).await;
        assert_eq!(response["id"], 99);
        assert_eq!(response["error"]["code"], -32600);
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
            Arc::new(PendingApproval::default()),
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
        1
    );
    assert!(events.iter().any(|event| matches!(
        event,
        AgentEvent::AdapterWarning { code: Some(code), .. }
            if code == "CODEX_SERVER_REQUEST_DUPLICATE"
    )));
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
            json!({"id":99,"method":"item/commandExecution/requestApproval","params":{"itemId":"cmd-1"}}),
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
