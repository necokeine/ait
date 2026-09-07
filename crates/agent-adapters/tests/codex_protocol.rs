//! In-memory protocol conformance tests for the Codex app-server adapter.

use std::{path::PathBuf, sync::Arc, time::Duration};

use ait_agent_adapters::{
    AgentEvent, AgentRunRequest, AgentRunStatus, ApprovalDecision, ApprovalHandler, ApprovalPolicy,
    ApprovalRequest, SandboxMode,
    codex::{
        ClientInfo, drive_model_list_protocol, drive_protocol, drive_protocol_with_interrupt_grace,
    },
};
use async_trait::async_trait;
use serde_json::{Value, json};
use tokio::{
    io::{AsyncBufReadExt, AsyncWriteExt, BufReader, split},
    sync::mpsc,
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
async fn cancellation_waits_for_the_target_turn_to_report_interrupted() {
    let (client_io, server_io) = tokio::io::duplex(32 * 1024);
    let (client_read, client_write) = split(client_io);
    let (server_read, mut server_write) = split(server_io);
    let (turn_started, start_cancellation) = tokio::sync::oneshot::channel();
    let server = tokio::spawn(async move {
        let mut lines = BufReader::new(server_read).lines();
        read_json(&mut lines).await;
        write_json(&mut server_write, json!({"id":0,"result":{}})).await;
        read_json(&mut lines).await;
        read_json(&mut lines).await;
        write_json(
            &mut server_write,
            json!({"id":1,"result":{"thread":{"id":"thread-cancel"}}}),
        )
        .await;
        read_json(&mut lines).await;
        write_json(
            &mut server_write,
            json!({"id":2,"result":{"turn":{"id":"turn-cancel"}}}),
        )
        .await;
        write_json(
            &mut server_write,
            json!({"method":"item/started","params":{"item":{"id":"command-1","type":"commandExecution","status":"inProgress","command":"cargo test"}}}),
        )
        .await;
        turn_started.send(()).unwrap();
        let interrupt = read_json(&mut lines).await;
        assert_eq!(interrupt["method"], "turn/interrupt");
        assert_eq!(interrupt["params"]["turnId"], "turn-cancel");
        tokio::time::sleep(Duration::from_millis(50)).await;
        write_json(&mut server_write, json!({"id":3,"result":{}})).await;
        write_json(
            &mut server_write,
            json!({"method":"turn/completed","params":{"turn":{"id":"turn-cancel","status":"interrupted"}}}),
        )
        .await;
    });
    let request = request();
    let cancellation = request.cancellation.clone();
    let (sender, mut receiver) = mpsc::channel(32);
    let drive = tokio::spawn(async move {
        drive_protocol_with_interrupt_grace(
            client_read,
            client_write,
            request,
            client(),
            Arc::new(ait_agent_adapters::DenyAllApprovals),
            &sender,
            Duration::from_secs(1),
        )
        .await
    });
    start_cancellation.await.unwrap();
    let started = tokio::time::Instant::now();
    cancellation.cancel();
    let error = drive.await.unwrap().unwrap_err();
    assert_eq!(error.kind, ait_agent_adapters::AdapterErrorKind::Cancelled);
    assert!(started.elapsed() >= Duration::from_millis(40));
    server.await.unwrap();
    let mut interrupted = false;
    let mut saw_running_command = false;
    while let Ok(event) = receiver.try_recv() {
        match event.unwrap() {
            AgentEvent::Completed {
                status: AgentRunStatus::Interrupted,
                ..
            } => interrupted = true,
            AgentEvent::ItemStarted { item }
                if item.get("type").and_then(Value::as_str) == Some("commandExecution") =>
            {
                saw_running_command = true;
            }
            _ => {}
        }
    }
    assert!(interrupted);
    assert!(saw_running_command);
}

#[tokio::test]
async fn missing_interrupt_ack_hits_the_grace_deadline() {
    let (client_io, server_io) = tokio::io::duplex(32 * 1024);
    let (client_read, client_write) = split(client_io);
    let (server_read, mut server_write) = split(server_io);
    let (turn_started, start_cancellation) = tokio::sync::oneshot::channel();
    let server = tokio::spawn(async move {
        let mut lines = BufReader::new(server_read).lines();
        read_json(&mut lines).await;
        write_json(&mut server_write, json!({"id":0,"result":{}})).await;
        read_json(&mut lines).await;
        read_json(&mut lines).await;
        write_json(
            &mut server_write,
            json!({"id":1,"result":{"thread":{"id":"thread-timeout"}}}),
        )
        .await;
        read_json(&mut lines).await;
        write_json(
            &mut server_write,
            json!({"id":2,"result":{"turn":{"id":"turn-timeout"}}}),
        )
        .await;
        turn_started.send(()).unwrap();
        let interrupt = read_json(&mut lines).await;
        assert_eq!(interrupt["method"], "turn/interrupt");
        std::future::pending::<()>().await;
    });
    let request = request();
    let cancellation = request.cancellation.clone();
    let (sender, _receiver) = mpsc::channel(32);
    let drive = tokio::spawn(async move {
        drive_protocol_with_interrupt_grace(
            client_read,
            client_write,
            request,
            client(),
            Arc::new(ait_agent_adapters::DenyAllApprovals),
            &sender,
            Duration::from_millis(40),
        )
        .await
    });
    start_cancellation.await.unwrap();
    cancellation.cancel();
    let error = tokio::time::timeout(Duration::from_secs(1), drive)
        .await
        .unwrap()
        .unwrap()
        .unwrap_err();
    assert_eq!(error.kind, ait_agent_adapters::AdapterErrorKind::Cancelled);
    server.abort();
}
