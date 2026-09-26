mod dictation;
mod dictation_lifecycle;
mod voice;
mod voice_lifecycle;

use std::{
    sync::{
        Mutex,
        atomic::{AtomicBool, AtomicUsize, Ordering},
    },
    time::Duration,
};

use base64::{Engine as Base64Engine, engine::general_purpose::STANDARD};
use server_model::{
    Lifecycle, Limits, ServerInfo, VERSION,
    outbound::{Frame, Queued},
};

use crate::{
    audio::Format,
    ports::{Agents, Operation, Synthesizer, Transcriber},
};

use super::*;

#[derive(Debug, Default)]
struct Engine {
    calls: AtomicUsize,
    cancelled: AtomicUsize,
    blocking: AtomicBool,
    failing: AtomicBool,
    agent_blocking: AtomicBool,
    samples: Mutex<Vec<Vec<u8>>>,
    transcript: Mutex<Option<String>>,
    agent_calls: AtomicUsize,
    synthesis_calls: AtomicUsize,
    synthesis_blocking: AtomicBool,
    synthesis_failing: AtomicBool,
    agent_failing: AtomicBool,
    empty_reply: AtomicBool,
    resolve_blocking: AtomicBool,
}

impl Transcriber for Engine {
    fn transcribe(&self, audio: Audio, cancel: CancellationToken) -> Operation<'_, Transcript> {
        Box::pin(async move {
            self.calls.fetch_add(1, Ordering::SeqCst);
            self.samples.lock().unwrap().push(audio.bytes);
            if self.failing.load(Ordering::SeqCst) {
                return Err(Error::Provider);
            }
            if self.blocking.load(Ordering::SeqCst) {
                cancel.cancelled().await;
                self.cancelled.fetch_add(1, Ordering::SeqCst);
                return Err(Error::Cancelled);
            }
            Ok(Transcript {
                text: self
                    .transcript
                    .lock()
                    .unwrap()
                    .clone()
                    .unwrap_or_else(|| "recognized speech".to_owned()),
                language: Some("en".to_owned()),
            })
        })
    }
}

impl Synthesizer for Engine {
    fn synthesize<'a>(&'a self, text: &'a str, cancel: CancellationToken) -> Operation<'a, Audio> {
        Box::pin(async move {
            self.synthesis_calls.fetch_add(1, Ordering::SeqCst);
            assert_eq!(text, "Agent reply");
            if self.synthesis_failing.load(Ordering::SeqCst) {
                return Err(Error::Provider);
            }
            if self.synthesis_blocking.load(Ordering::SeqCst) {
                cancel.cancelled().await;
                self.cancelled.fetch_add(1, Ordering::SeqCst);
                return Err(Error::Cancelled);
            }
            Ok(Audio {
                bytes: vec![1; 150_000],
                format: Format::Pcm(24000),
            })
        })
    }
}

impl Agents for Engine {
    fn resolve<'a>(&'a self, identifier: &'a str) -> Operation<'a, String> {
        Box::pin(async move {
            if self.resolve_blocking.load(Ordering::SeqCst) {
                std::future::pending::<()>().await;
            }
            if identifier == "bad" {
                Err(Error::Agent)
            } else {
                Ok("agent-1".to_owned())
            }
        })
    }

    fn turn<'a>(
        &'a self,
        agent: &'a str,
        text: &'a str,
        cancel: CancellationToken,
    ) -> Operation<'a, String> {
        Box::pin(async move {
            self.agent_calls.fetch_add(1, Ordering::SeqCst);
            assert_eq!(agent, "agent-1");
            if self.agent_failing.load(Ordering::SeqCst) {
                return Err(Error::Agent);
            }
            assert_eq!(text, "recognized speech");
            if self.agent_blocking.load(Ordering::SeqCst) {
                cancel.cancelled().await;
                self.cancelled.fetch_add(1, Ordering::SeqCst);
                return Err(Error::Cancelled);
            }
            Ok(if self.empty_reply.load(Ordering::SeqCst) {
                " \n ".to_owned()
            } else {
                "Agent reply".to_owned()
            })
        })
    }
}

struct Fixture {
    connection: Connection,
    service: Speech,
    runtime: Arc<Runtime>,
    outbound: Outbound,
    receiver: mpsc::Receiver<Queued>,
    engine: Arc<Engine>,
    events: Vec<Value>,
}

impl Fixture {
    fn new() -> Self {
        let engine = Arc::new(Engine::default());
        let service = Speech::new(
            Some(engine.clone()),
            Some(engine.clone()),
            Some(engine.clone()),
        );
        let runtime = Arc::new(Runtime::new(ServerInfo {
            server_id: "test".to_owned(),
            instance_id: "instance".to_owned(),
            listen: "127.0.0.1:1".to_owned(),
            lifecycle: Lifecycle::Ready,
            protocol: VERSION,
            capabilities: Vec::new(),
            implemented_capabilities: Vec::new(),
            limits: Limits::default(),
        }));
        let (outbound, receiver) = Outbound::new();
        Self {
            connection: Connection::default(),
            service,
            runtime,
            outbound,
            receiver,
            engine,
            events: Vec::new(),
        }
    }

    fn event(&mut self, method: &str, params: Value) {
        let state = crate::dispatch::State {
            runtime: self.runtime.clone(),
            speech: Some(self.service.clone()),
        };
        self.connection
            .event(method, params, &state, &self.outbound)
            .unwrap();
        self.drain();
    }

    fn drain(&mut self) {
        while let Ok(frame) = self.receiver.try_recv() {
            let Frame::Text(text) = frame.message else {
                panic!("unexpected binary");
            };
            self.events.push(serde_json::from_str(&text).unwrap());
        }
    }

    fn poll(&mut self) {
        self.connection
            .poll(&self.service, &self.runtime, &self.outbound)
            .unwrap();
        self.drain();
    }

    async fn until(&mut self, method: &str) -> Value {
        for _ in 0..200 {
            self.poll();
            if let Some(index) = self
                .events
                .iter()
                .position(|event| event["method"] == method)
            {
                return self.events.remove(index)["params"].clone();
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
        panic!("missing {method}: {:?}", self.events);
    }

    fn start(&mut self, id: &str) {
        self.event(
            "dictation.stream.start",
            json!({"dictationId":id,"format":"audio/pcm;rate=16000;bits=16"}),
        );
    }

    fn chunk(&mut self, id: &str, seq: u32, bytes: &[u8]) {
        self.event("dictation.stream.chunk", json!({"dictationId":id,"seq":seq,"audio":STANDARD.encode(bytes),"format":"audio/pcm;rate=16000;bits=16"}));
    }

    async fn mode(&mut self, enabled: bool) -> Value {
        self.connection
            .request(
                "voice.mode.set.request",
                json!({"enabled":enabled,"agentId":"agent"}),
                &self.service,
            )
            .await
            .unwrap()
    }
}
