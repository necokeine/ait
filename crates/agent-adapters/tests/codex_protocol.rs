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

async fn respond_to_owner_policy_read<R, W>(
    lines: &mut tokio::io::Lines<R>,
    writer: &mut W,
    shell_environment_policy: Value,
) where
    R: tokio::io::AsyncBufRead + Unpin,
    W: tokio::io::AsyncWrite + Unpin,
{
    let request = read_json(lines).await;
    assert_eq!(request["method"], "config/read");
    assert_eq!(request["id"], -1);
    assert_eq!(request["params"]["cwd"], "/workspace");
    write_json(
        writer,
        json!({
            "id": -1,
            "result": {
                "config": {"shell_environment_policy": shell_environment_policy},
                "origins": {}
            }
        }),
    )
    .await;
}

fn assert_legacy_owner_config(thread: &Value) {
    let owner_config = &thread["params"]["config"];
    assert!(
        owner_config["shell_environment_policy.set.AIT_CODEX_PROCESS_OWNER"]
            .as_str()
            .is_some_and(|marker| !marker.is_empty())
    );
    assert_eq!(
        owner_config["shell_environment_policy.include_only"],
        json!(["PATH", "AIT_CODEX_PROCESS_OWNER"])
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
#[allow(
    clippy::too_many_lines,
    reason = "the protocol lifecycle and ownership handshake are one conformance scenario"
)]
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
        respond_to_owner_policy_read(
            &mut lines,
            &mut server_write,
            json!({
                "inherit":"none", "include_only":["PATH"]
            }),
        )
        .await;
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
        assert_legacy_owner_config(&thread);
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

struct BlockingApproval {
    entered: tokio::sync::Semaphore,
}

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
        respond_to_owner_policy_read(
            &mut lines,
            &mut server_write,
            json!({"filters":{"PATH":"include","SECRET_*":"exclude"}}),
        )
        .await;
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
        assert_eq!(
            thread["params"]["config"]["shell_environment_policy.filters.AIT_CODEX_PROCESS_OWNER"],
            "include"
        );
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

#[async_trait]
impl ApprovalHandler for BlockingApproval {
    async fn decide(&self, _request: &ApprovalRequest) -> ApprovalDecision {
        self.entered.add_permits(1);
        std::future::pending().await
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
        respond_to_owner_policy_read(&mut lines, &mut server_write, json!({})).await;
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
        respond_to_owner_policy_read(&mut lines, &mut server_write, json!({})).await;
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
        respond_to_owner_policy_read(&mut lines, &mut server_write, json!({})).await;
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

#[tokio::test]
async fn interrupt_deadline_bounds_a_blocked_approval_handler() {
    let (client_io, server_io) = tokio::io::duplex(32 * 1024);
    let (client_read, client_write) = split(client_io);
    let (server_read, mut server_write) = split(server_io);
    let server = tokio::spawn(async move {
        let mut lines = BufReader::new(server_read).lines();
        read_json(&mut lines).await;
        write_json(&mut server_write, json!({"id":0,"result":{}})).await;
        read_json(&mut lines).await;
        respond_to_owner_policy_read(&mut lines, &mut server_write, json!({})).await;
        read_json(&mut lines).await;
        write_json(
            &mut server_write,
            json!({"id":1,"result":{"thread":{"id":"thread-approval"}}}),
        )
        .await;
        read_json(&mut lines).await;
        write_json(
            &mut server_write,
            json!({"id":2,"result":{"turn":{"id":"turn-approval"}}}),
        )
        .await;
        write_json(
            &mut server_write,
            json!({"id":90,"method":"item/commandExecution/requestApproval","params":{"turnId":"turn-approval"}}),
        )
        .await;
        let interrupt = read_json(&mut lines).await;
        assert_eq!(interrupt["method"], "turn/interrupt");
        write_json(&mut server_write, json!({"id":3,"result":{}})).await;
        write_json(
            &mut server_write,
            json!({"id":91,"method":"item/commandExecution/requestApproval","params":{"turnId":"turn-approval"}}),
        )
        .await;
        std::future::pending::<()>().await;
    });
    let request = request();
    let cancellation = request.cancellation.clone();
    let approvals = Arc::new(BlockingApproval {
        entered: tokio::sync::Semaphore::new(0),
    });
    let (sender, mut receiver) = mpsc::channel(32);
    let drive = tokio::spawn({
        let approvals = approvals.clone();
        async move {
            drive_protocol_with_interrupt_grace(
                client_read,
                client_write,
                request,
                client(),
                approvals,
                &sender,
                Duration::from_millis(40),
            )
            .await
        }
    });
    let drain = tokio::spawn(async move { while receiver.recv().await.is_some() {} });
    tokio::time::timeout(Duration::from_secs(1), approvals.entered.acquire())
        .await
        .unwrap()
        .unwrap()
        .forget();
    let started = tokio::time::Instant::now();
    cancellation.cancel();
    let error = tokio::time::timeout(Duration::from_millis(500), drive)
        .await
        .expect("blocked approval handling must obey the interrupt deadline")
        .unwrap()
        .unwrap_err();
    assert_eq!(error.kind, ait_agent_adapters::AdapterErrorKind::Cancelled);
    assert!(started.elapsed() >= Duration::from_millis(30));
    server.abort();
    drain.await.unwrap();
}

#[tokio::test]
async fn interrupt_deadline_bounds_full_event_channel_backpressure() {
    let (client_io, server_io) = tokio::io::duplex(32 * 1024);
    let (client_read, client_write) = split(client_io);
    let (server_read, mut server_write) = split(server_io);
    let (turn_exists, start_cancellation) = tokio::sync::oneshot::channel();
    let server = tokio::spawn(async move {
        let mut lines = BufReader::new(server_read).lines();
        read_json(&mut lines).await;
        write_json(&mut server_write, json!({"id":0,"result":{}})).await;
        read_json(&mut lines).await;
        respond_to_owner_policy_read(&mut lines, &mut server_write, json!({})).await;
        read_json(&mut lines).await;
        write_json(
            &mut server_write,
            json!({"id":1,"result":{"thread":{"id":"thread-backpressure"}}}),
        )
        .await;
        read_json(&mut lines).await;
        write_json(
            &mut server_write,
            json!({"id":2,"result":{"turn":{"id":"turn-backpressure"}}}),
        )
        .await;
        turn_exists.send(()).unwrap();
        let interrupt = read_json(&mut lines).await;
        assert_eq!(interrupt["method"], "turn/interrupt");
        write_json(&mut server_write, json!({"id":3,"result":{}})).await;
        write_json(
            &mut server_write,
            json!({"method":"turn/completed","params":{"turn":{"id":"turn-backpressure","status":"interrupted"}}}),
        )
        .await;
    });
    let request = request();
    let cancellation = request.cancellation.clone();
    // ThreadStarted fills the only slot; TurnStarted then blocks until cancel.
    let (sender, _receiver) = mpsc::channel(1);
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
    tokio::time::sleep(Duration::from_millis(20)).await;
    let started = tokio::time::Instant::now();
    cancellation.cancel();
    let error = tokio::time::timeout(Duration::from_millis(500), drive)
        .await
        .expect("event backpressure must obey the interrupt deadline")
        .unwrap()
        .unwrap_err();
    assert_eq!(error.kind, ait_agent_adapters::AdapterErrorKind::Cancelled);
    assert!(started.elapsed() >= Duration::from_millis(30));
    server.await.unwrap();
}
