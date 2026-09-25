//! Speech engines and native Agent execution are replaceable external boundaries.

use std::{future::Future, pin::Pin};

use tokio_util::sync::CancellationToken;

use crate::{Error, audio::Audio};

/// Cancellable asynchronous adapter result, borrowing its provider.
pub type Operation<'a, T> = Pin<Box<dyn Future<Output = Result<T, Error>> + Send + 'a>>;

/// Recognized text and optional provider metadata.
#[derive(Debug, Clone, Default, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Transcript {
    /// Recognized text; an empty string represents no speech.
    pub text: String,
    /// Detected or configured language.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub language: Option<String>,
}

/// Speech-to-text boundary. Implementations must honor cancellation and bound output.
pub trait Transcriber: Send + Sync + std::fmt::Debug {
    /// Recognize audio, returning text or a safe configuration/provider/cancellation error.
    fn transcribe(&self, audio: Audio, cancel: CancellationToken) -> Operation<'_, Transcript>;
}

/// Text-to-speech boundary. Implementations return mono PCM16 audio.
pub trait Synthesizer: Send + Sync + std::fmt::Debug {
    /// Synthesize text, returning audio or a safe configuration/provider/cancellation error.
    fn synthesize<'a>(&'a self, text: &'a str, cancel: CancellationToken) -> Operation<'a, Audio>;
}

/// Native Agent coordination supplied by the composition root.
pub trait Agents: Send + Sync + std::fmt::Debug {
    /// Resolve and validate an enabled voice target, returning its canonical identity.
    fn resolve<'a>(&'a self, identifier: &'a str) -> Operation<'a, String>;
    /// Execute a text turn and return its final response. Cancellation must interrupt owned work
    /// and await its cleanup before this operation returns.
    fn turn<'a>(
        &'a self,
        agent: &'a str,
        text: &'a str,
        cancel: CancellationToken,
    ) -> Operation<'a, String>;
}
