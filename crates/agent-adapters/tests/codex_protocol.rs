//! In-memory protocol conformance tests for the Codex app-server adapter.

use std::{path::PathBuf, sync::Arc};

use ait_agent_adapters::{
    AgentEvent, AgentRunRequest, AgentRunStatus, ApprovalDecision, ApprovalHandler, ApprovalPolicy,
    ApprovalRequest, SandboxMode,
    codex::{ClientInfo, drive_protocol},
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
        prompt: "Inspect the project".into(),
        cwd: PathBuf::from("/workspace"),
        resume_thread_id: None,
        sandbox: SandboxMode::WorkspaceWrite,
        approval_policy: ApprovalPolicy::OnRequest,
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
        write_json(
            &mut server_write,
            json!({"id": 1, "result": {"thread": {"id": "thr-1"}}}),
        )
        .await;
        let turn = read_json(&mut lines).await;
        assert_eq!(turn["method"], "turn/start");
        assert_eq!(turn["params"]["input"][0]["text"], "Inspect the project");
        assert_eq!(turn["params"]["effort"], "high");
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
