//! Connection-owned speech conversations and lossless dictation streams.

pub mod audio;
pub mod capabilities;
pub mod connection;
pub mod dispatch;
pub mod local;
pub mod openai;
pub mod ports;
pub mod protocol;
pub mod service;

/// Safe speech failures; provider response bodies, paths and credentials never cross the wire.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum Error {
    /// Invalid audio, identifier, sequence or payload.
    #[error("Invalid speech parameters or audio")]
    Invalid,
    /// The selected speech backend or required model is not configured.
    #[error("Speech backend or model is unavailable")]
    Unavailable,
    /// Per-connection or process-wide admission limit.
    #[error("Speech resource budget exhausted")]
    Capacity,
    /// Audio chunks, processing or playback exceeded their deadline.
    #[error("Speech operation timed out")]
    Timeout,
    /// Provider service, child process or network I/O failed.
    #[error("Speech provider failed")]
    Provider,
    /// A request was cancelled by its owning connection.
    #[error("Speech operation cancelled")]
    Cancelled,
    /// Native Agent operation failed.
    #[error("Voice Agent operation failed")]
    Agent,
}

impl Error {
    /// Stable reason code suitable for client diagnostics.
    #[must_use]
    pub fn reason(self) -> &'static str {
        match self {
            Self::Invalid => "invalid_audio_or_stream",
            Self::Unavailable => "speech_backend_unavailable",
            Self::Capacity => "speech_resource_exhausted",
            Self::Timeout => "speech_timeout",
            Self::Provider => "speech_provider_failed",
            Self::Cancelled => "speech_cancelled",
            Self::Agent => "voice_agent_failed",
        }
    }

    /// Whether retrying after a transient condition may succeed.
    #[must_use]
    pub fn retryable(self) -> bool {
        matches!(self, Self::Capacity | Self::Timeout | Self::Provider)
    }
}

impl From<Error> for server_model::ErrorCode {
    fn from(error: Error) -> Self {
        match error {
            Error::Invalid => Self::InvalidMessage,
            Error::Capacity => Self::ResourceExhausted,
            Error::Agent => Self::AgentIo,
            Error::Unavailable | Error::Timeout | Error::Provider | Error::Cancelled => {
                Self::SpeechIo
            }
        }
    }
}

#[cfg(test)]
mod tests;
