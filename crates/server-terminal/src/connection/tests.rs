//! Paseo owned-subscriptions and terminal-session-controller transport contracts.

use std::sync::{Arc, Mutex};

use server_model::outbound::{Frame, Queued};
use server_model::{Lifecycle, Limits, Runtime, ServerInfo, VERSION};
use tokio::sync::mpsc;

use super::*;
use crate::test_support::{Calls, fixture, request};

mod delivery;
mod ownership;

struct Fixture {
    state: Shared,
    calls: Arc<Mutex<Calls>>,
    terminal: String,
    connection: TerminalConnection,
    outbound: Outbound,
    receiver: mpsc::Receiver<Queued>,
}

impl Fixture {
    fn new() -> Self {
        let (mut service, _, calls) = fixture();
        let terminal = service.create(&request()).unwrap().id;
        calls.lock().unwrap().observation = Some(Observation {
            revision: 1,
            size: wire::Size::default(),
            frames: vec![(Opcode::Snapshot, b"bootstrap".to_vec())],
            exited: false,
        });
        let runtime = Arc::new(Runtime::new(ServerInfo {
            server_id: "server".to_owned(),
            instance_id: "instance".to_owned(),
            listen: "127.0.0.1:7316".to_owned(),
            lifecycle: Lifecycle::Ready,
            protocol: VERSION,
            capabilities: vec![],
            implemented_capabilities: vec![],
            limits: Limits::default(),
        }));
        let (outbound, receiver) = Outbound::new();
        Self {
            state: Shared {
                runtime,
                terminals: Some(Arc::new(Mutex::new(service))),
            },
            calls,
            terminal,
            connection: TerminalConnection::default(),
            outbound,
            receiver,
        }
    }

    async fn request(&mut self, method: &str, params: Value, available: usize) -> Value {
        self.connection
            .request(
                Request {
                    id: "source-request".to_owned(),
                    method: method.to_owned(),
                    params,
                    available,
                },
                &self.state,
                &self.outbound,
            )
            .await
            .unwrap();
        let value = self.json();
        assert_eq!(value["request_id"], "source-request");
        value
    }

    async fn subscribe(&mut self) -> Value {
        let response = self
            .request(
                "terminal.subscribe.request",
                json!({"terminalId":self.terminal}),
                16,
            )
            .await;
        assert!(response["result"]["error"].is_null());
        response["result"].clone()
    }

    fn json(&mut self) -> Value {
        let frame = self.receiver.try_recv().unwrap();
        let Frame::Text(text) = &frame.message else {
            panic!("expected JSON, got {:?}", frame.message);
        };
        serde_json::from_str(text).unwrap()
    }

    fn binary(&mut self) -> Vec<u8> {
        let frame = self.receiver.try_recv().unwrap();
        let Frame::Binary(bytes) = &frame.message else {
            panic!("expected binary, got {:?}", frame.message);
        };
        bytes.clone()
    }

    fn assert_quiet(&mut self) {
        assert!(matches!(
            self.receiver.try_recv(),
            Err(mpsc::error::TryRecvError::Empty)
        ));
    }

    fn output(&self, bytes: &[u8], exited: bool) {
        let mut calls = self.calls.lock().unwrap();
        let observation = calls.observation.as_mut().unwrap();
        observation.revision += 1;
        observation.frames = vec![(Opcode::Output, bytes.to_vec())];
        observation.exited = exited;
    }

    async fn poll(&mut self) {
        self.connection
            .poll(&self.state, &self.outbound)
            .await
            .unwrap();
    }
}

fn slot(value: &Value) -> u8 {
    u8::try_from(value["slot"].as_u64().unwrap()).unwrap()
}

fn subscription(value: &Value) -> &str {
    value["subscriptionId"].as_str().unwrap()
}
