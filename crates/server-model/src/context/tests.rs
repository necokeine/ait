use serde_json::json;

use super::*;
use crate::outbound::Frame;
use crate::tests::runtime;

fn request() -> Request {
    Request {
        id: "r1".to_owned(),
        method: "test.operation".to_owned(),
        params: json!({"increment":3}),
    }
}

fn decode(message: Frame) -> Value {
    let Frame::Text(text) = message else {
        panic!("expected JSON response")
    };
    serde_json::from_str(&text).unwrap()
}

#[tokio::test]
async fn rpc_runs_the_selected_operation_with_the_owned_request() {
    let runtime = runtime();
    let service = Arc::new(Mutex::new(4_u64));
    let (outbound, mut receiver) = Outbound::new();
    let context = Context {
        request: request(),
        runtime: &runtime,
        outbound: &outbound,
        available_subscriptions: 16,
    };
    context
        .rpc(
            Some(service.clone()),
            ErrorCode::RegistryIo,
            |value, method, params| {
                assert_eq!(method, "test.operation");
                *value += params["increment"].as_u64().unwrap();
                Ok::<_, ErrorCode>(json!({"value":value}))
            },
        )
        .await
        .unwrap();
    assert_eq!(*service.lock().unwrap(), 7);
    let response = decode(receiver.recv().await.unwrap().message);
    assert_eq!(response["request_id"], "r1");
    assert_eq!(response["result"]["value"], 7);
    assert!(receiver.try_recv().is_err());
}

#[tokio::test]
async fn missing_services_and_business_failures_use_one_error_envelope() {
    for (installed, expected) in [(false, "unsupported_capability"), (true, "registry_io")] {
        let runtime = runtime();
        let service = installed.then(|| Arc::new(Mutex::new(())));
        let (outbound, mut receiver) = Outbound::new();
        let context = Context {
            request: request(),
            runtime: &runtime,
            outbound: &outbound,
            available_subscriptions: 16,
        };
        context
            .rpc(service, ErrorCode::RegistryIo, |(), _, _| {
                Err::<Value, _>(ErrorCode::RegistryIo)
            })
            .await
            .unwrap();
        let response = decode(receiver.recv().await.unwrap().message);
        assert_eq!(response["type"], "error");
        assert_eq!(response["request_id"], "r1");
        assert_eq!(response["code"], expected);
        assert!(receiver.try_recv().is_err());
    }
}

#[test]
fn workspace_events_follow_the_response_and_absent_events_stay_absent() {
    for event in [None, Some(json!({"workspaceId":"w1"}))] {
        let runtime = runtime();
        let (outbound, mut receiver) = Outbound::new();
        let context = Context {
            request: request(),
            runtime: &runtime,
            outbound: &outbound,
            available_subscriptions: 16,
        };
        context
            .workspace(json!({"ok":true}), event.clone())
            .unwrap();
        let response = decode(receiver.try_recv().unwrap().message);
        assert_eq!(response["type"], "response");
        assert_eq!(response["request_id"], "r1");
        if let Some(event) = event {
            let message = decode(receiver.try_recv().unwrap().message);
            assert_eq!(message["method"], "workspace.update");
            assert_eq!(message["params"], event);
        }
        assert!(receiver.try_recv().is_err());
    }
}

#[test]
fn failed_response_prevents_the_workspace_event() {
    let runtime = runtime();
    let (outbound, receiver) = Outbound::new();
    drop(receiver);
    let context = Context {
        request: request(),
        runtime: &runtime,
        outbound: &outbound,
        available_subscriptions: 16,
    };
    assert!(matches!(
        context.workspace(json!({}), Some(json!({}))),
        Err(QueueError::Full)
    ));
    assert!(outbound.failure().is_cancelled());
}
