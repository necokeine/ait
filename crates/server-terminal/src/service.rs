//! Workspace placement and process ownership for the terminal capability.

use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

use server_metadata::model::registry::PersistedWorkspaceRecord;
use server_metadata::ports::registry::{ProjectRegistry, WorkspaceRegistry};
use uuid::Uuid;

use crate::Error;
use crate::ports::{Launch, Observation, Process, Runtime};
use crate::protocol::{CreateRequest, Input, ListRequest, ResizeIntent, Restore, TerminalInfo};

const MAX_TERMINALS: usize = 32;

#[derive(Debug)]
struct Entry {
    info: TerminalInfo,
    process: Box<dyn Process>,
    owner: Option<String>,
    manual_title: bool,
    closed: bool,
}

/// Terminal service. A host serializes calls and runs blocking operations outside its reactor.
#[derive(Debug)]
pub struct Terminals {
    workspaces: Box<dyn WorkspaceRegistry>,
    projects: Box<dyn ProjectRegistry>,
    runtime: Box<dyn Runtime>,
    entries: BTreeMap<String, Entry>,
}

impl Terminals {
    /// Compose workspace placement with an independently testable PTY factory.
    #[must_use]
    pub fn new(
        workspaces: Box<dyn WorkspaceRegistry>,
        projects: Box<dyn ProjectRegistry>,
        runtime: Box<dyn Runtime>,
    ) -> Self {
        Self {
            workspaces,
            projects,
            runtime,
            entries: BTreeMap::new(),
        }
    }

    /// Create a terminal in an active workspace, resolving omitted placement by deepest root.
    ///
    /// # Errors
    /// Returns invalid options, inactive/mismatched placement, registry errors, or spawn failures.
    pub fn create(&mut self, request: &CreateRequest) -> Result<TerminalInfo, Error> {
        request.size.validate()?;
        if request.agent_id.as_ref().is_some_and(|id| !id.is_empty())
            || request
                .command
                .as_ref()
                .is_some_and(|command| command.is_empty() || command.contains('\0'))
            || request.args.len() > 256
            || request.args.iter().any(|arg| arg.contains('\0'))
            || request
                .name
                .as_ref()
                .is_some_and(|name| name.len() > 800 || name.chars().any(char::is_control))
        {
            return Err(Error::Invalid);
        }
        let cwd = self.runtime.directory(&request.cwd)?;
        let workspaces = self.active_workspaces()?;
        let workspace = match &request.workspace_id {
            Some(id) => workspaces
                .iter()
                .find(|workspace| &workspace.workspace_id == id),
            None => workspaces
                .iter()
                .filter(|workspace| Path::new(&cwd).starts_with(&workspace.cwd))
                .max_by_key(|workspace| workspace.cwd.len()),
        }
        .ok_or(Error::WorkspaceNotFound)?;
        let root = self.runtime.directory(&workspace.cwd)?;
        if !Path::new(&cwd).starts_with(root) {
            return Err(Error::Invalid);
        }
        // Bound both running processes and recently completed screen retention.
        if self.entries.len() >= MAX_TERMINALS {
            let mut expired = Vec::new();
            for (id, entry) in &mut self.entries {
                if entry.closed || entry.process.exited()? {
                    expired.push(id.clone());
                }
            }
            for id in expired {
                self.entries.remove(&id);
            }
        }
        if self.entries.len() >= MAX_TERMINALS {
            return Err(Error::Exhausted);
        }
        let id = Uuid::new_v4().to_string();
        let launch = Launch {
            cwd: cwd.clone(),
            command: request.command.clone(),
            args: request.args.clone(),
            size: request.size,
            env: BTreeMap::from([
                ("PASEO_TERMINAL_ID".to_owned(), id.clone()),
                (
                    "PASEO_WORKSPACE_ID".to_owned(),
                    workspace.workspace_id.clone(),
                ),
            ]),
        };
        let process = self.runtime.spawn(&launch)?;
        let default_name = format!(
            "Terminal {}",
            self.entries
                .values()
                .filter(|entry| entry.info.cwd == cwd)
                .count()
                + 1
        );
        let info = TerminalInfo {
            id: id.clone(),
            name: request.name.clone().unwrap_or(default_name),
            cwd,
            workspace_id: workspace.workspace_id.clone(),
            title: process.title(),
            activity: None,
        };
        self.entries.insert(
            id,
            Entry {
                info: info.clone(),
                process,
                owner: None,
                manual_title: false,
                closed: false,
            },
        );
        Ok(info)
    }

    /// List running terminals, preserving workspace identities and deepest-root filtering.
    ///
    /// # Errors
    /// Returns path, registry, or process inspection failures.
    pub fn list(&mut self, filter: &ListRequest) -> Result<Vec<TerminalInfo>, Error> {
        let workspaces = self.active_workspaces()?;
        let root = filter
            .cwd
            .as_ref()
            .map(|cwd| self.runtime.directory(cwd))
            .transpose()?;
        let mut result = Vec::new();
        for entry in self.entries.values_mut() {
            if entry.closed || entry.process.exited()? {
                continue;
            }
            if !entry.manual_title {
                entry.info.title = entry.process.title();
            }
            let matches = if let Some(workspace_id) = &filter.workspace_id {
                &entry.info.workspace_id == workspace_id
            } else if let Some(root) = &root {
                let owner = workspaces
                    .iter()
                    .filter(|workspace| Path::new(&entry.info.cwd).starts_with(&workspace.cwd))
                    .max_by_key(|workspace| workspace.cwd.len());
                owner.map_or_else(
                    || Path::new(&entry.info.cwd).starts_with(root),
                    |workspace| &workspace.cwd == root,
                )
            } else {
                true
            };
            if matches {
                result.push(entry.info.clone());
            }
        }
        Ok(result)
    }

    /// Set a persistent-in-process title override, independent of subsequent OSC titles.
    ///
    /// # Errors
    /// Returns an invalid title or a missing terminal.
    pub fn rename(&mut self, id: &str, title: &str) -> Result<(), Error> {
        let title = title.trim();
        if title.is_empty()
            || title.encode_utf16().count() > 200
            || title.chars().any(char::is_control)
        {
            return Err(Error::Invalid);
        }
        let entry = self.entries.get_mut(id).ok_or(Error::NotFound)?;
        if entry.closed || entry.process.exited()? {
            return Err(Error::NotFound);
        }
        entry.info.title = Some(title.to_owned());
        entry.manual_title = true;
        Ok(())
    }

    /// Send input or resize. `owner` is a server-generated physical-connection identity.
    ///
    /// # Errors
    /// Returns invalid input, missing terminal, full queue, or native I/O failures.
    pub fn input(&mut self, id: &str, owner: &str, input: &Input) -> Result<(), Error> {
        let entry = self.entries.get_mut(id).ok_or(Error::NotFound)?;
        if entry.closed || entry.process.exited()? {
            return Err(Error::NotFound);
        }
        if let Input::Resize(resize) = input {
            resize.size.validate()?;
            if resize.intent == ResizeIntent::Update && entry.owner.as_deref() != Some(owner) {
                return Ok(());
            }
            entry.process.send(input)?;
            if resize.intent == ResizeIntent::Claim {
                entry.owner = Some(owner.to_owned());
            }
            return Ok(());
        }
        entry.process.send(input)
    }

    /// Obtain an atomic bootstrap or output delta, including final drained output.
    ///
    /// # Errors
    /// Returns a missing terminal or snapshot/native failures.
    pub fn observe(
        &mut self,
        id: &str,
        revision: Option<u64>,
        restore: Option<&Restore>,
    ) -> Result<Observation, Error> {
        let entry = self.entries.get_mut(id).ok_or(Error::NotFound)?;
        if entry.closed && revision.is_none() {
            return Err(Error::NotFound);
        }
        let mut observation = entry.process.observe(revision, restore)?;
        observation.exited |= entry.closed;
        Ok(observation)
    }

    /// Capture all retained rendered rows; a missing/exited-and-evicted terminal is empty.
    ///
    /// # Errors
    /// Returns a native screen access error.
    pub fn capture(&self, id: &str) -> Result<Vec<String>, Error> {
        self.entries
            .get(id)
            .filter(|entry| !entry.closed)
            .map_or_else(|| Ok(Vec::new()), |entry| entry.process.capture())
    }

    /// Idempotently kill a terminal, retaining its drained tail for existing observers.
    ///
    /// # Errors
    /// Returns native termination/reaping failures and retains the entry for retry.
    pub fn kill(&mut self, id: &str) -> Result<(), Error> {
        if let Some(entry) = self.entries.get_mut(id)
            && !entry.closed
        {
            entry.process.kill()?;
            entry.closed = true;
        }
        Ok(())
    }

    /// Reconcile terminal ownership after archive/removal of workspaces or projects.
    ///
    /// # Errors
    /// Returns registry or process cleanup failures; failed entries remain retryable.
    pub fn reconcile(&mut self) -> Result<(), Error> {
        let active: BTreeSet<_> = self
            .active_workspaces()?
            .into_iter()
            .map(|workspace| workspace.workspace_id)
            .collect();
        let removed: Vec<_> = self
            .entries
            .iter()
            .filter(|(_, entry)| !active.contains(&entry.info.workspace_id))
            .map(|(id, _)| id.clone())
            .collect();
        for id in removed {
            self.kill(&id)?;
        }
        Ok(())
    }

    /// Kill every process before the host releases its instance lease.
    ///
    /// # Errors
    /// Returns the first cleanup error after attempting every terminal.
    pub fn shutdown(&mut self) -> Result<(), Error> {
        let ids: Vec<_> = self.entries.keys().cloned().collect();
        let mut failure = None;
        for id in ids {
            if let Err(error) = self.kill(&id) {
                failure.get_or_insert(error);
            }
        }
        self.entries.retain(|_, entry| !entry.closed);
        failure.map_or(Ok(()), Err)
    }

    fn active_workspaces(&self) -> Result<Vec<PersistedWorkspaceRecord>, Error> {
        let projects: BTreeSet<_> = self
            .projects
            .list()
            .map_err(|_| Error::Registry)?
            .into_iter()
            .filter(|project| project.archived_at.as_ref().is_none_or(String::is_empty))
            .map(|project| project.project_id)
            .collect();
        Ok(self
            .workspaces
            .list()
            .map_err(|_| Error::Registry)?
            .into_iter()
            .filter(|workspace| {
                workspace.archived_at.as_ref().is_none_or(String::is_empty)
                    && projects.contains(&workspace.project_id)
            })
            .map(|mut workspace| {
                // Registry paths preserve the user's spelling; PTYs use canonical paths.
                if let Ok(cwd) = self.runtime.directory(&workspace.cwd) {
                    workspace.cwd = cwd;
                }
                workspace
            })
            .collect())
    }
}

#[cfg(test)]
mod tests;
