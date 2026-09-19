//! Ordered history chunks remain bounded and fenced independently of their payload.
use super::*;
use ait_contracts::worker::codex::CHUNK_BYTES;
use serde_json::json;

fn server() -> (Server, mpsc::Receiver<Reply>, mpsc::Sender<Action>) {
    let (actions, receiver) = mpsc::channel(1);
    let (replies, results) = mpsc::channel(2);
    (
        Server {
            project_execution: None,
            lease: Lease {
                project_owner: None,
                scope_id: "auxiliary".into(),
                worker_instance_id: "worker".into(),
                lease_epoch: 1,
            },
            request_id: None,
            assembly: Mutex::default(),
            actions: AsyncMutex::new(receiver),
            replies,
            progress: Arc::default(),
            approvals: None,
            cancel: CancellationToken::new(),
        },
        results,
        actions,
    )
}
async fn chunk(
    server: &Server,
    offset: usize,
    total: usize,
    bytes: Vec<u8>,
) -> Result<StoreResponse, ProtocolError> {
    server
        .request(
            &server.lease,
            "result",
            StoreRequest::CodexChunk {
                offset,
                total,
                bytes,
            },
        )
        .await
}
#[tokio::test]
async fn multi_frame_results_require_complete_order_and_clean_close() {
    let (server, mut results, _actions) = server();
    let value = json!({"text":"x".repeat(CHUNK_BYTES * 2)});
    let bytes = serde_json::to_vec(&Ok::<_, DomainError>((value.clone(), true))).unwrap();
    for (index, bytes_chunk) in bytes.chunks(CHUNK_BYTES).enumerate() {
        chunk(
            &server,
            index * CHUNK_BYTES,
            bytes.len(),
            bytes_chunk.to_vec(),
        )
        .await
        .unwrap();
    }
    assert_eq!(results.recv().await.unwrap().unwrap(), (value, true));
    assert!(server.finished().await.is_err());
    server
        .request(&server.lease, "close", StoreRequest::CodexClosed)
        .await
        .unwrap();
    server.finished().await.unwrap();
    assert_eq!(
        chunk(&server, 0, 1, vec![0]).await.unwrap_err(),
        ProtocolError::InvalidFrame
    );
}
#[tokio::test]
async fn malformed_offsets_sizes_and_stale_scopes_are_rejected_before_growth() {
    let (server, _results, _actions) = server();
    for (offset, total, bytes) in [
        (1, 1, vec![0]),
        (0, 0, vec![0]),
        (0, MAX_RESULT_BYTES + 1, vec![0]),
        (0, 1, vec![]),
        (0, CHUNK_BYTES + 1, vec![0; CHUNK_BYTES + 1]),
    ] {
        assert_eq!(
            chunk(&server, offset, total, bytes).await.unwrap_err(),
            ProtocolError::InvalidFrame
        );
        assert!(server.assembly.lock().unwrap().bytes.is_empty());
    }
    chunk(&server, 0, 4, vec![b'{']).await.unwrap();
    assert_eq!(
        chunk(&server, 1, 5, vec![b'}']).await.unwrap_err(),
        ProtocolError::InvalidFrame
    );
    assert!(
        server
            .request(&server.lease, "close", StoreRequest::CodexClosed)
            .await
            .is_err()
    );
    let mut stale = server.lease.clone();
    stale.scope_id = "another".into();
    assert_eq!(
        server
            .request(&stale, "next", StoreRequest::CodexNext)
            .await
            .unwrap_err(),
        ProtocolError::StaleWorkerLease
    );
}
#[tokio::test]
async fn cancellation_releases_an_idle_writer_without_a_start_action() {
    let (server, _results, _actions) = server();
    server.cancel.cancel();
    let reply = server
        .request(&server.lease, "next", StoreRequest::CodexNext)
        .await
        .unwrap();
    assert!(matches!(
        reply,
        StoreResponse::CodexAction {
            action: Action::Close
        }
    ));
}
