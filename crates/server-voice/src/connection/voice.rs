use std::{collections::BTreeMap, sync::Arc, time::Duration};

use base64::{Engine, engine::general_purpose::STANDARD};
use serde_json::{Value, json};
use server_model::{
    Runtime,
    outbound::{Outbound, QueueError},
};
use tokio::time::Instant;
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

use crate::{
    Error,
    audio::{Audio, Format, MAX_AUDIO_BYTES, decode_chunk},
    protocol,
    service::{Speech, Target},
};

use super::{Connection, emit, jobs, voice_error};

#[derive(Default)]
pub(super) struct Voice {
    target: Option<Arc<Target>>,
    bytes: Vec<u8>,
    format: Option<Format>,
    speaking: bool,
    silent_samples: usize,
    updated: Option<Instant>,
    pub generation: String,
    pub running: bool,
    cancel: CancellationToken,
    playback: Option<Playback>,
    pub(super) acknowledgments: BTreeMap<String, Instant>,
}

struct Playback {
    audio: Audio,
    group: String,
    index: usize,
    offset: usize,
}

impl Voice {
    pub(super) fn is_empty(&self) -> bool {
        self.target.is_none() && self.bytes.is_empty() && !self.running && self.playback.is_none()
    }

    fn abort(&mut self) {
        self.cancel.cancel();
        self.generation = Uuid::new_v4().to_string();
        self.running = false;
        self.bytes.clear();
        self.format = None;
        self.speaking = false;
        self.silent_samples = 0;
        self.updated = None;
        self.playback = None;
        self.acknowledgments.clear();
    }

    pub(super) fn played(&mut self, id: &str) {
        self.acknowledgments.remove(id);
    }

    pub(super) fn output(&mut self, audio: Audio) {
        self.playback = Some(Playback {
            audio,
            group: Uuid::new_v4().to_string(),
            index: 0,
            offset: 0,
        });
    }

    pub(super) fn poll(&mut self, outbound: &Outbound) -> Result<(), QueueError> {
        if self
            .updated
            .is_some_and(|updated| updated.elapsed() >= Duration::from_secs(60))
            || self
                .acknowledgments
                .values()
                .any(|time| time.elapsed() > Duration::from_secs(30))
        {
            self.abort();
            emit(outbound, "voice.input.state", json!({"isSpeaking":false}))?;
            return voice_error(outbound, Error::Timeout);
        }
        let Some(playback) = &mut self.playback else {
            return Ok(());
        };
        if self.acknowledgments.len() >= 4 {
            return Ok(());
        }
        let end = (playback.offset + 24_000).min(playback.audio.bytes.len());
        let last = end == playback.audio.bytes.len();
        let id = Uuid::new_v4().to_string();
        emit(
            outbound,
            "voice.audio.output",
            json!({
                "audio":STANDARD.encode(&playback.audio.bytes[playback.offset..end]), "format":playback.audio.format.mime(),
                "id":id, "isVoiceMode":true, "groupId":playback.group, "chunkIndex":playback.index, "isLastChunk":last
            }),
        )?;
        self.acknowledgments.insert(id, Instant::now());
        playback.offset = end;
        playback.index += 1;
        if last {
            self.playback = None;
        }
        Ok(())
    }
}

impl Connection {
    pub(crate) async fn request(
        &mut self,
        method: &str,
        params: Value,
        service: &Speech,
    ) -> Result<Value, Error> {
        match method {
            "voice.abort.request" => {
                if params != json!({}) {
                    return Err(Error::Invalid);
                }
                self.voice.abort();
                Ok(json!({"accepted":true}))
            }
            "voice.mode.set.request" => {
                let request: protocol::Mode = protocol::decode(params)?;
                if request.enabled {
                    if !service.availability().1 {
                        return Ok(mode_result(&self.voice, Some(Error::Unavailable)));
                    }
                    let id = request
                        .agent_id
                        .as_deref()
                        .filter(|id| server_model::valid_id(id))
                        .ok_or(Error::Invalid)?;
                    let agents = service.agents.as_ref().ok_or(Error::Unavailable)?;
                    let target =
                        match tokio::time::timeout(Duration::from_secs(10), agents.resolve(id))
                            .await
                        {
                            Ok(Ok(id)) => id,
                            Ok(Err(error)) => return Ok(mode_result(&self.voice, Some(error))),
                            Err(_) => return Ok(mode_result(&self.voice, Some(Error::Timeout))),
                        };
                    if self
                        .voice
                        .target
                        .as_ref()
                        .is_none_or(|current| current.id != target)
                    {
                        let target = match service.claim(target) {
                            Ok(target) => target,
                            Err(error) => return Ok(mode_result(&self.voice, Some(error))),
                        };
                        self.voice.abort();
                        self.voice.target = Some(target);
                    }
                } else {
                    self.voice.abort();
                    self.voice.target = None;
                }
                Ok(mode_result(&self.voice, None))
            }
            _ => Err(Error::Invalid),
        }
    }

    pub(super) fn voice_chunk(
        &mut self,
        request: &protocol::VoiceChunk,
        service: &Speech,
        runtime: &Arc<Runtime>,
        outbound: &Outbound,
    ) -> Result<(), Error> {
        if service.stt.is_none() {
            return Err(Error::Unavailable);
        }
        let format = Format::parse(&request.format)?;
        let bytes = if request.audio.is_empty() && request.is_last {
            Vec::new()
        } else {
            decode_chunk(&request.audio)?
        };
        if self.voice.format.is_some_and(|current| current != format)
            || matches!(format, Format::Pcm(_)) && !bytes.len().is_multiple_of(2)
        {
            return Err(Error::Invalid);
        }
        if self.voice.bytes.len().saturating_add(bytes.len()) > MAX_AUDIO_BYTES {
            return Err(Error::Capacity);
        }
        let active = match format {
            Format::Pcm(_) => bytes
                .as_chunks::<2>()
                .0
                .iter()
                .any(|sample| i16::from_le_bytes([sample[0], sample[1]]).unsigned_abs() > 600),
            Format::Wav => !bytes.is_empty(),
        };
        if active && !self.voice.speaking {
            self.voice.abort(); // Barge-in cancels old STT, Agent work, synthesis and playback.
            self.voice.speaking = true;
            emit(outbound, "voice.input.state", json!({"isSpeaking":true}))
                .map_err(|_| Error::Cancelled)?;
        }
        if !self.voice.speaking && !request.is_last {
            return Ok(());
        }
        self.voice.format = Some(format);
        self.voice.bytes.extend_from_slice(&bytes);
        self.voice.updated = Some(Instant::now());
        self.voice.silent_samples = if active {
            0
        } else {
            self.voice.silent_samples + bytes.len() / 2
        };
        let silence_end = matches!(format, Format::Pcm(rate) if self.voice.speaking && self.voice.silent_samples >= rate as usize * 3 / 5);
        if !request.is_last && !silence_end {
            return Ok(());
        }
        if self.voice.bytes.is_empty() {
            self.voice.format = None;
            self.voice.updated = None;
            return Ok(());
        }
        let permit = service
            .jobs
            .clone()
            .try_acquire_owned()
            .map_err(|_| Error::Capacity)?;
        self.voice.cancel = self.cancel.child_token();
        self.voice.generation = Uuid::new_v4().to_string();
        self.voice.running = true;
        let audio = Audio {
            bytes: std::mem::take(&mut self.voice.bytes),
            format,
        };
        self.voice.format = None;
        self.voice.speaking = false;
        self.voice.silent_samples = 0;
        self.voice.updated = None;
        emit(outbound, "voice.input.state", json!({"isSpeaking":false}))
            .map_err(|_| Error::Cancelled)?;
        jobs::voice(
            runtime,
            jobs::VoiceJob {
                generation: self.voice.generation.clone(),
                audio,
                target: self.voice.target.clone(),
                service: service.clone(),
                cancel: self.voice.cancel.clone(),
                sender: self.sender.clone(),
                permit,
            },
        );
        Ok(())
    }
}

fn mode_result(voice: &Voice, error: Option<Error>) -> Value {
    let mut result = json!({"enabled":voice.target.is_some(),"agentId":voice.target.as_ref().map(|target| &target.id),"accepted":error.is_none(),"error":error.map(|error| error.to_string())});
    if let Some(error) = error {
        result["reasonCode"] = json!(error.reason());
        result["retryable"] = json!(error.retryable());
    }
    result
}
