use super::*;
use ait_contracts::worker::{MAX_FRAME_BYTES, StoreRequest};
#[test]
fn escaped_credential_values_cannot_bypass_the_pipe_guard() {
    let secret = "secret\"with\nJSON\\escapes";
    let payload = Payload::Request {
        request_id: 1,
        operation_id: secret.into(),
        request: Box::new(StoreRequest::LoadRun),
    };
    assert!(leaks(&payload, Some(secret)));
    assert_eq!(
        format!(
            "{:?}",
            ait_contracts::worker::CredentialGrant(secret.into())
        ),
        "[REDACTED]"
    );
}
#[tokio::test]
async fn stalled_consumer_and_credential_echo_fail_boundedly() {
    let (input, _held_input) = tokio::io::duplex(1);
    let (output, _held_output) = tokio::io::duplex(1);
    let lease = Lease {
        scope_id: "r".into(),
        worker_instance_id: "w".into(),
        lease_epoch: 1,
    };
    let pipe = Connection::start(
        Reader::new(input, MAX_FRAME_BYTES),
        Writer::new(output, MAX_FRAME_BYTES),
        lease,
        Some("private-key".into()),
    );
    let result = pipe
        .send(Payload::Request {
            request_id: 1,
            operation_id: "private-key".into(),
            request: Box::new(StoreRequest::LoadRun),
        })
        .await;
    assert_eq!(result, Err(ProtocolError::InvalidFrame));
    assert!(!format!("{result:?}").contains("private-key"));
    pipe.close().await;
    let (input, _held_input) = tokio::io::duplex(1);
    let (output, _held_output) = tokio::io::duplex(1);
    let pipe = Connection::start(
        Reader::new(input, MAX_FRAME_BYTES),
        Writer::new(output, MAX_FRAME_BYTES),
        Lease {
            scope_id: "r".into(),
            worker_instance_id: "w".into(),
            lease_epoch: 1,
        },
        None,
    );
    assert!(
        tokio::time::timeout(Duration::from_secs(3), pipe.send(Payload::Heartbeat))
            .await
            .unwrap()
            .is_err()
    );
    pipe.close().await;
}
