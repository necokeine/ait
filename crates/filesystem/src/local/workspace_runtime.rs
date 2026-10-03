//! Bounded, demand-driven cache of Workspace Git and Forge presentation facts.

use std::collections::{BTreeMap, VecDeque};
use std::path::PathBuf;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use metadata::ports::workspace_runtime::{
    WorkspaceForgeSnapshot, WorkspaceRuntimeSnapshot, WorkspaceRuntimeSource,
};

use crate::local::{checkout::LocalCheckout, forge::LocalForge};

mod forge;
mod reads;

const CACHE_CAPACITY: usize = 1024;
const READ_CONCURRENCY: usize = 2;
const GIT_TTL: Duration = Duration::from_secs(2);
const FORGE_TTL: Duration = Duration::from_secs(30);

/// Checkout facts shared across directory clones and physical client connections.
///
/// Reads return immediately. Only directories requested by a live list/update reader refresh;
/// Git and Forge have separate admission limits, so a slow remote cannot block local facts.
#[derive(Debug, Clone)]
pub struct LocalWorkspaceRuntime {
    inner: Arc<Inner>,
}

#[derive(Debug)]
struct Inner {
    checkout: LocalCheckout,
    forge: LocalForge,
    cache: Mutex<BTreeMap<PathBuf, Entry>>,
    git_reads: Arc<AtomicUsize>,
    forge_reads: Arc<AtomicUsize>,
    git_jobs: Mutex<VecDeque<reads::Job>>,
    forge_jobs: Mutex<VecDeque<reads::Job>>,
    git_ttl: Duration,
    forge_ttl: Duration,
    changes: Option<model::changes::Changes>,
}

#[derive(Debug)]
struct Entry {
    value: WorkspaceRuntimeSnapshot,
    head: Option<String>,
    git_read: ReadState,
    forge_read: ReadState,
    accessed_at: Instant,
}

#[derive(Debug, Default)]
struct ReadState {
    completed_at: Option<Instant>,
    running: bool,
}

impl ReadState {
    fn due(&self, ttl: Duration) -> bool {
        !self.running && self.completed_at.is_none_or(|last| last.elapsed() >= ttl)
    }

    fn finish(&mut self) {
        self.running = false;
        self.completed_at = Some(Instant::now());
    }
}

#[derive(Debug)]
struct Permit(Arc<AtomicUsize>);

impl Permit {
    fn acquire(reads: &Arc<AtomicUsize>) -> Option<Self> {
        reads
            .fetch_update(Ordering::Relaxed, Ordering::Relaxed, |count| {
                (count < READ_CONCURRENCY).then_some(count + 1)
            })
            .ok()
            .map(|_| Self(reads.clone()))
    }
}

impl Drop for Permit {
    fn drop(&mut self) {
        self.0.fetch_sub(1, Ordering::Relaxed);
    }
}

impl LocalWorkspaceRuntime {
    /// Compose cached observations using the same local adapters as explicit Git/PR requests.
    ///
    /// `checkout` supplies bounded local reads; `forge` reuses the host's existing CLI auth.
    /// No commands start until a Workspace directory snapshot requests a checkout.
    #[must_use]
    pub fn new(checkout: LocalCheckout, forge: LocalForge) -> Self {
        Self {
            inner: Arc::new(Inner {
                checkout,
                forge,
                cache: Mutex::default(),
                git_reads: Arc::default(),
                forge_reads: Arc::default(),
                git_jobs: Mutex::default(),
                forge_jobs: Mutex::default(),
                git_ttl: GIT_TTL,
                forge_ttl: FORGE_TTL,
                changes: None,
            }),
        }
    }

    /// Wake Workspace directory observers when a background read completes.
    ///
    /// # Panics
    /// Panics if the runtime has already been cloned before composition completes.
    #[must_use]
    pub fn with_changes(mut self, changes: model::changes::Changes) -> Self {
        Arc::get_mut(&mut self.inner)
            .expect("new Workspace runtime has one owner")
            .changes = Some(changes);
        self
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct Identity {
    head: Option<String>,
    branch: Option<String>,
    remote: Option<String>,
}

impl Identity {
    fn from_entry(entry: &Entry) -> Self {
        Self {
            head: entry.head.clone(),
            branch: entry
                .value
                .git
                .as_ref()
                .and_then(|git| git.current_branch.clone()),
            remote: entry
                .value
                .git
                .as_ref()
                .and_then(|git| git.remote_url.clone()),
        }
    }
}

impl WorkspaceRuntimeSource for LocalWorkspaceRuntime {
    fn snapshot(&self, cwd: &str) -> WorkspaceRuntimeSnapshot {
        let cwd = std::fs::canonicalize(cwd).unwrap_or_else(|_| PathBuf::from(cwd));
        let mut cache = self
            .inner
            .cache
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if !cache.contains_key(&cwd) && cache.len() >= CACHE_CAPACITY {
            let oldest = cache
                .iter()
                .filter(|(_, entry)| !entry.git_read.running && !entry.forge_read.running)
                .min_by_key(|(_, entry)| entry.accessed_at)
                .map(|(cwd, _)| cwd.clone());
            if let Some(oldest) = oldest {
                cache.remove(&oldest);
            } else {
                return WorkspaceRuntimeSnapshot::default();
            }
        }
        let entry = cache.entry(cwd.clone()).or_insert_with(|| Entry {
            value: WorkspaceRuntimeSnapshot::default(),
            head: None,
            git_read: ReadState::default(),
            forge_read: ReadState::default(),
            accessed_at: Instant::now(),
        });
        entry.accessed_at = Instant::now();
        let git_due = entry.git_read.due(self.inner.git_ttl);
        if git_due {
            entry.git_read.running = true;
        }
        let forge_due = if entry
            .value
            .git
            .as_ref()
            .is_some_and(|git| git.remote_url.is_some())
        {
            entry.forge_read.due(self.inner.forge_ttl)
        } else {
            if entry.git_read.completed_at.is_some() {
                entry.value.forge = Some(WorkspaceForgeSnapshot::default());
            }
            false
        };
        if forge_due {
            entry.forge_read.running = true;
        }
        let identity = Identity::from_entry(entry);
        let snapshot = entry.value.clone();
        drop(cache);
        if git_due {
            reads::enqueue(&self.inner, reads::Job::Git(cwd.clone()));
        }
        if forge_due {
            reads::enqueue(&self.inner, reads::Job::Forge(cwd, identity));
        }
        snapshot
    }
}

#[cfg(test)]
mod tests;
