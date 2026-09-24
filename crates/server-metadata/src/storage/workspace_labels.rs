//! Crash-recoverable Paseo workspace label catalog and assignment store.

use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, MutexGuard};

use crate::model::registry::PersistedWorkspaceRecord;
use crate::model::workspace_labels::WorkspaceLabelDefinition;
use crate::ports::registry::{RegistryError, WorkspaceRegistry};
use crate::ports::workspace_labels::{
    WorkspaceLabelStore, WorkspaceLabelStoreError, WorkspaceLabelStoreMutation,
    WorkspaceLabelStoreSnapshot,
};
use serde::{Deserialize, Serialize};

use crate::storage::registry::FileBackedWorkspaceRegistry;

const MAX_DOCUMENT_BYTES: u64 = 4 * 1024 * 1024;

/// File-backed label catalog using Paseo's two-file transaction journal.
#[derive(Debug)]
pub struct FileWorkspaceLabelStore {
    catalog_path: PathBuf,
    transaction_path: PathBuf,
    workspaces: FileBackedWorkspaceRegistry,
    state: Mutex<State>,
}

#[derive(Debug, Default)]
struct State {
    loaded: bool,
    blocked: bool,
    labels: Vec<WorkspaceLabelDefinition>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
enum TransactionPhase {
    Prepared,
    Committed,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct WorkspaceState {
    workspace_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    labels: Option<Vec<String>>,
    updated_at: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Transaction {
    phase: TransactionPhase,
    before_labels: Vec<WorkspaceLabelDefinition>,
    after_labels: Vec<WorkspaceLabelDefinition>,
    before_workspaces: Vec<WorkspaceState>,
    after_workspaces: Vec<WorkspaceState>,
}

impl FileWorkspaceLabelStore {
    /// Use `<data-dir>/projects/workspace-labels.json` and Paseo's transaction filename.
    #[must_use]
    pub fn new(data_dir: &Path, workspaces: FileBackedWorkspaceRegistry) -> Self {
        let projects = data_dir.join("projects");
        Self {
            catalog_path: projects.join("workspace-labels.json"),
            transaction_path: projects.join("workspace-labels.transaction.json"),
            workspaces,
            state: Mutex::new(State::default()),
        }
    }

    fn state(&self) -> Result<MutexGuard<'_, State>, WorkspaceLabelStoreError> {
        self.state
            .lock()
            .map_err(|_| WorkspaceLabelStoreError::Uncertain)
    }

    fn ensure_loaded(&self, state: &mut State) -> Result<(), WorkspaceLabelStoreError> {
        if state.loaded {
            return state
                .blocked
                .then_some(WorkspaceLabelStoreError::Uncertain)
                .map_or(Ok(()), Err);
        }
        match read_optional::<Transaction>(&self.transaction_path)? {
            Some(transaction) if transaction.phase == TransactionPhase::Prepared => {
                self.recover(&transaction, state)?;
            }
            Some(transaction) => {
                state.labels =
                    read_optional(&self.catalog_path)?.ok_or(WorkspaceLabelStoreError::Invalid)?;
                validate_labels(&state.labels)?;
                let _ = fs::remove_file(&self.transaction_path);
                debug_assert_eq!(transaction.phase, TransactionPhase::Committed);
            }
            None => {
                state.labels = read_optional(&self.catalog_path)?.unwrap_or_default();
                validate_labels(&state.labels)?;
            }
        }
        state.loaded = true;
        Ok(())
    }

    fn recover(
        &self,
        transaction: &Transaction,
        state: &mut State,
    ) -> Result<(), WorkspaceLabelStoreError> {
        validate_labels(&transaction.before_labels)?;
        validate_labels(&transaction.after_labels)?;
        let current = self.workspaces.list().map_err(map_registry_error)?;
        let updates = restore_workspaces(&current, &transaction.before_workspaces);
        let catalog_path = self.catalog_path.clone();
        let labels = transaction.before_labels.clone();
        let transaction_path = self.transaction_path.clone();
        let recovered = self.workspaces.commit_workspace_label_mutation(
            &updates,
            move |_| write_json_atomic(&catalog_path, &labels).map_err(store_to_registry),
            move || remove_if_present(&transaction_path).map_err(store_to_registry),
            false,
        );
        if recovered.is_err() {
            return self.block(state);
        }
        state.labels.clone_from(&transaction.before_labels);
        Ok(())
    }

    fn resolve_failed_commit(
        &self,
        state: &mut State,
        original: WorkspaceLabelStoreError,
    ) -> Result<(), WorkspaceLabelStoreError> {
        let transaction = match read_optional::<Transaction>(&self.transaction_path) {
            Ok(Some(transaction)) => transaction,
            Ok(None) => return Err(original),
            Err(_) => return self.block(state),
        };
        if transaction.phase == TransactionPhase::Committed {
            return self.block(state);
        }
        self.recover(&transaction, state)?;
        Err(original)
    }

    fn block(&self, state: &mut State) -> Result<(), WorkspaceLabelStoreError> {
        state.blocked = true;
        let _ = self.workspaces.block_all_mutations_until_restart();
        Err(WorkspaceLabelStoreError::Uncertain)
    }
}

impl WorkspaceLabelStore for FileWorkspaceLabelStore {
    fn initialize(&self) -> Result<(), WorkspaceLabelStoreError> {
        let mut state = self.state()?;
        self.ensure_loaded(&mut state)
    }

    fn snapshot(&self) -> Result<WorkspaceLabelStoreSnapshot, WorkspaceLabelStoreError> {
        let mut state = self.state()?;
        self.ensure_loaded(&mut state)?;
        if state.blocked {
            return Err(WorkspaceLabelStoreError::Uncertain);
        }
        Ok(WorkspaceLabelStoreSnapshot {
            labels: state.labels.clone(),
            workspaces: self.workspaces.list().map_err(map_registry_error)?,
        })
    }

    fn commit(
        &self,
        mutation: &WorkspaceLabelStoreMutation,
    ) -> Result<(), WorkspaceLabelStoreError> {
        let mut state = self.state()?;
        self.ensure_loaded(&mut state)?;
        if state.blocked {
            return Err(WorkspaceLabelStoreError::Uncertain);
        }
        if state.labels != mutation.expected_labels {
            return Err(WorkspaceLabelStoreError::Conflict);
        }
        validate_labels(&mutation.labels)?;
        if state.labels == mutation.labels && mutation.workspace_updates.is_empty() {
            return Ok(());
        }
        let workspaces = self.workspaces.list().map_err(map_registry_error)?;
        let transaction = transaction_for(&state.labels, mutation, &workspaces)?;
        let committed = Transaction {
            phase: TransactionPhase::Committed,
            ..transaction.clone()
        };
        let transaction_path = self.transaction_path.clone();
        let catalog_path = self.catalog_path.clone();
        let after_labels = mutation.labels.clone();
        let committed_path = self.transaction_path.clone();
        let result = self.workspaces.commit_workspace_label_mutation(
            &mutation.workspace_updates,
            move |_| {
                write_json_atomic(&transaction_path, &transaction).map_err(store_to_registry)?;
                write_json_atomic(&catalog_path, &after_labels).map_err(store_to_registry)
            },
            move || write_json_atomic(&committed_path, &committed).map_err(store_to_registry),
            true,
        );
        if let Err(error) = result {
            return self.resolve_failed_commit(&mut state, map_registry_error(error));
        }
        state.labels.clone_from(&mutation.labels);
        let _ = fs::remove_file(&self.transaction_path);
        Ok(())
    }
}

fn transaction_for(
    current_labels: &[WorkspaceLabelDefinition],
    mutation: &WorkspaceLabelStoreMutation,
    workspaces: &[PersistedWorkspaceRecord],
) -> Result<Transaction, WorkspaceLabelStoreError> {
    let mut before_workspaces = Vec::with_capacity(mutation.workspace_updates.len());
    for update in &mutation.workspace_updates {
        let before = workspaces
            .iter()
            .find(|workspace| workspace.workspace_id == update.workspace_id)
            .ok_or(WorkspaceLabelStoreError::Invalid)?;
        before_workspaces.push(workspace_state(before));
    }
    Ok(Transaction {
        phase: TransactionPhase::Prepared,
        before_labels: current_labels.to_vec(),
        after_labels: mutation.labels.clone(),
        before_workspaces,
        after_workspaces: mutation
            .workspace_updates
            .iter()
            .map(workspace_state)
            .collect(),
    })
}

fn workspace_state(workspace: &PersistedWorkspaceRecord) -> WorkspaceState {
    WorkspaceState {
        workspace_id: workspace.workspace_id.clone(),
        labels: workspace.labels.clone(),
        updated_at: workspace.updated_at.clone(),
    }
}

fn restore_workspaces(
    workspaces: &[PersistedWorkspaceRecord],
    states: &[WorkspaceState],
) -> Vec<PersistedWorkspaceRecord> {
    states
        .iter()
        .filter_map(|state| {
            let mut workspace = workspaces
                .iter()
                .find(|workspace| workspace.workspace_id == state.workspace_id)?
                .clone();
            workspace.labels.clone_from(&state.labels);
            workspace.updated_at.clone_from(&state.updated_at);
            Some(workspace)
        })
        .collect()
}

fn validate_labels(labels: &[WorkspaceLabelDefinition]) -> Result<(), WorkspaceLabelStoreError> {
    let bytes = serde_json::to_vec(labels).map_err(|_| WorkspaceLabelStoreError::Invalid)?;
    serde_json::from_slice::<Vec<WorkspaceLabelDefinition>>(&bytes)
        .map(|_| ())
        .map_err(|_| WorkspaceLabelStoreError::Invalid)
}

fn read_optional<T: for<'de> Deserialize<'de>>(
    path: &Path,
) -> Result<Option<T>, WorkspaceLabelStoreError> {
    let metadata = match fs::symlink_metadata(path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(_) => return Err(WorkspaceLabelStoreError::Io),
    };
    if metadata.file_type().is_symlink()
        || !metadata.is_file()
        || metadata.len() > MAX_DOCUMENT_BYTES
    {
        return Err(WorkspaceLabelStoreError::Invalid);
    }
    let bytes = fs::read(path).map_err(|_| WorkspaceLabelStoreError::Io)?;
    serde_json::from_slice(&bytes)
        .map(Some)
        .map_err(|_| WorkspaceLabelStoreError::Invalid)
}

fn write_json_atomic<T: Serialize>(path: &Path, value: &T) -> Result<(), WorkspaceLabelStoreError> {
    let parent = path
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
        .unwrap_or(Path::new("."));
    fs::create_dir_all(parent).map_err(|_| WorkspaceLabelStoreError::Io)?;
    if fs::symlink_metadata(path)
        .is_ok_and(|metadata| metadata.file_type().is_symlink() || !metadata.is_file())
    {
        return Err(WorkspaceLabelStoreError::Invalid);
    }
    let mut bytes =
        serde_json::to_vec_pretty(value).map_err(|_| WorkspaceLabelStoreError::Invalid)?;
    bytes.push(b'\n');
    let mut temporary =
        tempfile::NamedTempFile::new_in(parent).map_err(|_| WorkspaceLabelStoreError::Io)?;
    temporary
        .write_all(&bytes)
        .map_err(|_| WorkspaceLabelStoreError::Io)?;
    temporary
        .as_file()
        .sync_all()
        .map_err(|_| WorkspaceLabelStoreError::Io)?;
    temporary
        .persist(path)
        .map_err(|_| WorkspaceLabelStoreError::Io)?;
    Ok(())
}

fn remove_if_present(path: &Path) -> Result<(), WorkspaceLabelStoreError> {
    match fs::remove_file(path) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(_) => Err(WorkspaceLabelStoreError::Io),
    }
}

const fn map_registry_error(error: RegistryError) -> WorkspaceLabelStoreError {
    match error {
        RegistryError::InvalidRecord | RegistryError::InvalidFile => {
            WorkspaceLabelStoreError::Invalid
        }
        RegistryError::Io | RegistryError::Observer => WorkspaceLabelStoreError::Io,
        RegistryError::Frozen => WorkspaceLabelStoreError::Uncertain,
    }
}

const fn store_to_registry(error: WorkspaceLabelStoreError) -> RegistryError {
    match error {
        WorkspaceLabelStoreError::Invalid | WorkspaceLabelStoreError::Conflict => {
            RegistryError::InvalidRecord
        }
        WorkspaceLabelStoreError::Io => RegistryError::Io,
        WorkspaceLabelStoreError::Uncertain => RegistryError::Frozen,
    }
}

#[cfg(test)]
mod tests;
