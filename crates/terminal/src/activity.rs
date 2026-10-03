//! Paseo terminal activity transitions and the read-only Workspace projection.

use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};
use std::time::{SystemTime, UNIX_EPOCH};

use metadata::model::workspace_activity::WorkspaceStateBucket;
use metadata::ports::workspace_state::{
    WorkspaceActivity, WorkspaceActivitySource, WorkspaceStateError,
};
use serde::{Deserialize, Serialize};

/// State reported by a local shell or Agent hook.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ReportState {
    /// Work is in progress.
    Running,
    /// No work is in progress.
    Idle,
    /// The process is waiting for user input.
    NeedsInput,
}

/// Public terminal activity state.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum State {
    /// Work is in progress.
    Working,
    /// No work is in progress; inspect the attention reason separately.
    Idle,
}

/// Why a terminal needs attention.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum AttentionReason {
    /// A working process became idle.
    Finished,
    /// The process requested user input.
    NeedsInput,
}

/// Latest terminal hook state; unknown terminals have no activity value.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Activity {
    /// Current work state.
    pub state: State,
    /// Sticky completion or input attention, if any.
    pub attention_reason: Option<AttentionReason>,
    /// Last actual state transition, in Unix milliseconds.
    pub changed_at: u64,
}

#[derive(Debug)]
struct Entry {
    workspace: String,
    activity: Option<Activity>,
}

/// Shared terminal-owned activity projection, without process handles or credentials.
#[derive(Debug, Clone, Default)]
pub struct Activities {
    entries: Arc<Mutex<BTreeMap<String, Entry>>>,
    changes: Option<model::changes::Changes>,
}

impl Activities {
    /// Wake Workspace subscribers after terminal activity changes.
    #[must_use]
    pub fn with_changes(mut self, changes: model::changes::Changes) -> Self {
        self.changes = Some(changes);
        self
    }

    pub(crate) fn register(&self, id: String, workspace: String) {
        self.entries.lock().expect("terminal activity lock").insert(
            id,
            Entry {
                workspace,
                activity: None,
            },
        );
    }

    pub(crate) fn remove(&self, id: &str) {
        let removed = self
            .entries
            .lock()
            .expect("terminal activity lock")
            .remove(id);
        if removed.is_some() {
            self.notify();
        }
    }

    pub(crate) fn get(&self, id: &str) -> Option<Activity> {
        self.entries
            .lock()
            .expect("terminal activity lock")
            .get(id)
            .and_then(|entry| entry.activity.clone())
    }

    pub(crate) fn report(&self, id: &str, state: ReportState) {
        self.change(id, |previous| transition(previous, state, timestamp()));
    }

    pub(crate) fn interrupt(&self, id: &str) {
        self.change(id, |previous| {
            previous.filter(|activity| activity.state != State::Working)
        });
    }

    pub(crate) fn clear_attention(&self, id: &str) -> bool {
        self.change(id, |previous| {
            previous.map(|mut activity| {
                if activity.attention_reason.take().is_some() {
                    activity.state = State::Idle;
                    activity.changed_at = timestamp();
                }
                activity
            })
        })
    }

    fn change(&self, id: &str, change: impl FnOnce(Option<Activity>) -> Option<Activity>) -> bool {
        let mut entries = self.entries.lock().expect("terminal activity lock");
        let Some(entry) = entries.get_mut(id) else {
            return false;
        };
        let previous = entry.activity.take();
        let next = change(previous.clone());
        let changed = next != previous;
        entry.activity = next;
        drop(entries);
        if changed {
            self.notify();
        }
        changed
    }

    fn notify(&self) {
        if let Some(changes) = &self.changes {
            changes.notify();
        }
    }
}

impl WorkspaceActivitySource for Activities {
    fn snapshot(&self) -> Result<Vec<WorkspaceActivity>, WorkspaceStateError> {
        Ok(self
            .entries
            .lock()
            .expect("terminal activity lock")
            .values()
            .filter_map(|entry| {
                let activity = entry.activity.as_ref()?;
                let bucket = match (activity.state, activity.attention_reason) {
                    (_, Some(AttentionReason::NeedsInput)) => WorkspaceStateBucket::NeedsInput,
                    (_, Some(AttentionReason::Finished)) => WorkspaceStateBucket::Attention,
                    (State::Working, None) => WorkspaceStateBucket::Running,
                    (State::Idle, None) => return None,
                };
                Some(WorkspaceActivity {
                    workspace_id: entry.workspace.clone(),
                    bucket,
                    changed_at: i64::try_from(activity.changed_at)
                        .ok()
                        .and_then(chrono::DateTime::from_timestamp_millis)
                        .map(|time| time.to_rfc3339_opts(chrono::SecondsFormat::Millis, true)),
                })
            })
            .collect())
    }
}

fn transition(previous: Option<Activity>, report: ReportState, now: u64) -> Option<Activity> {
    let (state, attention_reason) = match report {
        ReportState::Running => (State::Working, None),
        ReportState::NeedsInput => (State::Idle, Some(AttentionReason::NeedsInput)),
        ReportState::Idle => {
            let finished = previous.as_ref().is_some_and(|activity| {
                activity.state == State::Working
                    || activity.attention_reason == Some(AttentionReason::Finished)
            });
            (State::Idle, finished.then_some(AttentionReason::Finished))
        }
    };
    if previous.as_ref().is_some_and(|activity| {
        activity.state == state && activity.attention_reason == attention_reason
    }) {
        return previous;
    }
    Some(Activity {
        state,
        attention_reason,
        changed_at: now,
    })
}

fn timestamp() -> u64 {
    u64::try_from(
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis(),
    )
    .unwrap_or(u64::MAX)
}

#[cfg(test)]
mod tests;
