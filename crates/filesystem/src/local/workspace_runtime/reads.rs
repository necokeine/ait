//! Fair refresh admission: two local and two remote workers drain bounded demand queues.

use std::collections::VecDeque;
use std::path::PathBuf;
use std::sync::atomic::AtomicUsize;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use super::{Identity, Inner, Permit, forge};
use crate::ports::forge::ForgeRuntime;

const DEMAND_TTL: Duration = Duration::from_secs(5);

#[derive(Debug)]
/// One coalesced refresh admitted by the directory cache.
pub(super) enum Job {
    /// Read local Git status and statistics.
    Git(PathBuf),
    /// Read Forge facts for one captured Git identity.
    Forge(PathBuf, Identity),
}

impl Job {
    fn cwd(&self) -> &PathBuf {
        match self {
            Self::Git(cwd) | Self::Forge(cwd, _) => cwd,
        }
    }

    fn is_forge(&self) -> bool {
        matches!(self, Self::Forge(..))
    }
}

fn queue(inner: &Inner, is_forge: bool) -> (&Mutex<VecDeque<Job>>, &Arc<AtomicUsize>) {
    if is_forge {
        (&inner.forge_jobs, &inner.forge_reads)
    } else {
        (&inner.git_jobs, &inner.git_reads)
    }
}

/// Queue `job` in FIFO order and start a bounded worker when capacity is available.
/// Thread admission failures release queued read state and are logged for diagnostics.
pub(super) fn enqueue(inner: &Arc<Inner>, job: Job) {
    let is_forge = job.is_forge();
    let (jobs, active) = queue(inner, is_forge);
    let mut jobs = jobs
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    jobs.push_back(job);
    let Some(permit) = Permit::acquire(active) else {
        return;
    };
    let worker = inner.clone();
    // Admission and idle-worker retirement share the queue lock, preventing lost wakeups.
    let started = std::thread::Builder::new()
        .name(format!(
            "workspace-{}-read",
            if is_forge { "forge" } else { "git" }
        ))
        .spawn(move || drain(&worker, is_forge, permit));
    if let Err(error) = started {
        tracing::warn!(%error, "could not start Workspace runtime read");
        let abandoned: Vec<_> = jobs.drain(..).collect();
        drop(jobs);
        let mut cache = inner
            .cache
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        for job in abandoned {
            if let Some(entry) = cache.get_mut(job.cwd()) {
                if is_forge {
                    entry.forge_read.finish();
                } else {
                    entry.git_read.finish();
                }
            }
        }
    }
}

fn drain(inner: &Inner, is_forge: bool, permit: Permit) {
    let (jobs, _) = queue(inner, is_forge);
    loop {
        let mut jobs = jobs
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let Some(job) = jobs.pop_front() else {
            drop(permit);
            return;
        };
        drop(jobs);
        if !has_demand(inner, &job) {
            continue;
        }
        match job {
            Job::Git(cwd) => read_git(inner, &cwd),
            Job::Forge(cwd, identity) => read_forge(inner, &cwd, &identity),
        }
    }
}

fn has_demand(inner: &Inner, job: &Job) -> bool {
    let mut cache = inner
        .cache
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let Some(entry) = cache.get_mut(job.cwd()) else {
        return false;
    };
    if entry.accessed_at.elapsed() < DEMAND_TTL {
        return true;
    }
    if job.is_forge() {
        entry.forge_read.running = false;
    } else {
        entry.git_read.running = false;
    }
    false
}

fn read_git(inner: &Inner, cwd: &std::path::Path) {
    let read = inner
        .checkout
        .sidebar_summary(&cwd.to_string_lossy())
        .unwrap_or_default();
    let mut cache = inner
        .cache
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let Some(entry) = cache.get_mut(cwd) else {
        return;
    };
    let identity_changed = entry.head != read.head
        || entry
            .value
            .git
            .as_ref()
            .map(|git| (&git.current_branch, &git.remote_url))
            != read
                .git
                .as_ref()
                .map(|git| (&git.current_branch, &git.remote_url));
    if identity_changed {
        entry.value.forge = None;
        // In-flight/queued Forge reads retain admission; their identity check rejects old results.
        entry.forge_read.completed_at = None;
    }
    entry.head = read.head;
    entry.value.git = read.git;
    entry.git_read.finish();
    drop(cache);
    if let Some(changes) = &inner.changes {
        changes.notify();
    }
}

fn read_forge(inner: &Inner, cwd: &std::path::Path, identity: &Identity) {
    let read = forge::snapshot(
        inner
            .forge
            .current_pull_request_status(&cwd.to_string_lossy()),
    );
    let mut cache = inner
        .cache
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let Some(entry) = cache.get_mut(cwd) else {
        return;
    };
    entry.forge_read.running = false;
    if identity != &Identity::from_entry(entry) {
        return;
    }
    let mut read = read;
    if read.features_enabled && read.error.is_some() && read.pull_request.is_none() {
        read.pull_request = entry
            .value
            .forge
            .as_ref()
            .and_then(|forge| forge.pull_request.clone());
    }
    entry.value.forge = Some(read);
    entry.forge_read.finish();
    drop(cache);
    if let Some(changes) = &inner.changes {
        changes.notify();
    }
}

#[cfg(test)]
mod tests;
