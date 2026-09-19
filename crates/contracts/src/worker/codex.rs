//! Worker-owned Codex operations, including requests that do not belong to a Run.
use ait_domain::{AgentConfiguration, AgentProvider};
use serde::{Deserialize, Serialize};

/// Largest chunk of a serialized result; history never needs to fit one IPC frame.
pub const CHUNK_BYTES: usize = 16_384;
/// Aggregate result bound, enforced before allocating peer-controlled history.
pub const MAX_RESULT_BYTES: usize = 64 * 1024 * 1024;

/// Work assigned to the sole owner of a Codex app-server process.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Operation {
    /// Hold a writer until the daemon explicitly admits input or closes it.
    Open {
        /// Stable input correlation identity.
        request_id: String,
        /// Existing native Thread identity; absent for a new persistent Thread.
        thread_id: Option<String>,
        /// New user input only.
        prompt: String,
        /// Fixed model.
        model: String,
        /// Optional reasoning override.
        reasoning_effort: Option<String>,
        /// Developer instructions used only when creating a Thread.
        developer_instructions: Option<String>,
    },
    /// Enumerate archived and active history for these native source categories.
    List {
        /// Native source categories from the pinned schema.
        source_kinds: Vec<String>,
    },
    /// Read a complete native Thread without acquiring a writer.
    Read {
        /// Stable native Thread identity.
        thread_id: String,
    },
    /// Discover picker-visible models.
    Models {
        /// Host-authenticated Codex provider.
        provider: AgentProvider,
    },
    /// Generate metadata in a read-only ephemeral Thread.
    Title {
        /// Stable auxiliary operation identity.
        request_id: String,
        /// Bounded content to summarize.
        user_prompt: String,
        /// Frozen small-agent configuration.
        config: AgentConfiguration,
        /// Resolved Codex provider.
        provider: AgentProvider,
    },
}

/// Admission commands are deliberately distinct from app-server JSON-RPC.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Action {
    /// Send the durably admitted input once.
    Start,
    /// Reread authoritative history without replaying input.
    Read,
    /// Release and reap the writer before completing the worker.
    Close,
}
