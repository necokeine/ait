//! Paseo-compatible schedule records and canonical methods.
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::Value;

/// The nine supported client requests.
pub const CAPABILITIES: &[&str] = &[
    "schedule.create.request",
    "schedule.list.request",
    "schedule.inspect.request",
    "schedule.logs.request",
    "schedule.update.request",
    "schedule.pause.request",
    "schedule.resume.request",
    "schedule.delete.request",
    "schedule.run_once.request",
];

/// Fixed interval or five-field cron cadence.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(
    tag = "type",
    rename_all = "lowercase",
    rename_all_fields = "camelCase"
)]
pub enum Cadence {
    /// Rolling interval, retained for upstream compatibility.
    Every {
        /// Positive interval in milliseconds.
        every_ms: i64,
    },
    /// Calendar cadence; day-of-month and weekday must both match.
    Cron {
        /// Five numeric cron fields.
        expression: String,
        /// IANA timezone; omitted means UTC.
        timezone: Option<String>,
    },
}

/// Existing Agent or a new Agent for each occurrence.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all_fields = "camelCase")]
pub enum Target {
    /// Persist self targets as explicit Agent targets, matching the upstream WS adapter.
    #[serde(rename = "agent", alias = "self")]
    Agent {
        /// Registered Agent UUID.
        agent_id: String,
    },
    /// Create a dedicated Workspace and Agent for every execution.
    #[serde(rename = "new-agent")]
    NewAgent {
        /// Provider configuration supplied to the independent host adapter.
        config: Value,
    },
}

/// Schedule admission state.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Status {
    /// Eligible for automatic ticks.
    Active,
    /// Only explicit manual runs may start.
    Paused,
    /// Terminal; cannot resume or run manually.
    Completed,
}

/// Durable run state.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum RunStatus {
    /// Accepted but not settled.
    Running,
    /// Runner finished successfully.
    Succeeded,
    /// Runner failed or was interrupted.
    Failed,
}

/// One occurrence and its final output.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Run {
    /// Unique occurrence UUID.
    pub id: String,
    /// Intended fire time.
    pub scheduled_for: DateTime<Utc>,
    /// Actual admission time.
    pub started_at: DateTime<Utc>,
    /// Settlement time.
    pub ended_at: Option<DateTime<Utc>>,
    /// Execution state.
    pub status: RunStatus,
    /// Actual Agent, if created or selected.
    pub agent_id: Option<String>,
    /// Dedicated Workspace, when one was created.
    pub workspace_id: Option<String>,
    /// Final textual output.
    pub output: Option<String>,
    /// Safe failure explanation.
    pub error: Option<String>,
}

/// Persistent upstream-shaped schedule document.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Schedule {
    /// Stable schedule UUID.
    pub id: String,
    /// Optional display name.
    pub name: Option<String>,
    /// Prompt submitted for every occurrence.
    pub prompt: String,
    /// Trigger cadence.
    pub cadence: Cadence,
    /// Execution destination.
    pub target: Target,
    /// Admission state.
    pub status: Status,
    /// Creation time.
    pub created_at: DateTime<Utc>,
    /// Last persisted change.
    pub updated_at: DateTime<Utc>,
    /// Next automatic occurrence.
    pub next_run_at: Option<DateTime<Utc>>,
    /// Last settlement time.
    pub last_run_at: Option<DateTime<Utc>>,
    /// Most recent pause time.
    pub paused_at: Option<DateTime<Utc>>,
    /// Optional automatic expiry.
    pub expires_at: Option<DateTime<Utc>>,
    /// Optional completed-run limit.
    pub max_runs: Option<u64>,
    /// Append-and-settle occurrence history.
    pub runs: Vec<Run>,
}

/// Input accepted by create; unknown fields are rejected before any write.
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Create {
    /// Optional display name.
    pub name: Option<String>,
    /// Nonempty prompt.
    pub prompt: String,
    /// Schedule cadence.
    pub cadence: Cadence,
    /// Existing Agent or new-Agent configuration.
    pub target: Target,
    /// Completed-run limit.
    pub max_runs: Option<u64>,
    /// Automatic expiry.
    pub expires_at: Option<DateTime<Utc>>,
    /// Defaults to true for intervals and false for cron.
    pub run_on_create: Option<bool>,
}
