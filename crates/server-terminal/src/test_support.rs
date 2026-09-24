use std::sync::{Arc, Mutex};

use serde_json::json;
use server_metadata::model::registry::{PersistedProjectRecord, PersistedWorkspaceRecord};
use server_metadata::ports::registry::{
    ActiveProjectInput, MutationListener, MutationSubscription, ProjectMutation, ProjectRegistry,
    RegistryError, WorkspaceArchiveContext, WorkspaceMutation, WorkspaceMutationContext,
    WorkspaceRegistry,
};

use crate::Error;
use crate::ports::{Launch, Observation, Process, Runtime};
use crate::protocol::{CreateRequest, Input, Restore, Size};
use crate::service::Terminals;

#[derive(Debug, Clone)]
pub(crate) struct Registries {
    pub workspaces: Arc<Mutex<Vec<PersistedWorkspaceRecord>>>,
    pub projects: Arc<Mutex<Vec<PersistedProjectRecord>>>,
    pub failure: Arc<Mutex<bool>>,
}

impl Default for Registries {
    fn default() -> Self {
        let workspace = serde_json::from_value(json!({"workspaceId":"w","projectId":"p","cwd":"/repo","kind":"directory","displayName":"repo","isPaseoOwnedWorktree":false,"createdAt":"now","updatedAt":"now","archivedAt":null})).unwrap();
        let project = serde_json::from_value(json!({"projectId":"p","rootPath":"/repo","kind":"non_git","displayName":"repo","createdAt":"now","updatedAt":"now","archivedAt":null})).unwrap();
        Self {
            workspaces: Arc::new(Mutex::new(vec![workspace])),
            projects: Arc::new(Mutex::new(vec![project])),
            failure: Arc::new(Mutex::new(false)),
        }
    }
}

#[derive(Debug)]
struct Subscription;
impl MutationSubscription for Subscription {}

impl ProjectRegistry for Registries {
    fn initialize(&self) -> Result<(), RegistryError> {
        Ok(())
    }
    fn exists_on_disk(&self) -> bool {
        false
    }
    fn list(&self) -> Result<Vec<PersistedProjectRecord>, RegistryError> {
        if *self.failure.lock().unwrap() {
            Err(RegistryError::Io)
        } else {
            Ok(self.projects.lock().unwrap().clone())
        }
    }
    fn get(&self, id: &str) -> Result<Option<PersistedProjectRecord>, RegistryError> {
        Ok(self
            .projects
            .lock()
            .unwrap()
            .iter()
            .find(|record| record.project_id == id)
            .cloned())
    }
    fn get_or_create_active_by_root(
        &self,
        _: &ActiveProjectInput,
    ) -> Result<PersistedProjectRecord, RegistryError> {
        Err(RegistryError::Io)
    }
    fn upsert(&self, _: &PersistedProjectRecord) -> Result<(), RegistryError> {
        Err(RegistryError::Io)
    }
    fn update(
        &self,
        _: &str,
        _: &dyn Fn(&PersistedProjectRecord) -> PersistedProjectRecord,
    ) -> Result<Option<PersistedProjectRecord>, RegistryError> {
        Err(RegistryError::Io)
    }
    fn archive(&self, _: &str, _: &str) -> Result<(), RegistryError> {
        Err(RegistryError::Io)
    }
    fn remove(&self, _: &str) -> Result<(), RegistryError> {
        Err(RegistryError::Io)
    }
    fn subscribe_to_mutations(
        &self,
        _: MutationListener<ProjectMutation>,
    ) -> Box<dyn MutationSubscription> {
        Box::new(Subscription)
    }
}

impl WorkspaceRegistry for Registries {
    fn initialize(&self) -> Result<(), RegistryError> {
        Ok(())
    }
    fn exists_on_disk(&self) -> bool {
        false
    }
    fn list(&self) -> Result<Vec<PersistedWorkspaceRecord>, RegistryError> {
        if *self.failure.lock().unwrap() {
            Err(RegistryError::Io)
        } else {
            Ok(self.workspaces.lock().unwrap().clone())
        }
    }
    fn get(&self, id: &str) -> Result<Option<PersistedWorkspaceRecord>, RegistryError> {
        Ok(self
            .workspaces
            .lock()
            .unwrap()
            .iter()
            .find(|record| record.workspace_id == id)
            .cloned())
    }
    fn upsert(
        &self,
        _: &PersistedWorkspaceRecord,
        _: WorkspaceMutationContext,
    ) -> Result<(), RegistryError> {
        Err(RegistryError::Io)
    }
    fn update(
        &self,
        _: &str,
        _: &dyn Fn(&PersistedWorkspaceRecord) -> PersistedWorkspaceRecord,
    ) -> Result<Option<PersistedWorkspaceRecord>, RegistryError> {
        Err(RegistryError::Io)
    }
    fn archive(&self, _: &str, _: &str, _: &WorkspaceArchiveContext) -> Result<(), RegistryError> {
        Err(RegistryError::Io)
    }
    fn remove(&self, _: &str) -> Result<(), RegistryError> {
        Err(RegistryError::Io)
    }
    fn subscribe_to_mutations(
        &self,
        _: MutationListener<WorkspaceMutation>,
    ) -> Box<dyn MutationSubscription> {
        Box::new(Subscription)
    }
    fn block_all_mutations_until_restart(&self) -> Result<(), RegistryError> {
        Err(RegistryError::Io)
    }
}

#[derive(Debug, Default)]
pub(crate) struct Calls {
    pub launches: Vec<Launch>,
    pub inputs: Vec<String>,
    pub killed: usize,
    pub exited: bool,
    pub failure: bool,
}

#[derive(Debug)]
struct MockRuntime(Arc<Mutex<Calls>>);

impl Runtime for MockRuntime {
    fn directory(&self, path: &str) -> Result<String, Error> {
        if path.starts_with('/') {
            Ok(path.to_owned())
        } else {
            Err(Error::Invalid)
        }
    }
    fn spawn(&self, launch: &Launch) -> Result<Box<dyn Process>, Error> {
        let mut calls = self.0.lock().unwrap();
        if calls.failure {
            return Err(Error::Io);
        }
        calls.launches.push(launch.clone());
        Ok(Box::new(MockProcess(self.0.clone())))
    }
}

#[derive(Debug)]
struct MockProcess(Arc<Mutex<Calls>>);
impl Process for MockProcess {
    fn title(&self) -> Option<String> {
        Some("shell title".to_owned())
    }
    fn exited(&mut self) -> Result<bool, Error> {
        Ok(self.0.lock().unwrap().exited)
    }
    fn send(&mut self, input: &Input) -> Result<(), Error> {
        self.0.lock().unwrap().inputs.push(format!("{input:?}"));
        Ok(())
    }
    fn observe(&mut self, _: Option<u64>, _: Option<&Restore>) -> Result<Observation, Error> {
        Ok(Observation {
            revision: 1,
            size: Size::default(),
            frames: vec![],
            exited: false,
        })
    }
    fn capture(&self) -> Result<Vec<String>, Error> {
        Ok(vec![
            "first".to_owned(),
            "second".to_owned(),
            "last".to_owned(),
        ])
    }
    fn kill(&mut self) -> Result<(), Error> {
        let mut calls = self.0.lock().unwrap();
        if calls.failure {
            return Err(Error::Io);
        }
        calls.killed += 1;
        Ok(())
    }
}

pub(crate) fn fixture() -> (Terminals, Registries, Arc<Mutex<Calls>>) {
    let registry = Registries::default();
    let calls = Arc::new(Mutex::new(Calls::default()));
    let service = Terminals::new(
        Box::new(registry.clone()),
        Box::new(registry.clone()),
        Box::new(MockRuntime(calls.clone())),
    );
    (service, registry, calls)
}

pub(crate) fn request() -> CreateRequest {
    serde_json::from_value(
        json!({"cwd":"/repo/sub","command":"shell","args":["literal; argument"]}),
    )
    .unwrap()
}
