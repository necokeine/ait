use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use serde_json::Value;

/// A durable control-plane entity family.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize, Deserialize)]
pub enum ControlRecordKind {
    /// Selects the `Project` variant.
    Project,
    /// Selects the `Agent` variant.
    Agent,
    /// Selects the `Provider` variant.
    Provider,
    /// Selects the `ProviderCredential` variant.
    ProviderCredential,
    /// Selects the `RunCredential` variant.
    RunCredential,
    /// Selects the `Session` variant.
    Session,
    /// Selects the `Message` variant.
    Message,
    /// Selects the `Run` variant.
    Run,
    /// Selects the `WorkspaceRunJournal` variant.
    WorkspaceRunJournal,
    /// Selects the `Cron` variant.
    Cron,
    /// Selects the `Settings` variant.
    Settings,
}

/// One bounded record selection. Filters in a read are combined with union semantics.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ControlFilter {
    /// Selects the `All` variant.
    All(ControlRecordKind),
    /// Selects the `Id` variant.
    Id {
        /// Kind value.
        kind: ControlRecordKind,
        /// Id value.
        id: String,
    },
    /// Selects the `Project` variant.
    Project {
        /// Kind value.
        kind: ControlRecordKind,
        /// Project identifier.
        project_id: String,
    },
    /// Indexed canonical-path uniqueness lookup used by registration/import.
    ProjectWorkdir {
        /// Workdir value.
        workdir: String,
    },
    /// Selects the `MessageAncestors` variant.
    MessageAncestors {
        /// Head identifier.
        head_id: String,
    },
    /// Selects the `MessageChildren` variant.
    MessageChildren {
        /// Parent identifier.
        parent_id: String,
    },
    /// Selects the `RunsForSession` variant.
    RunsForSession {
        /// Session identifier.
        session_id: String,
    },
    /// Selects the `RunsForCron` variant.
    RunsForCron {
        /// Cron identifier.
        cron_id: String,
    },
    /// Selects the `AgentsForProvider` variant.
    AgentsForProvider {
        /// Provider identifier.
        provider_id: String,
    },
}

impl ControlFilter {
    #[must_use]
    /// Selects every record of `kind`.
    pub const fn all(kind: ControlRecordKind) -> Self {
        Self::All(kind)
    }

    #[must_use]
    /// Selects the record of `kind` with the supplied identifier.
    pub fn id(kind: ControlRecordKind, id: impl Into<String>) -> Self {
        Self::Id {
            kind,
            id: id.into(),
        }
    }

    #[must_use]
    /// Selects records of `kind` owned by the supplied Project.
    pub fn project(kind: ControlRecordKind, project_id: impl Into<String>) -> Self {
        Self::Project {
            kind,
            project_id: project_id.into(),
        }
    }

    #[must_use]
    /// Selects the ancestor path ending at the supplied message.
    pub fn message_ancestors(head_id: impl Into<String>) -> Self {
        Self::MessageAncestors {
            head_id: head_id.into(),
        }
    }

    #[must_use]
    /// Selects direct children of the supplied message.
    pub fn message_children(parent_id: impl Into<String>) -> Self {
        Self::MessageChildren {
            parent_id: parent_id.into(),
        }
    }

    #[must_use]
    /// Selects Runs owned by the supplied Session.
    pub fn runs_for_session(session_id: impl Into<String>) -> Self {
        Self::RunsForSession {
            session_id: session_id.into(),
        }
    }

    #[must_use]
    /// Selects Runs triggered by the supplied Cron.
    pub fn runs_for_cron(cron_id: impl Into<String>) -> Self {
        Self::RunsForCron {
            cron_id: cron_id.into(),
        }
    }

    #[must_use]
    /// Selects Agents configured for the supplied Provider.
    pub fn agents_for_provider(provider_id: impl Into<String>) -> Self {
        Self::AgentsForProvider {
            provider_id: provider_id.into(),
        }
    }
}

/// One independently stored, versioned application record.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ControlRecord {
    /// Kind value.
    pub kind: ControlRecordKind,
    /// Stable identifier.
    pub id: String,
    /// Project identifier.
    pub project_id: Option<String>,
    /// Value value.
    pub value: Value,
}

/// A bounded record read and the database revision observed with it.
#[derive(Clone, Debug, PartialEq)]
pub struct ControlRead {
    /// Revision value.
    pub revision: u64,
    /// Records value.
    pub records: Vec<ControlRecord>,
}

/// One row-level change committed atomically with durable events.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum ControlChange {
    /// Selects the `Put` variant.
    Put(ControlRecord),
    /// Selects the `Delete` variant.
    Delete {
        #[doc = "Kind value."]
        kind: ControlRecordKind,
        #[doc = "Id value."]
        id: String,
    },
}

/// Event to append atomically with an entity change.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct PendingEvent {
    /// Kind value.
    pub kind: String,
    /// Entity identifier.
    pub entity_id: Option<String>,
    /// Body value.
    pub body: Value,
    /// Created timestamp.
    pub created_at: i64,
}

/// Durable event returned to a reconnecting client.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct DurableEvent {
    /// Cursor value.
    pub cursor: u64,
    /// Kind value.
    pub kind: String,
    /// Entity identifier.
    pub entity_id: Option<String>,
    /// Body value.
    pub body: Value,
    /// Created timestamp.
    pub created_at: i64,
}

/// Retained cursor range used to detect an expired or future reconnect cursor.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct EventBounds {
    /// Oldest value.
    pub oldest: Option<u64>,
    /// Latest value.
    pub latest: Option<u64>,
}

/// One cursor-validated replay page read at a single observed revision.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct DurableEventPage {
    /// Bounds value.
    pub bounds: EventBounds,
    /// Events value.
    pub events: Vec<DurableEvent>,
    /// Cursor valid value.
    pub cursor_valid: bool,
}

/// Latest bounded display projection for one active Run.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ProgressCheckpoint {
    /// Run identifier.
    pub run_id: String,
    /// Body value.
    pub body: Value,
    /// Updated timestamp.
    pub updated_at: i64,
}

/// Failures exposed by control-plane persistence adapters.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ControlStoreError {
    /// Selects the `Conflict` variant.
    Conflict,
    /// Selects the `Other` variant.
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

    /// Replays at most `limit` durable events after `cursor`.
    ///
    /// # Errors
    ///
    /// Returns [`ControlStoreError`] when the replay cannot be read.
    async fn replay(
        &self,
        cursor: u64,
        limit: usize,
    ) -> Result<Vec<DurableEvent>, ControlStoreError>;

    /// Returns the oldest and latest retained event cursors.
    ///
    /// # Errors
    ///
    /// Returns [`ControlStoreError`] when the bounds cannot be read.
    async fn event_bounds(&self) -> Result<EventBounds, ControlStoreError>;

    /// Replays a cursor-validated page containing at most `limit` events.
    ///
    /// # Errors
    ///
    /// Returns [`ControlStoreError`] when the page cannot be read.
    async fn replay_page(
        &self,
        cursor: u64,
        limit: usize,
    ) -> Result<DurableEventPage, ControlStoreError>;

    /// Persists a Run progress checkpoint and its durable events atomically.
    ///
    /// # Errors
    ///
    /// Returns [`ControlStoreError`] when the transaction cannot be committed.
    async fn save_progress(
        &self,
        checkpoint: ProgressCheckpoint,
        events: Vec<PendingEvent>,
    ) -> Result<(), ControlStoreError>;

    /// Loads active Run progress checkpoints for the supplied Project.
    ///
    /// # Errors
    ///
    /// Returns [`ControlStoreError`] when the checkpoints cannot be read.
    async fn load_progress(
        &self,
        project_id: &str,
    ) -> Result<Vec<ProgressCheckpoint>, ControlStoreError>;

    /// Removes the retained progress checkpoint for `run_id`.
    ///
    /// # Errors
    ///
    /// Returns [`ControlStoreError`] when the checkpoint cannot be removed.
    async fn clear_progress(&self, run_id: &str) -> Result<(), ControlStoreError>;
}
