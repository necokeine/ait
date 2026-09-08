#![allow(missing_docs)]

use async_trait::async_trait;
use serde_json::Value;

/// A durable control-plane entity family.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub enum ControlRecordKind {
    Project,
    Agent,
    Provider,
    ProviderCredential,
    RunCredential,
    Session,
    Message,
    Run,
    WorkspaceRunJournal,
    Cron,
    Settings,
}

/// One bounded record selection. Filters in a read are combined with union semantics.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ControlFilter {
    pub kind: ControlRecordKind,
    pub id: Option<String>,
    pub project_id: Option<String>,
}

impl ControlFilter {
    #[must_use]
    pub const fn all(kind: ControlRecordKind) -> Self {
        Self {
            kind,
            id: None,
            project_id: None,
        }
    }

    #[must_use]
    pub fn id(kind: ControlRecordKind, id: impl Into<String>) -> Self {
        Self {
            kind,
            id: Some(id.into()),
            project_id: None,
        }
    }

    #[must_use]
    pub fn project(kind: ControlRecordKind, project_id: impl Into<String>) -> Self {
        Self {
            kind,
            id: None,
            project_id: Some(project_id.into()),
        }
    }
}

/// One independently stored, versioned application record.
#[derive(Clone, Debug, PartialEq)]
pub struct ControlRecord {
    pub kind: ControlRecordKind,
    pub id: String,
    pub project_id: Option<String>,
    pub value: Value,
}

/// A bounded record read and the database revision observed with it.
#[derive(Clone, Debug, PartialEq)]
pub struct ControlRead {
    pub revision: u64,
    pub records: Vec<ControlRecord>,
}

/// One row-level change committed atomically with durable events.
#[derive(Clone, Debug, PartialEq)]
pub enum ControlChange {
    Put(ControlRecord),
    Delete { kind: ControlRecordKind, id: String },
}

/// Event to append atomically with an entity change.
#[derive(Clone, Debug, PartialEq)]
pub struct PendingEvent {
    pub kind: String,
    pub entity_id: Option<String>,
    pub body: Value,
    pub created_at: i64,
}

/// Durable event returned to a reconnecting client.
#[derive(Clone, Debug, PartialEq)]
pub struct DurableEvent {
    pub cursor: u64,
    pub kind: String,
    pub entity_id: Option<String>,
    pub body: Value,
    pub created_at: i64,
}

/// Retained cursor range used to detect an expired or future reconnect cursor.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct EventBounds {
    pub oldest: Option<u64>,
    pub latest: Option<u64>,
}

/// One cursor-validated replay page read at a single observed revision.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct DurableEventPage {
    pub bounds: EventBounds,
    pub events: Vec<DurableEvent>,
    pub cursor_valid: bool,
}

/// Latest bounded display projection for one active Run.
#[derive(Clone, Debug, PartialEq)]
pub struct ProgressCheckpoint {
    pub run_id: String,
    pub body: Value,
    pub updated_at: i64,
}

/// Failures exposed by control-plane persistence adapters.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ControlStoreError {
    Conflict,
    Other(String),
}

impl std::fmt::Display for ControlStoreError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Conflict => formatter.write_str("control record conflict"),
            Self::Other(message) => formatter.write_str(message),
        }
    }
}

impl std::error::Error for ControlStoreError {}

/// Record-oriented persistence seam for the local control plane and event outbox.
#[async_trait]
pub trait ControlStore: Send + Sync {
    /// Reads only records selected by the supplied filters.
    async fn read(&self, filters: &[ControlFilter]) -> Result<ControlRead, ControlStoreError>;

    /// Applies row-level changes if `expected_revision` is still current.
    async fn apply(
        &self,
        expected_revision: u64,
        changes: Vec<ControlChange>,
        events: Vec<PendingEvent>,
    ) -> Result<u64, ControlStoreError>;

    async fn replay(
        &self,
        cursor: u64,
        limit: usize,
    ) -> Result<Vec<DurableEvent>, ControlStoreError>;

    async fn event_bounds(&self) -> Result<EventBounds, ControlStoreError>;

    async fn replay_page(
        &self,
        cursor: u64,
        limit: usize,
    ) -> Result<DurableEventPage, ControlStoreError>;

    async fn save_progress(
        &self,
        checkpoint: ProgressCheckpoint,
        events: Vec<PendingEvent>,
    ) -> Result<(), ControlStoreError>;

    async fn load_progress(&self) -> Result<Vec<ProgressCheckpoint>, ControlStoreError>;

    async fn clear_progress(&self, run_id: &str) -> Result<(), ControlStoreError>;
}
