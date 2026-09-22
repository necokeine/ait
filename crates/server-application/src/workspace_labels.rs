//! Workspace label coordination, sequencing, and durable assignment rewrites.

use std::collections::{BTreeMap, VecDeque};
use std::fmt;
use std::sync::{Arc, Mutex, Weak};

use server_domain::registry::PersistedWorkspaceRecord;
use server_domain::workspace_labels::{
    WorkspaceLabelColor, WorkspaceLabelDefinition, normalize_workspace_label_name,
    workspace_label_key,
};
use server_ports::workspace_labels::{
    WorkspaceLabelStore, WorkspaceLabelStoreError, WorkspaceLabelStoreMutation,
};

const JOURNAL_LIMIT: usize = 256;

/// Incremental synchronization cursor.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorkspaceLabelCursor {
    /// Process generation returned by an earlier synchronization.
    pub generation: String,
    /// Last sequence observed by the client.
    pub after_seq: u64,
}

/// Whether a synchronization contains a full catalog or compacted changes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WorkspaceLabelSyncMode {
    /// Complete current catalog.
    Snapshot,
    /// Changes strictly after the supplied cursor.
    Changes,
}

/// One compacted removal in a catch-up response.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorkspaceLabelRemoval {
    /// Removed display name.
    pub name: String,
    /// Sequence that removed or renamed the label.
    pub seq: u64,
}

/// Sequencing metadata for a label list response.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorkspaceLabelSyncMetadata {
    /// Snapshot or catch-up mode.
    pub mode: WorkspaceLabelSyncMode,
    /// Current process generation.
    pub generation: String,
    /// Sequence at the synchronization boundary.
    pub head_seq: u64,
    /// Compacted removals for change mode.
    pub removals: Vec<WorkspaceLabelRemoval>,
}

/// Label list plus synchronization metadata.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorkspaceLabelSync {
    /// Full snapshot or compacted upserts.
    pub labels: Vec<WorkspaceLabelDefinition>,
    /// Synchronization boundary.
    pub sync: WorkspaceLabelSyncMetadata,
}

/// Durable live label change published after storage commit.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WorkspaceLabelChange {
    /// Definition creation or edit.
    Upsert {
        /// Current definition.
        label: WorkspaceLabelDefinition,
        /// Previous display name for a rename.
        previous_name: Option<String>,
    },
    /// Definition deletion.
    Remove {
        /// Deleted display name.
        name: String,
    },
}

/// Live change with process generation and monotonic sequence.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SequencedWorkspaceLabelChange {
    /// Process generation.
    pub generation: String,
    /// Positive sequence number.
    pub seq: u64,
    /// Catalog change.
    pub change: WorkspaceLabelChange,
}

/// Successful assignment result.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorkspaceLabelAssignment {
    /// Authoritative catalog definition.
    pub label: WorkspaceLabelDefinition,
    /// Complete assignment names for the workspace.
    pub workspace_labels: Vec<String>,
}

/// Successful edit result.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorkspaceLabelEdit {
    /// Updated definition.
    pub label: WorkspaceLabelDefinition,
    /// Workspaces whose name assignment was rewritten.
    pub affected_workspace_count: usize,
}

/// Business or persistence failure.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum WorkspaceLabelError {
    /// Normalized name is empty.
    #[error("label name cannot be empty")]
    NameEmpty,
    /// Requested catalog definition is absent.
    #[error("label not found")]
    LabelNotFound,
    /// A different definition already owns the requested normalized name.
    #[error("a label with that name already exists")]
    NameTaken,
    /// Assignment target is missing or archived.
    #[error("workspace not found")]
    WorkspaceNotFound,
    /// Storage failed before commit.
    #[error("workspace label storage failed")]
    Storage,
    /// Storage could not establish whether the compound mutation committed.
    #[error("workspace label storage outcome is uncertain; restart before retrying")]
    StorageUncertain,
}

/// Callback invoked after one durable catalog change.
pub type WorkspaceLabelListener = Arc<dyn Fn(SequencedWorkspaceLabelChange) + Send + Sync>;

struct SequenceState {
    generation: String,
    head_seq: u64,
    journal: VecDeque<SequencedWorkspaceLabelChange>,
    next_listener: u64,
    listeners: BTreeMap<u64, WorkspaceLabelListener>,
}

impl fmt::Debug for SequenceState {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("SequenceState")
            .field("generation", &self.generation)
            .field("head_seq", &self.head_seq)
            .field("journal_len", &self.journal.len())
            .field("next_listener", &self.next_listener)
            .field("listeners", &self.listeners.len())
            .finish()
    }
}

/// RAII connection-owned label subscription.
pub struct WorkspaceLabelSubscription {
    sequence: Weak<Mutex<SequenceState>>,
    listener_id: u64,
}

impl fmt::Debug for WorkspaceLabelSubscription {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("WorkspaceLabelSubscription")
            .field("listener_id", &self.listener_id)
            .finish_non_exhaustive()
    }
}

impl Drop for WorkspaceLabelSubscription {
    fn drop(&mut self) {
        let Some(sequence) = self.sequence.upgrade() else {
            return;
        };
        sequence
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .listeners
            .remove(&self.listener_id);
    }
}

/// Serialized workspace label use cases.
#[derive(Debug)]
pub struct WorkspaceLabels {
    store: Box<dyn WorkspaceLabelStore>,
    operations: Mutex<()>,
    sequence: Arc<Mutex<SequenceState>>,
}

impl WorkspaceLabels {
    /// Construct a label coordinator with a fresh process generation.
    ///
    /// # Errors
    /// Returns storage initialization or entropy failures.
    pub fn new(store: Box<dyn WorkspaceLabelStore>) -> Result<Self, WorkspaceLabelError> {
        store.initialize().map_err(map_store_error)?;
        Ok(Self {
            store,
            operations: Mutex::new(()),
            sequence: Arc::new(Mutex::new(SequenceState {
                generation: generate_generation()?,
                head_seq: 0,
                journal: VecDeque::new(),
                next_listener: 1,
                listeners: BTreeMap::new(),
            })),
        })
    }

    /// Synchronize the label catalog without subscribing.
    ///
    /// # Errors
    /// Returns storage failures.
    pub fn list(
        &self,
        cursor: Option<&WorkspaceLabelCursor>,
    ) -> Result<WorkspaceLabelSync, WorkspaceLabelError> {
        let _operation = self.operation();
        let snapshot = self.store.snapshot().map_err(map_store_error)?;
        Ok(self.synchronize(&snapshot.labels, cursor))
    }

    /// Atomically subscribe and return a synchronization boundary.
    ///
    /// # Errors
    /// Returns storage failures.
    pub fn subscribe(
        &self,
        cursor: Option<&WorkspaceLabelCursor>,
        listener: WorkspaceLabelListener,
    ) -> Result<(WorkspaceLabelSync, WorkspaceLabelSubscription), WorkspaceLabelError> {
        let _operation = self.operation();
        let snapshot = self.store.snapshot().map_err(map_store_error)?;
        let mut sequence = self.sequence();
        let listener_id = sequence.next_listener;
        sequence.next_listener = sequence.next_listener.saturating_add(1);
        sequence.listeners.insert(listener_id, listener);
        let sync = synchronize(&sequence, &snapshot.labels, cursor);
        drop(sequence);
        Ok((
            sync,
            WorkspaceLabelSubscription {
                sequence: Arc::downgrade(&self.sequence),
                listener_id,
            },
        ))
    }

    /// Assign or unassign a definition, creating it only for a true assignment.
    ///
    /// # Errors
    /// Returns validation, missing-workspace, or storage errors.
    pub fn set_assignment(
        &self,
        workspace_id: &str,
        label: &WorkspaceLabelDefinition,
        assigned: bool,
        updated_at: &str,
    ) -> Result<WorkspaceLabelAssignment, WorkspaceLabelError> {
        let _operation = self.operation();
        let name = require_name(&label.name)?;
        let snapshot = self.store.snapshot().map_err(map_store_error)?;
        let workspace = snapshot
            .workspaces
            .iter()
            .find(|workspace| workspace.workspace_id == workspace_id)
            .filter(|workspace| !is_archived(workspace))
            .ok_or(WorkspaceLabelError::WorkspaceNotFound)?;
        let key = workspace_label_key(&name);
        let existing = snapshot
            .labels
            .iter()
            .find(|candidate| workspace_label_key(&candidate.name) == key)
            .cloned();
        let definition = existing.clone().unwrap_or(WorkspaceLabelDefinition {
            name,
            color: label.color,
        });
        let current = workspace.labels.clone().unwrap_or_default();
        let workspace_labels = update_assignment(&current, &key, &definition.name, assigned);
        let workspace_changed = workspace_labels != current;
        let catalog_changed = existing.is_none() && assigned;
        if workspace_changed || catalog_changed {
            let mut updated = workspace.clone();
            updated.labels = (!workspace_labels.is_empty()).then(|| workspace_labels.clone());
            updated_at.clone_into(&mut updated.updated_at);
            let labels = if catalog_changed {
                let mut labels = snapshot.labels.clone();
                labels.push(definition.clone());
                labels
            } else {
                snapshot.labels.clone()
            };
            self.store
                .commit(&WorkspaceLabelStoreMutation {
                    expected_labels: snapshot.labels,
                    labels,
                    workspace_updates: workspace_changed.then_some(updated).into_iter().collect(),
                })
                .map_err(map_store_error)?;
            if catalog_changed {
                self.publish(WorkspaceLabelChange::Upsert {
                    label: definition.clone(),
                    previous_name: None,
                });
            }
        }
        Ok(WorkspaceLabelAssignment {
            label: definition,
            workspace_labels,
        })
    }

    /// Edit a name and color as one catalog transaction.
    ///
    /// # Errors
    /// Returns empty, missing, collision, or storage errors.
    pub fn update(
        &self,
        name: &str,
        new_name: Option<&str>,
        color: Option<WorkspaceLabelColor>,
        updated_at: &str,
    ) -> Result<WorkspaceLabelEdit, WorkspaceLabelError> {
        let _operation = self.operation();
        let from_key = workspace_label_key(&require_name(name)?);
        let new_name = new_name.map(require_name).transpose()?;
        let snapshot = self.store.snapshot().map_err(map_store_error)?;
        let index = snapshot
            .labels
            .iter()
            .position(|label| workspace_label_key(&label.name) == from_key)
            .ok_or(WorkspaceLabelError::LabelNotFound)?;
        let existing = snapshot.labels[index].clone();
        if let Some(new_name) = &new_name
            && workspace_label_key(new_name) != from_key
            && snapshot
                .labels
                .iter()
                .any(|label| workspace_label_key(&label.name) == workspace_label_key(new_name))
        {
            return Err(WorkspaceLabelError::NameTaken);
        }
        let name_changed = new_name
            .as_ref()
            .is_some_and(|new_name| *new_name != existing.name);
        let color_changed = color.is_some_and(|color| color != existing.color);
        if !name_changed && !color_changed {
            return Ok(WorkspaceLabelEdit {
                label: existing,
                affected_workspace_count: 0,
            });
        }
        let definition = WorkspaceLabelDefinition {
            name: new_name.clone().unwrap_or_else(|| existing.name.clone()),
            color: color.unwrap_or(existing.color),
        };
        let workspace_updates = if name_changed {
            rewrite_assignments(
                &snapshot.workspaces,
                &from_key,
                Some(&definition.name),
                updated_at,
            )
        } else {
            Vec::new()
        };
        let affected_workspace_count = workspace_updates.len();
        let mut labels = snapshot.labels.clone();
        labels[index] = definition.clone();
        self.store
            .commit(&WorkspaceLabelStoreMutation {
                expected_labels: snapshot.labels,
                labels,
                workspace_updates,
            })
            .map_err(map_store_error)?;
        self.publish(WorkspaceLabelChange::Upsert {
            label: definition.clone(),
            previous_name: name_changed.then_some(existing.name),
        });
        Ok(WorkspaceLabelEdit {
            label: definition,
            affected_workspace_count,
        })
    }

    /// Delete a definition and remove its assignment from active and archived workspaces.
    ///
    /// # Errors
    /// Returns empty-name or storage errors. Missing labels are idempotent.
    pub fn delete(&self, name: &str, updated_at: &str) -> Result<usize, WorkspaceLabelError> {
        let _operation = self.operation();
        let key = workspace_label_key(&require_name(name)?);
        let snapshot = self.store.snapshot().map_err(map_store_error)?;
        let Some(existing) = snapshot
            .labels
            .iter()
            .find(|label| workspace_label_key(&label.name) == key)
            .cloned()
        else {
            return Ok(0);
        };
        let workspace_updates = rewrite_assignments(&snapshot.workspaces, &key, None, updated_at);
        let affected_workspace_count = workspace_updates.len();
        let labels = snapshot
            .labels
            .iter()
            .filter(|label| workspace_label_key(&label.name) != key)
            .cloned()
            .collect();
        self.store
            .commit(&WorkspaceLabelStoreMutation {
                expected_labels: snapshot.labels,
                labels,
                workspace_updates,
            })
            .map_err(map_store_error)?;
        self.publish(WorkspaceLabelChange::Remove {
            name: existing.name,
        });
        Ok(affected_workspace_count)
    }

    /// Count active and archived assignments without mutating storage.
    ///
    /// # Errors
    /// Returns empty-name or storage errors.
    pub fn inspect_delete(&self, name: &str) -> Result<usize, WorkspaceLabelError> {
        let _operation = self.operation();
        let key = workspace_label_key(&require_name(name)?);
        let snapshot = self.store.snapshot().map_err(map_store_error)?;
        Ok(snapshot
            .workspaces
            .iter()
            .filter(|workspace| workspace_has_label(workspace, &key))
            .count())
    }

    fn operation(&self) -> std::sync::MutexGuard<'_, ()> {
        self.operations
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    fn sequence(&self) -> std::sync::MutexGuard<'_, SequenceState> {
        self.sequence
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    fn synchronize(
        &self,
        labels: &[WorkspaceLabelDefinition],
        cursor: Option<&WorkspaceLabelCursor>,
    ) -> WorkspaceLabelSync {
        synchronize(&self.sequence(), labels, cursor)
    }

    fn publish(&self, change: WorkspaceLabelChange) {
        let (entry, listeners) = {
            let mut sequence = self.sequence();
            sequence.head_seq = sequence.head_seq.saturating_add(1);
            let entry = SequencedWorkspaceLabelChange {
                generation: sequence.generation.clone(),
                seq: sequence.head_seq,
                change,
            };
            sequence.journal.push_back(entry.clone());
            while sequence.journal.len() > JOURNAL_LIMIT {
                sequence.journal.pop_front();
            }
            let listeners = sequence.listeners.values().cloned().collect::<Vec<_>>();
            (entry, listeners)
        };
        for listener in listeners {
            let entry = entry.clone();
            let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| listener(entry)));
        }
    }
}

fn synchronize(
    sequence: &SequenceState,
    labels: &[WorkspaceLabelDefinition],
    cursor: Option<&WorkspaceLabelCursor>,
) -> WorkspaceLabelSync {
    let oldest_seq = sequence
        .journal
        .front()
        .map_or(sequence.head_seq.saturating_add(1), |entry| entry.seq);
    let can_catch_up = cursor.is_some_and(|cursor| {
        cursor.generation == sequence.generation
            && cursor.after_seq <= sequence.head_seq
            && cursor.after_seq >= oldest_seq.saturating_sub(1)
    });
    if !can_catch_up {
        return WorkspaceLabelSync {
            labels: labels.to_vec(),
            sync: WorkspaceLabelSyncMetadata {
                mode: WorkspaceLabelSyncMode::Snapshot,
                generation: sequence.generation.clone(),
                head_seq: sequence.head_seq,
                removals: Vec::new(),
            },
        };
    }
    let after_seq = cursor.map_or(0, |cursor| cursor.after_seq);
    let mut upserts = BTreeMap::<String, WorkspaceLabelDefinition>::new();
    let mut removals = BTreeMap::<String, WorkspaceLabelRemoval>::new();
    for entry in sequence
        .journal
        .iter()
        .filter(|entry| entry.seq > after_seq)
    {
        match &entry.change {
            WorkspaceLabelChange::Remove { name } => {
                let key = name.to_lowercase();
                let created_during_catch_up = upserts.remove(&key).is_some();
                if !created_during_catch_up {
                    removals.insert(
                        key,
                        WorkspaceLabelRemoval {
                            name: name.clone(),
                            seq: entry.seq,
                        },
                    );
                }
            }
            WorkspaceLabelChange::Upsert {
                label,
                previous_name,
            } => {
                if let Some(previous_name) = previous_name {
                    let previous_key = previous_name.to_lowercase();
                    let created_during_catch_up = upserts.remove(&previous_key).is_some();
                    if !created_during_catch_up {
                        removals.insert(
                            previous_key,
                            WorkspaceLabelRemoval {
                                name: previous_name.clone(),
                                seq: entry.seq,
                            },
                        );
                    }
                }
                let key = label.name.to_lowercase();
                removals.remove(&key);
                upserts.insert(key, label.clone());
            }
        }
    }
    WorkspaceLabelSync {
        labels: upserts.into_values().collect(),
        sync: WorkspaceLabelSyncMetadata {
            mode: WorkspaceLabelSyncMode::Changes,
            generation: sequence.generation.clone(),
            head_seq: sequence.head_seq,
            removals: removals.into_values().collect(),
        },
    }
}

fn require_name(name: &str) -> Result<String, WorkspaceLabelError> {
    let name = normalize_workspace_label_name(name);
    if name.is_empty() {
        Err(WorkspaceLabelError::NameEmpty)
    } else {
        Ok(name)
    }
}

fn update_assignment(
    current: &[String],
    key: &str,
    display_name: &str,
    assigned: bool,
) -> Vec<String> {
    let assigned_index = current
        .iter()
        .position(|label| workspace_label_key(label) == key);
    match (assigned, assigned_index) {
        (true, None) => current
            .iter()
            .cloned()
            .chain(std::iter::once(display_name.to_owned()))
            .collect(),
        (false, Some(index)) => current
            .iter()
            .enumerate()
            .filter(|(candidate, _)| *candidate != index)
            .map(|(_, label)| label.clone())
            .collect(),
        _ => current.to_vec(),
    }
}

fn workspace_has_label(workspace: &PersistedWorkspaceRecord, key: &str) -> bool {
    workspace
        .labels
        .as_ref()
        .is_some_and(|labels| labels.iter().any(|label| workspace_label_key(label) == key))
}

fn rewrite_assignments(
    workspaces: &[PersistedWorkspaceRecord],
    from_key: &str,
    to: Option<&str>,
    updated_at: &str,
) -> Vec<PersistedWorkspaceRecord> {
    workspaces
        .iter()
        .filter_map(|workspace| {
            if !workspace_has_label(workspace, from_key) {
                return None;
            }
            let labels = workspace
                .labels
                .as_ref()
                .into_iter()
                .flatten()
                .filter_map(|label| {
                    if workspace_label_key(label) == from_key {
                        to.map(str::to_owned)
                    } else {
                        Some(label.clone())
                    }
                })
                .collect::<Vec<_>>();
            let mut updated = workspace.clone();
            updated.labels = (!labels.is_empty()).then_some(labels);
            updated_at.clone_into(&mut updated.updated_at);
            Some(updated)
        })
        .collect()
}

fn is_archived(workspace: &PersistedWorkspaceRecord) -> bool {
    workspace
        .archived_at
        .as_deref()
        .is_some_and(|archived_at| !archived_at.is_empty())
}

const fn map_store_error(error: WorkspaceLabelStoreError) -> WorkspaceLabelError {
    match error {
        WorkspaceLabelStoreError::Uncertain => WorkspaceLabelError::StorageUncertain,
        WorkspaceLabelStoreError::Invalid
        | WorkspaceLabelStoreError::Conflict
        | WorkspaceLabelStoreError::Io => WorkspaceLabelError::Storage,
    }
}

fn generate_generation() -> Result<String, WorkspaceLabelError> {
    let mut bytes = [0_u8; 16];
    getrandom::fill(&mut bytes).map_err(|_| WorkspaceLabelError::Storage)?;
    let encoded = bytes
        .iter()
        .flat_map(|byte| [byte >> 4, byte & 0x0f])
        .map(|nibble| char::from(b"0123456789abcdef"[usize::from(nibble)]))
        .collect::<String>();
    Ok(format!(
        "{}-{}-{}-{}-{}",
        &encoded[0..8],
        &encoded[8..12],
        &encoded[12..16],
        &encoded[16..20],
        &encoded[20..32]
    ))
}

#[cfg(test)]
mod tests;
