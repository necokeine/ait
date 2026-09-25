//! Non-secret Agent configuration and explicit catalog-default DTOs.

use serde::{Deserialize, Serialize};

/// Configuration capabilities only; no provider/model validation or execution is advertised.
pub const CAPABILITIES: &[&str] = &[
    "agent.configure",
    "agent.get",
    "agent.list",
    "agent.default.get",
    "agent.default.set",
];

/// Complete preset configuration. Unknown fields, including raw credential fields, are rejected.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Config {
    /// Non-secret display name, 1–255 UTF-8 bytes, excluding controls or all-whitespace names.
    pub name: String,
    /// Configuration schema; currently only `codex`.
    pub driver_type: String,
    /// Explicit model identifier, 1–128 restricted ASCII bytes; not discovered or validated remotely.
    pub model: String,
    /// Optional `env:AIT_SERVER_CREDENTIAL_<NAME>` reference; never a credential value.
    #[serde(default)]
    pub credential_ref: Option<String>,
    /// Eligibility for selection; does not imply an executable adapter is installed.
    pub enabled: bool,
}

/// Create a preset or append an immutable full replacement to an existing preset.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Configure {
    /// Absent for creation; supply together with `expected_revision` for replacement.
    #[serde(default)]
    pub agent_id: Option<String>,
    /// Observed head revision; absent for creation, positive for replacement.
    #[serde(default)]
    pub expected_revision: Option<u64>,
    /// Complete replacement configuration.
    pub config: Config,
    /// Method-scoped durable retry key, 1–128 visible ASCII bytes.
    pub idempotency_key: String,
}

/// Read a current or historical immutable configuration.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Get {
    /// Stable Agent UUID.
    pub agent_id: String,
    /// Absent for the current head, otherwise the exact positive revision.
    #[serde(default)]
    pub revision: Option<u64>,
}

/// Stable-ID keyset pagination of current configurations.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct List {
    /// Exclusive Agent UUID cursor.
    #[serde(default)]
    pub after: Option<String>,
    /// Between 1 and 50; defaults to 20.
    #[serde(default = "default_limit")]
    pub limit: usize,
}

fn default_limit() -> usize {
    20
}

/// Empty parameters for reading the explicit catalog default.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct GetDefault {}

/// Replace the explicit default; every field is required, including a null clear target.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SetDefault {
    /// Agent UUID or explicit null to clear; omission is rejected.
    #[serde(deserialize_with = "required_optional")]
    pub agent_id: Option<String>,
    /// Last observed default version, initially zero.
    pub expected_version: u64,
    /// Durable key scoped to this method.
    pub idempotency_key: String,
}

fn required_optional<'de, D: serde::Deserializer<'de>>(
    deserializer: D,
) -> Result<Option<String>, D::Error> {
    Option::deserialize(deserializer)
}

/// Stable configuration receipt; later edits never rewrite it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Receipt {
    /// Durable operation UUID.
    pub operation_id: String,
    /// Created or edited preset UUID.
    pub agent_id: String,
    /// Exact immutable revision produced by the operation.
    pub revision: u64,
}

/// Immutable non-secret configuration revision.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Agent {
    /// Stable preset identity.
    pub agent_id: String,
    /// Exact revision number.
    pub revision: u64,
    /// Frozen fields, containing references only.
    pub config: Config,
    /// Revision creation time in Unix epoch milliseconds.
    pub recorded_at: u64,
}

/// Bounded current-configuration page.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Page {
    /// Current heads in stable UUID order.
    pub agents: Vec<Agent>,
    /// Exclusive cursor; a full last page may be followed by an empty page.
    pub next_after: Option<String>,
}

/// Current explicit catalog selection, or empty at initial version zero.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DefaultSelection {
    /// Selected Agent UUID or null.
    pub agent_id: Option<String>,
    /// Compare-and-swap version for the next selection change.
    pub version: u64,
}

/// Stable selection receipt; it is not a read of the current default.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DefaultReceipt {
    /// Durable operation UUID.
    pub operation_id: String,
    /// Selection committed by this operation.
    pub selection: DefaultSelection,
}
