//! Offline equivalents of Paseo's app-server transport and JSONL framing contracts.

use std::os::unix::fs::PermissionsExt;

use super::*;

struct Fixture {
    _root: tempfile::TempDir,
    transport: Transport,
}

impl Fixture {
    fn new(body: &str) -> Self {
        let root = tempfile::tempdir().unwrap();
        let program = root.path().join("fake-app-server");
        let source = format!(
            "#!/usr/bin/env python3\nimport json, os, sys, time\n\
             def receive(): return json.loads(sys.stdin.readline())\n\
             def send(value): print(json.dumps(value), flush=True)\n{body}\n"
        );
        std::fs::write(&program, source).unwrap();
        std::fs::set_permissions(&program, std::fs::Permissions::from_mode(0o700)).unwrap();
        let transport = Transport::spawn(
            &program,
            root.path().to_str().unwrap(),
            Duration::from_secs(3),
        )
        .unwrap();
        Self {
            _root: root,
            transport,
        }
    }

    async fn request(&mut self) -> Result<Value, AgentSessionError> {
        self.transport.request("model/list", json!({})).await
    }

    async fn event(&mut self) -> Result<Value, AgentSessionError> {
        tokio::time::timeout(Duration::from_secs(3), async {
            loop {
                if let Some(event) = self.transport.poll()? {
                    return Ok(event);
                }
                tokio::task::yield_now().await;
            }
        })
        .await
        .unwrap()
    }
}

#[tokio::test]
async fn initialize_sends_experimental_capabilities_then_initialized_notification() {
    let mut fixture = Fixture::new(
        "first = receive()\nsend({'id': first['id'], 'result': {}})\n\
         initialized = receive()\nnext_request = receive()\n\
         send({'id': next_request['id'], 'result': [first, initialized]})",
    );
    fixture.transport.initialize().await.unwrap();
    let observed = fixture.request().await.unwrap();
    assert_eq!(observed[0]["method"], "initialize");
    assert_eq!(observed[0]["params"]["clientInfo"]["name"], "ait-server");
    assert_eq!(
        observed[0]["params"]["capabilities"]["experimentalApi"],
        true
    );
    assert_eq!(observed[1], json!({"method":"initialized","params":{}}));
    fixture.transport.close().await.unwrap();
}

#[tokio::test]
async fn bytewise_utf8_and_crlf_output_preserves_the_response() {
    // Pipes can coalesce writes; this checks a bytewise producer, not fixed reader boundaries.
    let mut fixture = Fixture::new(
        "request = receive()\n\
         response = (json.dumps({'id': request['id'], 'result': '你好\\n世界'}, ensure_ascii=False) + '\\r\\n').encode()\n\
         for byte in response: os.write(sys.stdout.fileno(), bytes([byte]))",
    );
    assert_eq!(fixture.request().await.unwrap(), "你好\n世界");
    fixture.transport.close().await.unwrap();
}

#[tokio::test]
async fn supported_notifications_before_acknowledgment_keep_source_order() {
    let mut fixture = Fixture::new(
        "request = receive()\n\
         send({'method': 'item/agentMessage/delta', 'params': {'delta': 'first'}})\n\
         send({'method': 'item/completed', 'params': {'id': 'second'}})\n\
         send({'id': request['id'], 'result': {'accepted': True}})\n\
         send({'method': 'turn/completed', 'params': {'id': 'third'}})",
    );
    assert_eq!(fixture.request().await.unwrap()["accepted"], true);
    assert_eq!(fixture.event().await.unwrap()["params"]["delta"], "first");
    assert_eq!(fixture.event().await.unwrap()["params"]["id"], "second");
    assert_eq!(fixture.event().await.unwrap()["params"]["id"], "third");
    fixture.transport.close().await.unwrap();
}

#[tokio::test]
async fn legacy_mirror_and_unrelated_notifications_do_not_duplicate_output() {
    let mut fixture = Fixture::new(
        "request = receive()\n\
         send({'method': 'codex/event/agent_message_delta', 'params': {'delta': 'duplicate'}})\n\
         send({'method': 'thread/tokenUsage/updated', 'params': {}})\n\
         send({'method': 'item/agentMessage/delta', 'params': {'delta': 'actual'}})\n\
         send({'id': request['id'], 'result': {}})\nreceive()",
    );
    fixture.request().await.unwrap();
    assert_eq!(
        fixture.event().await.unwrap()["method"],
        "thread/tokenUsage/updated"
    );
    assert_eq!(fixture.event().await.unwrap()["params"]["delta"], "actual");
    assert_eq!(fixture.transport.poll().unwrap(), None);
    fixture.transport.close().await.unwrap();
}

#[tokio::test]
async fn native_approval_before_response_survives_and_can_be_answered() {
    let mut fixture = Fixture::new(
        "request = receive()\n\
         send({'id': 'permission-id', 'method': 'item/commandExecution/requestApproval', 'params': {'command': 'pwd'}})\n\
         send({'id': request['id'], 'result': {}})\n\
         answer = receive()\nsend({'method': 'item/completed', 'params': answer})",
    );
    fixture.request().await.unwrap();
    let approval = fixture.event().await.unwrap();
    assert_eq!(approval["id"], "permission-id");
    fixture
        .transport
        .respond(&approval["id"], json!({"decision":"decline"}))
        .await
        .unwrap();
    let answered = fixture.event().await.unwrap();
    assert_eq!(
        answered["params"],
        json!({"id":"permission-id","result":{"decision":"decline"}})
    );
    fixture.transport.close().await.unwrap();
}

#[tokio::test]
async fn rejection_keeps_transport_usable_and_increments_request_ids() {
    let mut fixture = Fixture::new(
        "first = receive()\nsend({'id': first['id'], 'error': {'code': -32602}})\n\
         second = receive()\nsend({'id': second['id'], 'result': second})",
    );
    assert_eq!(fixture.request().await, Err(AgentSessionError::Rejected));
    assert!(!fixture.transport.closed);
    assert_eq!(fixture.request().await.unwrap()["id"], 2);
    fixture.transport.close().await.unwrap();
}

#[tokio::test]
async fn definite_steer_rejection_preserves_queued_completion_and_transport() {
    let mut fixture = Fixture::new(
        "request = receive()\n\
         send({'method': 'turn/completed', 'params': {'turnId': 'old'}})\n\
         send({'id': request['id'], 'error': {'code': -32600, 'message': 'no active turn to steer'}})\n\
         next_request = receive()\nsend({'id': next_request['id'], 'result': {}})",
    );
    assert_eq!(
        fixture.transport.request("turn/steer", json!({})).await,
        Err(AgentSessionError::Rejected)
    );
    assert_eq!(fixture.event().await.unwrap()["params"]["turnId"], "old");
    fixture.request().await.unwrap();
    fixture.transport.close().await.unwrap();
}

#[tokio::test]
async fn ambiguous_steer_error_closes_the_transport_without_retry() {
    let mut fixture = Fixture::new(
        "request = receive()\nsend({'id': request['id'], 'error': {'code': -32603}})\nreceive()",
    );
    assert_eq!(
        fixture.transport.request("turn/steer", json!({})).await,
        Err(AgentSessionError::Failed)
    );
    assert!(fixture.transport.closed);
    assert_eq!(fixture.request().await, Err(AgentSessionError::Failed));
    assert_eq!(fixture.transport.sequence, 1);
}

#[tokio::test]
async fn malformed_stdout_fails_closed_without_accepting_a_later_response() {
    // Rust intentionally fails closed rather than applying Paseo's stdout-noise recovery.
    let mut fixture = Fixture::new(
        "request = receive()\nprint('localized diagnostic', flush=True)\n\
         send({'id': request['id'], 'result': {'mustNotAccept': True}})",
    );
    assert_eq!(fixture.request().await, Err(AgentSessionError::Failed));
    assert!(fixture.transport.closed);
}

#[tokio::test]
async fn mismatched_response_id_fails_closed() {
    let mut fixture =
        Fixture::new("request = receive()\nsend({'id': request['id'] + 1, 'result': {}})");
    assert_eq!(fixture.request().await, Err(AgentSessionError::Failed));
    assert!(fixture.transport.closed);
}

#[tokio::test]
async fn missing_response_result_fails_closed() {
    let mut fixture = Fixture::new("request = receive()\nsend({'id': request['id']})");
    assert_eq!(fixture.request().await, Err(AgentSessionError::Failed));
    assert!(fixture.transport.closed);
}

#[tokio::test]
async fn unterminated_frame_at_eof_does_not_complete_the_request() {
    let mut fixture = Fixture::new(
        "request = receive()\nsys.stdout.write(json.dumps({'id': request['id'], 'result': {}}))\nsys.stdout.flush()",
    );
    assert_eq!(fixture.request().await, Err(AgentSessionError::Failed));
}

#[tokio::test]
async fn oversized_incoming_frame_rejects_instead_of_consuming_unbounded_stdout() {
    let mut fixture = Fixture::new(
        "request = receive()\nsend({'id': request['id'], 'result': 'x' * (2 * 1024 * 1024)})",
    );
    assert_eq!(fixture.request().await, Err(AgentSessionError::Failed));
    assert!(fixture.transport.closed);
}

#[tokio::test]
async fn oversized_outgoing_request_is_not_written_and_closes_transport() {
    let mut fixture =
        Fixture::new("first = receive()\nsend({'id': first['id'], 'result': first})\nreceive()");
    let oversized = json!({"id":999,"method":"turn/start","params":{
        "text":"x".repeat(MAX_FRAME)
    }});
    assert_eq!(
        write(&fixture.transport.input, &oversized).await,
        Err(AgentSessionError::Failed)
    );
    // A normal round trip proves rejected bytes did not reach the peer before teardown.
    assert_eq!(
        fixture.request().await.unwrap(),
        json!({"id":1,"method":"model/list","params":{}})
    );
    assert_eq!(
        fixture
            .transport
            .request("turn/start", oversized["params"].clone())
            .await,
        Err(AgentSessionError::Failed)
    );
    assert!(fixture.transport.closed);
}

#[tokio::test]
async fn notification_queue_accepts_its_exact_capacity_before_acknowledgment() {
    let mut fixture = Fixture::new(
        "request = receive()\n\
         for index in range(128): send({'method': 'item/agentMessage/delta', 'params': {'index': index}})\n\
         send({'id': request['id'], 'result': {}})",
    );
    fixture.request().await.unwrap();
    for expected in 0..MAX_EVENTS {
        assert_eq!(fixture.event().await.unwrap()["params"]["index"], expected);
    }
    fixture.transport.close().await.unwrap();
}

#[tokio::test]
async fn notification_queue_overflow_fails_instead_of_dropping_output() {
    let mut fixture = Fixture::new(
        "request = receive()\n\
         for index in range(129): send({'method': 'item/agentMessage/delta', 'params': {'index': index}})\n\
         send({'id': request['id'], 'result': {}})",
    );
    assert_eq!(fixture.request().await, Err(AgentSessionError::Failed));
    assert!(fixture.transport.closed);
}

#[tokio::test]
async fn exited_child_fails_pending_request_and_is_reaped() {
    let mut fixture = Fixture::new("receive()\nsys.exit(7)");
    assert_eq!(fixture.request().await, Err(AgentSessionError::Failed));
    assert!(fixture.transport.child.id().is_none());
    fixture.transport.close().await.unwrap();
}

#[tokio::test]
async fn request_timeout_closes_and_reaps_unresponsive_child() {
    let mut fixture = Fixture::new("receive()\ntime.sleep(60)");
    fixture.transport.deadline = Duration::from_millis(100);
    assert_eq!(fixture.request().await, Err(AgentSessionError::Failed));
    assert!(fixture.transport.child.id().is_none());
}

#[tokio::test]
async fn close_is_idempotent_and_subsequent_requests_and_responses_fail_immediately() {
    let mut fixture = Fixture::new("receive()");
    fixture.transport.close().await.unwrap();
    fixture.transport.close().await.unwrap();
    assert_eq!(fixture.request().await, Err(AgentSessionError::Failed));
    assert_eq!(
        fixture.transport.respond(&json!(7), json!({})).await,
        Err(AgentSessionError::Failed)
    );
    assert_eq!(fixture.transport.sequence, 0);
    assert!(fixture.transport.child.id().is_none());
}

#[tokio::test]
async fn unsupported_native_interaction_is_rejected_and_terminates_the_stream() {
    let mut fixture = Fixture::new(
        "send({'id': 7, 'method': 'item/permissions/requestApproval', 'params': {}})\n\
         response = receive()\n\
         assert response == {'id': 7, 'error': {'code': -32601, 'message': 'Provider interaction is not supported'}}",
    );
    assert_eq!(
        fixture.event().await.unwrap()["method"],
        "server/unsupportedRequest"
    );
    let exit = tokio::time::timeout(Duration::from_secs(3), fixture.transport.child.wait())
        .await
        .unwrap()
        .unwrap();
    assert!(exit.success());
    assert_eq!(fixture.transport.poll(), Err(AgentSessionError::Failed));
    fixture.transport.close().await.unwrap();
}
