//! Canonical voice methods; payload fields retain Paseo's camelCase spelling.

use serde::Deserialize;

/// Implemented request and event capabilities.
pub const CAPABILITIES: &[&str] = &[
    "voice.mode.set.request",
    "voice.audio.chunk",
    "voice.abort.request",
    "voice.audio.played",
    "dictation.stream.start",
    "dictation.stream.chunk",
    "dictation.stream.finish",
    "dictation.stream.cancel",
];

/// Enable voice for a specific Agent, or disable the current connection's voice mode.
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Mode {
    /// Desired mode.
    pub enabled: bool,
    /// Required when enabling; resolved to one canonical Agent ID.
    pub agent_id: Option<String>,
}

/// Base64 microphone audio with an optional utterance boundary.
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct VoiceChunk {
    /// Base64 bytes.
    pub audio: String,
    /// Audio MIME type, including PCM sample rate where applicable.
    pub format: String,
    /// Explicit end of utterance; PCM also supports silence-based boundaries.
    pub is_last: bool,
}

/// Confirm one server audio chunk has actually finished playing.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Played {
    /// Audio chunk ID, scoped to this physical connection.
    pub id: String,
}

/// Start or acknowledge an existing stream on the same connection.
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Start {
    /// Connection-local stream ID.
    pub dictation_id: String,
    /// Immutable format for the entire stream.
    pub format: String,
}

/// A sequenced, replayable audio chunk.
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Chunk {
    /// Connection-local stream ID.
    pub dictation_id: String,
    /// Zero-based sequence number.
    pub seq: u32,
    /// Base64 bytes.
    pub audio: String,
    /// Must match the stream's start format.
    pub format: String,
}

/// Finish after every chunk through `final_seq` has arrived.
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Finish {
    /// Connection-local stream ID.
    pub dictation_id: String,
    /// Inclusive final sequence number.
    pub final_seq: u32,
}

/// Cancel a stream and any outstanding transcription work.
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Cancel {
    /// Connection-local stream ID.
    pub dictation_id: String,
}

pub(crate) fn decode<T: serde::de::DeserializeOwned>(
    value: serde_json::Value,
) -> Result<T, crate::Error> {
    serde_json::from_value(value).map_err(|_| crate::Error::Invalid)
}
