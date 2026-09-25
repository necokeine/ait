use std::{collections::BTreeMap, sync::Arc, time::Duration};

use serde_json::{Value, json};
use server_model::{
    Runtime,
    outbound::{Outbound, QueueError},
    valid_id,
};
use tokio::{sync::OwnedSemaphorePermit, time::Instant};
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

use crate::{
    Error,
    audio::{Audio, Format, MAX_AUDIO_BYTES, decode_chunk},
    ports::Transcript,
    protocol,
    service::Speech,
};

use super::{Connection, emit, jobs, protocol_error};

const MAX_STREAMS: usize = 4;
const MAX_RETAINED: usize = 32;
const MAX_CHUNKS: u32 = 16_384;
const FINISH_TIMEOUT: Duration = Duration::from_secs(120);
const IDLE_TIMEOUT: Duration = Duration::from_secs(60);

pub(super) struct Dictation {
    generation: String,
    format: Format,
    pending: BTreeMap<u32, Vec<u8>>,
    ranges: Vec<std::ops::Range<usize>>,
    bytes: Vec<u8>,
    size: usize,
    final_seq: Option<u32>,
    deadline: Option<Instant>,
    updated: Instant,
    partial_at: Instant,
    partial_bytes: usize,
    busy: bool,
    completed: Option<Value>,
    cancel: CancellationToken,
    permit: Option<OwnedSemaphorePermit>,
}

impl Drop for Dictation {
    fn drop(&mut self) {
        self.cancel.cancel();
    }
}

impl Dictation {
    fn ack(&self) -> i64 {
        i64::try_from(self.ranges.len()).unwrap_or(0) - 1
    }

    fn chunk(&mut self, chunk: &protocol::Chunk) -> Result<(), Error> {
        if Format::parse(&chunk.format)? != self.format
            || chunk.seq >= MAX_CHUNKS
            || chunk.seq as usize > self.ranges.len() + 128
            || self.final_seq.is_some_and(|last| chunk.seq > last)
        {
            return Err(Error::Invalid);
        }
        let bytes = decode_chunk(&chunk.audio)?;
        if matches!(self.format, Format::Pcm(_)) && !bytes.len().is_multiple_of(2) {
            return Err(Error::Invalid);
        }
        if let Some(range) = self.ranges.get(chunk.seq as usize) {
            return if self.bytes.get(range.clone()) == Some(bytes.as_slice()) {
                Ok(())
            } else {
                Err(Error::Invalid)
            };
        }
        if let Some(previous) = self.pending.get(&chunk.seq) {
            return if previous == &bytes {
                Ok(())
            } else {
                Err(Error::Invalid)
            };
        }
        if self.completed.is_some() || self.size.saturating_add(bytes.len()) > MAX_AUDIO_BYTES {
            return Err(Error::Capacity);
        }
        self.size += bytes.len();
        self.pending.insert(chunk.seq, bytes);
        while let Some(bytes) = self
            .pending
            .remove(&(u32::try_from(self.ranges.len()).map_err(|_| Error::Capacity)?))
        {
            let start = self.bytes.len();
            self.bytes.extend_from_slice(&bytes);
            self.ranges.push(start..self.bytes.len());
        }
        self.updated = Instant::now();
        Ok(())
    }

    fn finish(&mut self, final_seq: u32) -> Result<(), Error> {
        if final_seq >= MAX_CHUNKS
            || self.final_seq.is_some_and(|previous| previous != final_seq)
            || self.ack() > i64::from(final_seq)
            || self
                .pending
                .last_key_value()
                .is_some_and(|(seq, _)| *seq > final_seq)
        {
            return Err(Error::Invalid);
        }
        if self.final_seq.is_none() {
            self.final_seq = Some(final_seq);
            self.deadline = Some(Instant::now() + FINISH_TIMEOUT);
        }
        Ok(())
    }
}

impl Connection {
    pub(super) fn dictation_event(
        &mut self,
        input: (&str, Value),
        service: &Speech,
        runtime: &Arc<Runtime>,
        outbound: &Outbound,
    ) -> Result<(), QueueError> {
        let (method, params) = input;
        let Some(id) = params
            .get("dictationId")
            .and_then(Value::as_str)
            .filter(|id| valid_id(id))
            .map(str::to_owned)
        else {
            return protocol_error(outbound, server_model::ErrorCode::InvalidMessage);
        };
        let result = match method {
            "dictation.stream.start" => {
                protocol::decode(params).and_then(|request| self.start(request, service))
            }
            "dictation.stream.chunk" => {
                protocol::decode(params).and_then(|request: protocol::Chunk| {
                    self.dictations
                        .get_mut(&id)
                        .ok_or(Error::Invalid)?
                        .chunk(&request)
                })
            }
            "dictation.stream.finish" => {
                protocol::decode(params).and_then(|request: protocol::Finish| {
                    self.dictations
                        .get_mut(&id)
                        .ok_or(Error::Invalid)?
                        .finish(request.final_seq)
                })
            }
            "dictation.stream.cancel" => protocol::decode::<protocol::Cancel>(params).map(|_| {
                self.dictations.remove(&id);
            }),
            _ => Err(Error::Invalid),
        };
        if let Err(error) = result {
            // Reject a malformed/replayed mutation without discarding previously acknowledged audio.
            return dictation_error(outbound, &id, error);
        }
        let Some(stream) = self.dictations.get(&id) else {
            return Ok(());
        };
        if method == "dictation.stream.finish" {
            emit(
                outbound,
                "dictation.stream.finish.accepted",
                json!({"dictationId":id,"timeoutMs":120_000}),
            )?;
            if let Some(result) = &stream.completed {
                return emit(outbound, "dictation.stream.final", result.clone());
            }
        } else {
            emit(
                outbound,
                "dictation.stream.ack",
                json!({"dictationId":id,"ackSeq":stream.ack()}),
            )?;
        }
        self.poll_dictations(service, runtime, outbound)
    }

    fn start(&mut self, request: protocol::Start, service: &Speech) -> Result<(), Error> {
        let format = Format::parse(&request.format)?;
        if let Some(existing) = self.dictations.get(&request.dictation_id) {
            return if existing.format == format {
                Ok(())
            } else {
                Err(Error::Invalid)
            };
        }
        if service.stt.is_none() {
            return Err(Error::Unavailable);
        }
        if self
            .dictations
            .values()
            .filter(|stream| stream.completed.is_none())
            .count()
            >= MAX_STREAMS
        {
            return Err(Error::Capacity);
        }
        if self.dictations.len() >= MAX_RETAINED {
            let oldest = self
                .dictations
                .iter()
                .filter(|(_, stream)| stream.completed.is_some())
                .min_by_key(|(_, stream)| stream.updated)
                .map(|(id, _)| id.clone());
            if let Some(id) = oldest {
                self.dictations.remove(&id);
            }
        }
        let now = Instant::now();
        self.dictations.insert(
            request.dictation_id,
            Dictation {
                generation: Uuid::new_v4().to_string(),
                format,
                pending: BTreeMap::new(),
                ranges: Vec::new(),
                bytes: Vec::new(),
                size: 0,
                final_seq: None,
                deadline: None,
                updated: now,
                partial_at: now,
                partial_bytes: 0,
                busy: false,
                completed: None,
                cancel: self.cancel.child_token(),
                permit: Some(service.stream()?),
            },
        );
        Ok(())
    }

    pub(super) fn poll_dictations(
        &mut self,
        service: &Speech,
        runtime: &Arc<Runtime>,
        outbound: &Outbound,
    ) -> Result<(), QueueError> {
        let now = Instant::now();
        let expired: Vec<_> = self
            .dictations
            .iter()
            .filter(|(_, stream)| {
                stream.deadline.is_some_and(|deadline| now >= deadline)
                    || now.duration_since(stream.updated) >= IDLE_TIMEOUT
                        && stream.deadline.is_none()
            })
            .map(|(id, _)| id.clone())
            .collect();
        for id in expired {
            if self
                .dictations
                .remove(&id)
                .is_some_and(|stream| stream.completed.is_none())
            {
                dictation_error(outbound, &id, Error::Timeout)?;
            }
        }
        for (id, stream) in &mut self.dictations {
            if stream.busy || stream.completed.is_some() {
                continue;
            }
            let final_result = stream
                .final_seq
                .is_some_and(|seq| stream.ack() == i64::from(seq));
            let partial = stream.final_seq.is_none()
                && matches!(stream.format, Format::Pcm(_))
                && stream.bytes.len().saturating_sub(stream.partial_bytes) >= 32_000
                && now.duration_since(stream.partial_at) >= Duration::from_secs(2);
            if !final_result && !partial {
                continue;
            }
            let Ok(permit) = service.jobs.clone().try_acquire_owned() else {
                continue;
            };
            let Some(stt) = service.stt.clone() else {
                continue;
            };
            stream.busy = true;
            stream.partial_at = now;
            stream.partial_bytes = stream.bytes.len();
            let audio = Audio {
                bytes: stream.bytes.clone(),
                format: stream.format,
            };
            jobs::dictation(
                runtime,
                jobs::DictationJob {
                    id: id.clone(),
                    generation: stream.generation.clone(),
                    final_result,
                    audio,
                    stt,
                    cancel: stream.cancel.child_token(),
                    sender: self.sender.clone(),
                    permit,
                },
            );
        }
        Ok(())
    }

    pub(super) fn dictation_completed(
        &mut self,
        key: (&str, &str),
        final_result: bool,
        result: Result<Transcript, Error>,
        outbound: &Outbound,
    ) -> Result<(), QueueError> {
        let (id, generation) = key;
        let Some(stream) = self
            .dictations
            .get_mut(id)
            .filter(|stream| stream.generation == generation)
        else {
            return Ok(());
        };
        stream.busy = false;
        match result {
            Ok(transcript) if final_result => {
                let params = json!({"dictationId":id,"text":transcript.text});
                emit(outbound, "dictation.stream.final", params.clone())?;
                stream.completed = Some(params);
                stream.bytes.clear();
                stream.bytes.shrink_to_fit();
                stream.pending.clear();
                stream.deadline = None;
                stream.updated = Instant::now();
                stream.permit = None;
            }
            Ok(transcript) if stream.final_seq.is_none() => emit(
                outbound,
                "dictation.stream.partial",
                json!({"dictationId":id,"text":transcript.text}),
            )?,
            Err(error) if final_result => {
                self.dictations.remove(id);
                dictation_error(outbound, id, error)?;
            }
            Ok(_) | Err(_) => {} // A partial failure cannot invalidate losslessly buffered audio.
        }
        Ok(())
    }
}

fn dictation_error(outbound: &Outbound, id: &str, error: Error) -> Result<(), QueueError> {
    emit(
        outbound,
        "dictation.stream.error",
        json!({"dictationId":id,"error":error.to_string(),"reasonCode":error.reason(),"retryable":error.retryable()}),
    )
}
