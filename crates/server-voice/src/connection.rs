//! Physical-connection ownership, cancellation and ordered speech delivery.

mod dictation;
mod jobs;
mod voice;

use std::{collections::BTreeMap, sync::Arc};

use serde_json::{Value, json};
use server_model::{
    Runtime, ServerMessage,
    outbound::{Outbound, QueueError},
};
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

use crate::{Error, audio::Audio, ports::Transcript, service::Speech};

use dictation::Dictation;
use voice::Voice;

/// All speech streams and playback acknowledgments belonging to one socket.
pub struct Connection {
    dictations: BTreeMap<String, Dictation>,
    voice: Voice,
    cancel: CancellationToken,
    sender: mpsc::Sender<Completion>,
    receiver: mpsc::Receiver<Completion>,
}

impl Default for Connection {
    fn default() -> Self {
        let (sender, receiver) = mpsc::channel(32);
        Self {
            dictations: BTreeMap::new(),
            voice: Voice::default(),
            cancel: CancellationToken::new(),
            sender,
            receiver,
        }
    }
}

impl Drop for Connection {
    fn drop(&mut self) {
        self.cancel.cancel();
    }
}

enum Completion {
    Dictation {
        id: String,
        generation: String,
        final_result: bool,
        result: Result<Transcript, Error>,
    },
    Transcript {
        generation: String,
        transcript: Transcript,
    },
    Audio {
        generation: String,
        audio: Audio,
    },
    VoiceFinished {
        generation: String,
        result: Result<(), Error>,
    },
}

impl Connection {
    /// Whether this connection owns any state that requires expiry or completion polling.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.dictations.is_empty() && self.voice.is_empty()
    }

    /// Apply an already-negotiated speech notification.
    /// # Errors
    /// Returns malformed-message errors or bounded output queue failures; valid stream failures
    /// are delivered through the corresponding dictation/voice error events.
    pub fn event(
        &mut self,
        method: &str,
        params: Value,
        state: &crate::dispatch::State,
        outbound: &Outbound,
    ) -> Result<(), QueueError> {
        let runtime = &state.runtime;
        let Some(service) = &state.speech else {
            return protocol_error(outbound, server_model::ErrorCode::UnsupportedCapability);
        };
        if runtime.cancellation.is_cancelled() {
            return protocol_error(outbound, server_model::ErrorCode::ServerDraining);
        }
        let result = match method {
            "dictation.stream.start"
            | "dictation.stream.chunk"
            | "dictation.stream.finish"
            | "dictation.stream.cancel" => {
                return self.dictation_event((method, params), service, runtime, outbound);
            }
            "voice.audio.chunk" => crate::protocol::decode(params)
                .and_then(|chunk| self.voice_chunk(&chunk, service, runtime, outbound)),
            "voice.audio.played" => crate::protocol::decode::<crate::protocol::Played>(params)
                .and_then(|played| {
                    if !server_model::valid_id(&played.id) {
                        return Err(Error::Invalid);
                    }
                    self.voice.played(&played.id);
                    Ok(())
                }),
            _ => Err(Error::Invalid),
        };
        if let Err(error) = result {
            return voice_error(outbound, error);
        }
        Ok(())
    }

    /// Deliver completed work and advance timeouts, partial transcripts and playback.
    /// # Errors
    /// Returns the first output encoding or queue error.
    pub fn poll(
        &mut self,
        service: &Speech,
        runtime: &Arc<Runtime>,
        outbound: &Outbound,
    ) -> Result<(), QueueError> {
        while let Ok(completion) = self.receiver.try_recv() {
            match completion {
                Completion::Dictation {
                    id,
                    generation,
                    final_result,
                    result,
                } => {
                    self.dictation_completed((&id, &generation), final_result, result, outbound)?;
                }
                Completion::Transcript {
                    generation,
                    transcript,
                } if self.voice.generation == generation => {
                    let mut params = serde_json::to_value(transcript)?;
                    params["requestId"] = json!(generation);
                    emit(outbound, "voice.transcription.result", params)?;
                }
                Completion::Audio { generation, audio } if self.voice.generation == generation => {
                    self.voice.output(audio);
                }
                Completion::VoiceFinished { generation, result }
                    if self.voice.generation == generation =>
                {
                    self.voice.running = false;
                    if let Err(error) = result {
                        voice_error(outbound, error)?;
                    }
                }
                Completion::Transcript { .. }
                | Completion::Audio { .. }
                | Completion::VoiceFinished { .. } => {}
            }
        }
        self.poll_dictations(service, runtime, outbound)?;
        self.voice.poll(outbound)
    }
}

fn emit(outbound: &Outbound, method: &str, params: Value) -> Result<(), QueueError> {
    outbound.send(&ServerMessage::Event {
        method: method.to_owned(),
        params,
    })
}

fn protocol_error(outbound: &Outbound, code: server_model::ErrorCode) -> Result<(), QueueError> {
    outbound.send(&ServerMessage::Error {
        request_id: None,
        code,
        message: code.message().to_owned(),
        retryable: code.retryable(),
    })
}

fn voice_error(outbound: &Outbound, error: Error) -> Result<(), QueueError> {
    emit(
        outbound,
        "voice.error",
        json!({"error":error.to_string(), "reasonCode":error.reason(), "retryable":error.retryable()}),
    )
}

#[cfg(test)]
mod tests;
