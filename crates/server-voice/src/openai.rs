//! OpenAI-compatible file transcription and PCM speech synthesis over bounded HTTP.

use std::time::Duration;

use reqwest::{
    Client, Url,
    multipart::{Form, Part},
};
use secrecy::{ExposeSecret, SecretString};
use tokio_util::sync::CancellationToken;

use crate::{
    Error,
    audio::{Audio, Format, MAX_AUDIO_BYTES, MAX_TEXT_BYTES},
    ports::{Operation, Synthesizer, Transcriber, Transcript},
};

/// Explicit speech endpoint settings. Credentials are never serialized or logged.
pub struct Config {
    /// API root, normally ending in `/v1`.
    pub base_url: String,
    /// Optional token for authenticated endpoints.
    pub api_key: Option<SecretString>,
    /// Transcription model supported by the selected endpoint.
    pub stt_model: String,
    /// Speech synthesis model supported by the selected endpoint.
    pub tts_model: String,
    /// Voice identifier understood by the TTS model.
    pub voice: String,
}

impl std::fmt::Debug for Config {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SpeechHttpConfig").finish_non_exhaustive()
    }
}

/// HTTP speech adapter. Clones reuse one bounded connection pool.
#[derive(Debug, Clone)]
pub struct OpenAi {
    client: Client,
    config: std::sync::Arc<Config>,
}

impl OpenAi {
    /// Validate configuration and construct a reusable client without making a network request.
    /// # Errors
    /// Rejects invalid endpoint URLs, empty options or client initialization failures.
    pub fn new(config: Config) -> Result<Self, Error> {
        let url = Url::parse(&config.base_url).map_err(|_| Error::Invalid)?;
        if !matches!(url.scheme(), "http" | "https")
            || url.host_str().is_none()
            || !url.username().is_empty()
            || url.password().is_some()
            || url.query().is_some()
            || url.fragment().is_some()
            || [&config.stt_model, &config.tts_model, &config.voice]
                .iter()
                .any(|value| value.trim().is_empty() || value.len() > 128)
        {
            return Err(Error::Invalid);
        }
        let client = Client::builder()
            .timeout(Duration::from_secs(120))
            .connect_timeout(Duration::from_secs(10))
            .redirect(reqwest::redirect::Policy::none())
            .build()
            .map_err(|_| Error::Unavailable)?;
        Ok(Self {
            client,
            config: std::sync::Arc::new(config),
        })
    }

    fn post(&self, path: &str) -> reqwest::RequestBuilder {
        let request = self.client.post(format!(
            "{}/{path}",
            self.config.base_url.trim_end_matches('/')
        ));
        match &self.config.api_key {
            Some(key) => request.bearer_auth(key.expose_secret()),
            None => request,
        }
    }
}

impl Transcriber for OpenAi {
    fn transcribe(&self, audio: Audio, cancel: CancellationToken) -> Operation<'_, Transcript> {
        Box::pin(async move {
            let wav = tokio::task::spawn_blocking(move || audio.wav())
                .await
                .map_err(|_| Error::Provider)??;
            let file = Part::bytes(wav)
                .file_name("speech.wav")
                .mime_str("audio/wav")
                .map_err(|_| Error::Invalid)?;
            let form = Form::new()
                .part("file", file)
                .text("model", self.config.stt_model.clone())
                .text("response_format", "json");
            let bytes = response(
                self.post("audio/transcriptions").multipart(form),
                cancel,
                MAX_TEXT_BYTES * 2,
            )
            .await?;
            let transcript: Transcript =
                serde_json::from_slice(&bytes).map_err(|_| Error::Provider)?;
            if transcript.text.len() > MAX_TEXT_BYTES {
                return Err(Error::Capacity);
            }
            Ok(transcript)
        })
    }
}

impl Synthesizer for OpenAi {
    fn synthesize<'a>(&'a self, text: &'a str, cancel: CancellationToken) -> Operation<'a, Audio> {
        Box::pin(async move {
            if text.trim().is_empty() || text.len() > MAX_TEXT_BYTES {
                return Err(Error::Invalid);
            }
            let mut bytes = Vec::new();
            let mut remaining = text;
            while !remaining.is_empty() {
                let end = remaining
                    .char_indices()
                    .nth(2000)
                    .map_or(remaining.len(), |(index, _)| index);
                let request = self.post("audio/speech").json(&serde_json::json!({
                    "model":self.config.tts_model, "voice":self.config.voice, "input":&remaining[..end], "response_format":"pcm"
                }));
                let chunk = response(
                    request,
                    cancel.clone(),
                    MAX_AUDIO_BYTES.saturating_sub(bytes.len()),
                )
                .await?;
                if chunk.is_empty() || !chunk.len().is_multiple_of(2) {
                    return Err(Error::Provider);
                }
                bytes.extend_from_slice(&chunk);
                remaining = &remaining[end..];
            }
            Audio {
                bytes,
                format: Format::Pcm(24_000),
            }
            .pcm()
        })
    }
}

async fn response(
    request: reqwest::RequestBuilder,
    cancel: CancellationToken,
    limit: usize,
) -> Result<Vec<u8>, Error> {
    tokio::select! { biased;
        () = cancel.cancelled() => Err(Error::Cancelled),
        result = async {
            let mut response = request.send().await.map_err(|_| Error::Provider)?;
            if !response.status().is_success() {
                return Err(if matches!(response.status().as_u16(), 401 | 403 | 404) { Error::Unavailable } else { Error::Provider });
            }
            if response.content_length().is_some_and(|size| size > limit as u64) { return Err(Error::Capacity); }
            let mut bytes = Vec::new();
            while let Some(chunk) = response.chunk().await.map_err(|_| Error::Provider)? {
                if bytes.len().saturating_add(chunk.len()) > limit { return Err(Error::Capacity); }
                bytes.extend_from_slice(&chunk);
            }
            Ok(bytes)
        } => result,
    }
}

#[cfg(test)]
mod tests;
