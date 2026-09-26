use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, mpsc as blocking_channel};
use std::time::Duration;

use serde_json::json;
use server_model::outbound::{Outbound, Queued};
use tokio::sync::{Semaphore, mpsc};
use tokio_util::sync::CancellationToken;
use tokio_util::task::TaskTracker;

use super::super::{DiffObservation, PendingSubscription};
use crate::service::checkout::*;

const WAIT_LIMIT: Duration = Duration::from_secs(5);

pub(super) struct Harness {
    pub(super) service: Arc<Mutex<Checkout>>,
    pub(super) jobs: Arc<Semaphore>,
    pub(super) tracker: TaskTracker,
    pub(super) server_cancel: CancellationToken,
    armed: Arc<AtomicBool>,
    calls: mpsc::UnboundedReceiver<PollCall>,
    outbound: Outbound,
    receiver: mpsc::Receiver<Queued>,
}

impl Harness {
    pub(super) fn new() -> Self {
        let (calls, receiver) = mpsc::unbounded_channel();
        let armed = Arc::new(AtomicBool::new(false));
        let checkout = Checkout::new(Box::new(GatedCheckout {
            armed: armed.clone(),
            calls,
        }));
        let (outbound, outgoing) = Outbound::new();
        Self {
            service: Arc::new(Mutex::new(checkout)),
            jobs: Arc::new(Semaphore::new(1)),
            tracker: TaskTracker::new(),
            server_cancel: CancellationToken::new(),
            armed,
            calls: receiver,
            outbound,
            receiver: outgoing,
        }
    }

    pub(super) fn observation(&self, cwd: &str) -> DiffObservation {
        DiffObservation::prepare(
            &self.service.lock().unwrap(),
            json!({"cwd":cwd,"compare":{"mode":"uncommitted"}}),
        )
        .unwrap()
        .0
    }

    pub(super) fn pending(&self, cwd: &str) -> PendingSubscription {
        PendingSubscription {
            observation: self.observation(cwd),
            service: self.service.clone(),
            jobs: self.jobs.clone(),
            tracker: self.tracker.clone(),
            server_cancel: self.server_cancel.clone(),
            outbound: self.outbound.clone(),
        }
    }

    pub(super) fn arm(&self) {
        self.armed.store(true, Ordering::SeqCst);
    }

    pub(super) async fn next_call(&mut self) -> PollCall {
        tokio::time::timeout(WAIT_LIMIT, self.calls.recv())
            .await
            .expect("diff starts once admitted")
            .expect("checkout remains installed")
    }

    pub(super) fn assert_no_calls(&mut self) {
        assert!(self.calls.try_recv().is_err());
    }

    pub(super) fn assert_no_updates(&mut self) {
        assert!(self.receiver.try_recv().is_err());
    }

    pub(super) async fn wait_for_tasks(&self) {
        tokio::time::timeout(WAIT_LIMIT, self.tracker.wait())
            .await
            .expect("released blocking work drains before shutdown completes");
    }
}

impl Drop for Harness {
    fn drop(&mut self) {
        self.server_cancel.cancel();
    }
}

#[derive(Debug)]
pub(super) struct PollCall {
    pub(super) cwd: String,
    release: blocking_channel::Sender<()>,
}

impl Drop for PollCall {
    fn drop(&mut self) {
        let _ = self.release.send(());
    }
}

#[derive(Debug)]
struct GatedCheckout {
    armed: Arc<AtomicBool>,
    calls: mpsc::UnboundedSender<PollCall>,
}

impl CheckoutRuntime for GatedCheckout {
    fn diff(
        &self,
        cwd: &str,
        _compare: &CheckoutDiffCompare,
    ) -> Result<CheckoutDiff, CheckoutRuntimeError> {
        if !self.armed.load(Ordering::SeqCst) {
            return Ok(CheckoutDiff {
                files: Vec::new(),
                diff_too_large: false,
            });
        }
        let (release, wait) = blocking_channel::channel();
        self.calls
            .send(PollCall {
                cwd: cwd.to_owned(),
                release,
            })
            .expect("test receives each gated diff");
        wait.recv_timeout(WAIT_LIMIT)
            .expect("test releases its blocking diff");
        Ok(CheckoutDiff {
            files: vec![ParsedDiffFile {
                path: "changed.txt".to_owned(),
                old_path: None,
                is_new: true,
                is_deleted: false,
                additions: 1,
                deletions: 0,
                hunks: Vec::new(),
                status: Some(ParsedDiffStatus::Ok),
            }],
            diff_too_large: false,
        })
    }

    fn status(&self, _cwd: &str) -> Result<CheckoutStatus, CheckoutRuntimeError> {
        unreachable!("these observation tests only read diffs")
    }

    fn refresh(&self, _cwd: &str) -> Result<(), CheckoutRuntimeError> {
        unreachable!("these observation tests only read diffs")
    }

    fn commits(&self, _cwd: &str) -> Result<CheckoutCommits, CheckoutRuntimeError> {
        unreachable!("these observation tests only read diffs")
    }

    fn commit_file_diff(
        &self,
        _cwd: &str,
        _sha: &str,
        _path: &str,
    ) -> Result<Option<ParsedDiffFile>, CheckoutRuntimeError> {
        unreachable!("these observation tests only read diffs")
    }

    fn validate_branch(
        &self,
        _cwd: &str,
        _branch: &str,
    ) -> Result<CheckoutBranchResolution, CheckoutRuntimeError> {
        unreachable!("these observation tests only read diffs")
    }

    fn branch_suggestions(
        &self,
        _cwd: &str,
        _query: Option<&str>,
        _limit: usize,
    ) -> Result<Vec<CheckoutBranchSuggestion>, CheckoutRuntimeError> {
        unreachable!("these observation tests only read diffs")
    }

    fn switch_branch(
        &self,
        _cwd: &str,
        _branch: &str,
    ) -> Result<CheckoutBranchSource, CheckoutRuntimeError> {
        unreachable!("these observation tests only read diffs")
    }

    fn rename_branch(&self, _cwd: &str, _branch: &str) -> Result<String, CheckoutRuntimeError> {
        unreachable!("these observation tests only read diffs")
    }

    fn commit(
        &self,
        _cwd: &str,
        _message: &str,
        _add_all: bool,
    ) -> Result<(), CheckoutRuntimeError> {
        unreachable!("these observation tests only read diffs")
    }

    fn merge_to_base(
        &self,
        _cwd: &str,
        _base_ref: Option<&str>,
        _strategy: CheckoutMergeStrategy,
        _require_clean_target: bool,
    ) -> Result<(), CheckoutRuntimeError> {
        unreachable!("these observation tests only read diffs")
    }

    fn merge_from_base(
        &self,
        _cwd: &str,
        _base_ref: Option<&str>,
        _require_clean_target: bool,
    ) -> Result<(), CheckoutRuntimeError> {
        unreachable!("these observation tests only read diffs")
    }

    fn pull(&self, _cwd: &str) -> Result<(), CheckoutRuntimeError> {
        unreachable!("these observation tests only read diffs")
    }

    fn push(&self, _cwd: &str) -> Result<(), CheckoutRuntimeError> {
        unreachable!("these observation tests only read diffs")
    }

    fn discard_changes(&self, _cwd: &str, _paths: &[String]) -> Result<(), CheckoutRuntimeError> {
        unreachable!("these observation tests only read diffs")
    }

    fn stash_save(&self, _cwd: &str, _branch: Option<&str>) -> Result<(), CheckoutRuntimeError> {
        unreachable!("these observation tests only read diffs")
    }

    fn stash_pop(&self, _cwd: &str, _index: usize) -> Result<(), CheckoutRuntimeError> {
        unreachable!("these observation tests only read diffs")
    }

    fn stashes(
        &self,
        _cwd: &str,
        _paseo_only: bool,
    ) -> Result<Vec<CheckoutStashEntry>, CheckoutRuntimeError> {
        unreachable!("these observation tests only read diffs")
    }
}
