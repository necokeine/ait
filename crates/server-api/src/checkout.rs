use std::sync::{Arc, Mutex};
use std::time::Duration;

use serde::Serialize;
use serde_json::Value;
use server_application::checkout::{self as port, Checkout};
use server_protocol::checkout as protocol;
use server_protocol::{ErrorCode, ServerMessage, valid_id};
use tokio::sync::Semaphore;
use tokio::time::{Instant, MissedTickBehavior};
use tokio_util::sync::CancellationToken;
use tokio_util::task::TaskTracker;
use uuid::Uuid;

use crate::Shared;
use crate::outbound::Outbound;

const POLL_INTERVAL: Duration = Duration::from_millis(200);

pub(super) struct Dispatch {
    pub(super) value: Value,
    pub(super) subscription: Option<PendingSubscription>,
}

pub(super) struct PendingSubscription {
    subscription_id: String,
    cwd: String,
    compare: port::CheckoutDiffCompare,
    fingerprint: String,
    service: Arc<Mutex<Checkout>>,
    jobs: Arc<Semaphore>,
    tracker: TaskTracker,
    server_cancel: CancellationToken,
    outbound: Outbound,
}

/// RAII owner for one connection-local diff polling task.
pub(super) struct CheckoutDiffSubscription {
    cancellation: CancellationToken,
}

impl Drop for CheckoutDiffSubscription {
    fn drop(&mut self) {
        self.cancellation.cancel();
    }
}

impl PendingSubscription {
    pub(super) fn activate(self) -> (String, CheckoutDiffSubscription) {
        let cancellation = CancellationToken::new();
        let task_cancellation = cancellation.clone();
        let subscription_id = self.subscription_id.clone();
        self.tracker.spawn(async move {
            let mut fingerprint = self.fingerprint;
            let mut interval =
                tokio::time::interval_at(Instant::now() + POLL_INTERVAL, POLL_INTERVAL);
            interval.set_missed_tick_behavior(MissedTickBehavior::Skip);
            loop {
                tokio::select! {
                    biased;
                    () = task_cancellation.cancelled() => break,
                    () = self.server_cancel.cancelled() => break,
                    _ = interval.tick() => {}
                }
                let Some(snapshot) = poll_diff(
                    self.service.clone(),
                    self.jobs.clone(),
                    self.cwd.clone(),
                    self.compare.clone(),
                )
                .await
                else {
                    continue;
                };
                let next = protocol_diff_result(&self.cwd, snapshot);
                let next_fingerprint = serde_json::to_string(&next).unwrap_or_default();
                if next_fingerprint == fingerprint {
                    continue;
                }
                fingerprint = next_fingerprint;
                let Ok(params) = serde_json::to_value(protocol::CheckoutDiffSubscriptionResult {
                    subscription_id: self.subscription_id.clone(),
                    cwd: next.cwd,
                    files: next.files,
                    error: next.error,
                    diff_too_large: next.diff_too_large,
                }) else {
                    break;
                };
                if self
                    .outbound
                    .send(&ServerMessage::Event {
                        method: "checkout.diff.update".to_owned(),
                        params,
                    })
                    .is_err()
                {
                    break;
                }
            }
        });
        (subscription_id, CheckoutDiffSubscription { cancellation })
    }
}

pub(super) async fn dispatch(
    method: &str,
    params: Value,
    state: &Shared,
    outbound: Outbound,
) -> Result<Dispatch, ErrorCode> {
    if method == "checkout.diff.subscribe.request" {
        return subscribe(params, state, outbound).await;
    }
    let method = method.to_owned();
    crate::jobs::run(
        state,
        state.checkout.clone(),
        ErrorCode::ProjectIo,
        move |checkout| execute(checkout, &method, params),
    )
    .await
}

fn execute(checkout: &Checkout, method: &str, params: Value) -> Result<Dispatch, ErrorCode> {
    match method {
        "checkout.status.get.request" => status(checkout, &decode(params)?),
        "checkout.refresh.request" => refresh(checkout, &decode(params)?),
        "checkout.diff.get.request" => diff(checkout, decode(params)?),
        "checkout.commits.list.request" => commits(checkout, &decode(params)?),
        "checkout.commits.file_diff.request" => commit_file_diff(checkout, decode(params)?),
        "checkout.branch.validate.request" => validate_branch(checkout, &decode(params)?),
        "checkout.branch.suggestions.request" => branch_suggestions(checkout, &decode(params)?),
        "checkout.branch.switch.request" => switch_branch(checkout, decode(params)?),
        "checkout.rename_branch.request" => rename_branch(checkout, decode(params)?),
        "checkout.commit.request" => commit(checkout, decode(params)?),
        "checkout.merge.request" => merge_to_base(checkout, &decode(params)?),
        "checkout.merge_from_base.request" => merge_from_base(checkout, &decode(params)?),
        "checkout.pull.request" => mutate_path(checkout, &decode(params)?, Checkout::pull),
        "checkout.push.request" => mutate_path(checkout, &decode(params)?, Checkout::push),
        "checkout.discard_changes.request" => discard_changes(checkout, &decode(params)?),
        "checkout.stash.save.request" => stash_save(checkout, &decode(params)?),
        "checkout.stash.pop.request" => stash_pop(checkout, &decode(params)?),
        "checkout.stash.list.request" => stash_list(checkout, decode(params)?),
        _ => Err(ErrorCode::MethodNotFound),
    }
}

async fn subscribe(
    params: Value,
    state: &Shared,
    outbound: Outbound,
) -> Result<Dispatch, ErrorCode> {
    let request: protocol::CheckoutDiffSubscribeRequest = decode(params)?;
    let subscription_id = request
        .subscription_id
        .clone()
        .unwrap_or_else(|| Uuid::new_v4().to_string());
    if !valid_id(&subscription_id) {
        return Err(ErrorCode::InvalidMessage);
    }
    let compare = port_compare(request.compare);
    let cwd = request.cwd;
    let diff_cwd = cwd.clone();
    let diff_compare = compare.clone();
    let initial = crate::jobs::run(
        state,
        state.checkout.clone(),
        ErrorCode::ProjectIo,
        move |checkout| Ok(checkout.diff(&diff_cwd, &diff_compare)),
    )
    .await?;
    let initial = protocol_diff_result(&cwd, initial);
    let fingerprint = serde_json::to_string(&initial).map_err(|_| ErrorCode::ProjectIo)?;
    let value = encode(protocol::CheckoutDiffSubscriptionResult {
        subscription_id: subscription_id.clone(),
        cwd: initial.cwd,
        files: initial.files,
        error: initial.error,
        diff_too_large: initial.diff_too_large,
    })?;
    Ok(Dispatch {
        value,
        subscription: Some(PendingSubscription {
            subscription_id,
            cwd,
            compare,
            fingerprint,
            service: state
                .checkout
                .clone()
                .ok_or(ErrorCode::UnsupportedCapability)?,
            jobs: state.jobs.clone(),
            tracker: state.tasks.clone(),
            server_cancel: state.cancellation.clone(),
            outbound,
        }),
    })
}

fn status(
    checkout: &Checkout,
    request: &protocol::CheckoutPathRequest,
) -> Result<Dispatch, ErrorCode> {
    let result = checkout.status(&request.cwd);
    value(protocol_status(&request.cwd, result))
}

fn refresh(
    checkout: &Checkout,
    request: &protocol::CheckoutPathRequest,
) -> Result<Dispatch, ErrorCode> {
    let result = checkout.refresh(&request.cwd);
    value(protocol::CheckoutRefreshResult {
        cwd: request.cwd.clone(),
        success: result.is_ok(),
        error: result.err().map(protocol_error),
    })
}

fn diff(
    checkout: &Checkout,
    request: protocol::CheckoutDiffGetRequest,
) -> Result<Dispatch, ErrorCode> {
    let result = checkout.diff(&request.cwd, &port_compare(request.compare));
    value(protocol_diff_result(&request.cwd, result))
}

fn commits(
    checkout: &Checkout,
    request: &protocol::CheckoutPathRequest,
) -> Result<Dispatch, ErrorCode> {
    let result = match checkout.commits(&request.cwd) {
        Ok(result) => protocol::CheckoutCommitsListResult {
            cwd: request.cwd.clone(),
            base_ref: result.base_ref,
            commits: result.commits.into_iter().map(protocol_commit).collect(),
            error: None,
        },
        Err(error) => protocol::CheckoutCommitsListResult {
            cwd: request.cwd.clone(),
            base_ref: None,
            commits: Vec::new(),
            error: Some(protocol_error(error)),
        },
    };
    value(result)
}

fn commit_file_diff(
    checkout: &Checkout,
    request: protocol::CheckoutCommitFileDiffRequest,
) -> Result<Dispatch, ErrorCode> {
    let result = match checkout.commit_file_diff(&request.cwd, &request.sha, &request.path) {
        Ok(file) => protocol::CheckoutCommitFileDiffResult {
            cwd: request.cwd,
            sha: request.sha,
            path: request.path,
            file: file.map(protocol_diff_file),
            error: None,
        },
        Err(error) => protocol::CheckoutCommitFileDiffResult {
            cwd: request.cwd,
            sha: request.sha,
            path: request.path,
            file: None,
            error: Some(protocol_error(error)),
        },
    };
    value(result)
}

fn validate_branch(
    checkout: &Checkout,
    request: &protocol::CheckoutBranchValidateRequest,
) -> Result<Dispatch, ErrorCode> {
    let result = match checkout.validate_branch(&request.cwd, &request.branch_name) {
        Ok(port::CheckoutBranchResolution::Local(name)) => protocol::CheckoutBranchValidateResult {
            exists: true,
            resolved_ref: Some(name),
            is_remote: false,
            error: None,
        },
        Ok(port::CheckoutBranchResolution::RemoteOnly { name, .. }) => {
            protocol::CheckoutBranchValidateResult {
                exists: true,
                resolved_ref: Some(name),
                is_remote: true,
                error: None,
            }
        }
        Ok(port::CheckoutBranchResolution::NotFound) => protocol::CheckoutBranchValidateResult {
            exists: false,
            resolved_ref: None,
            is_remote: false,
            error: None,
        },
        Err(error) => protocol::CheckoutBranchValidateResult {
            exists: false,
            resolved_ref: None,
            is_remote: false,
            error: Some(error.message),
        },
    };
    value(result)
}

fn branch_suggestions(
    checkout: &Checkout,
    request: &protocol::CheckoutBranchSuggestionsRequest,
) -> Result<Dispatch, ErrorCode> {
    let limit = request.limit.unwrap_or(50);
    if !(1..=200).contains(&limit) {
        return Err(ErrorCode::InvalidMessage);
    }
    let result = match checkout.branch_suggestions(&request.cwd, request.query.as_deref(), limit) {
        Ok(suggestions) => {
            let details = suggestions
                .into_iter()
                .map(protocol_branch_suggestion)
                .collect::<Vec<_>>();
            protocol::CheckoutBranchSuggestionsResult {
                branches: details
                    .iter()
                    .map(|suggestion| suggestion.name.clone())
                    .collect(),
                branch_details: Some(details),
                error: None,
            }
        }
        Err(error) => protocol::CheckoutBranchSuggestionsResult {
            branches: Vec::new(),
            branch_details: None,
            error: Some(error.message),
        },
    };
    value(result)
}

fn switch_branch(
    checkout: &Checkout,
    request: protocol::CheckoutBranchSwitchRequest,
) -> Result<Dispatch, ErrorCode> {
    let result = checkout.switch_branch(&request.cwd, &request.branch);
    value(protocol::CheckoutBranchSwitchResult {
        cwd: request.cwd,
        success: result.is_ok(),
        branch: request.branch,
        source: result.as_ref().ok().map(|source| match source {
            port::CheckoutBranchSource::Local => protocol::CheckoutBranchSource::Local,
            port::CheckoutBranchSource::Remote => protocol::CheckoutBranchSource::Remote,
        }),
        error: result.err().map(protocol_error),
    })
}

fn rename_branch(
    checkout: &Checkout,
    request: protocol::CheckoutBranchRenameRequest,
) -> Result<Dispatch, ErrorCode> {
    let result = checkout.rename_branch(&request.cwd, &request.branch);
    value(protocol::CheckoutBranchRenameResult {
        success: result.is_ok(),
        cwd: request.cwd,
        current_branch: result.as_ref().ok().cloned(),
        error: result.err().map(protocol_error),
    })
}

fn commit(
    checkout: &Checkout,
    request: protocol::CheckoutCommitRequest,
) -> Result<Dispatch, ErrorCode> {
    let message = request.message.unwrap_or_default();
    mutation(
        request.cwd.clone(),
        checkout.commit(
            &request.cwd,
            message.trim(),
            request.add_all.unwrap_or(true),
        ),
    )
}

fn merge_to_base(
    checkout: &Checkout,
    request: &protocol::CheckoutMergeRequest,
) -> Result<Dispatch, ErrorCode> {
    let strategy = match request
        .strategy
        .unwrap_or(protocol::CheckoutMergeStrategy::Merge)
    {
        protocol::CheckoutMergeStrategy::Merge => port::CheckoutMergeStrategy::Merge,
        protocol::CheckoutMergeStrategy::Squash => port::CheckoutMergeStrategy::Squash,
    };
    mutation(
        request.cwd.clone(),
        checkout.merge_to_base(
            &request.cwd,
            request.base_ref.as_deref(),
            strategy,
            request.require_clean_target.unwrap_or(false),
        ),
    )
}

fn merge_from_base(
    checkout: &Checkout,
    request: &protocol::CheckoutMergeFromBaseRequest,
) -> Result<Dispatch, ErrorCode> {
    mutation(
        request.cwd.clone(),
        checkout.merge_from_base(
            &request.cwd,
            request.base_ref.as_deref(),
            request.require_clean_target.unwrap_or(true),
        ),
    )
}

fn mutate_path(
    checkout: &Checkout,
    request: &protocol::CheckoutPathRequest,
    operation: fn(&Checkout, &str) -> Result<(), port::CheckoutRuntimeError>,
) -> Result<Dispatch, ErrorCode> {
    mutation(request.cwd.clone(), operation(checkout, &request.cwd))
}

fn discard_changes(
    checkout: &Checkout,
    request: &protocol::CheckoutDiscardChangesRequest,
) -> Result<Dispatch, ErrorCode> {
    if request.paths.is_empty() {
        return Err(ErrorCode::InvalidMessage);
    }
    mutation(
        request.cwd.clone(),
        checkout.discard_changes(&request.cwd, &request.paths),
    )
}

fn stash_save(
    checkout: &Checkout,
    request: &protocol::CheckoutStashSaveRequest,
) -> Result<Dispatch, ErrorCode> {
    mutation(
        request.cwd.clone(),
        checkout.stash_save(&request.cwd, request.branch.as_deref()),
    )
}

fn stash_pop(
    checkout: &Checkout,
    request: &protocol::CheckoutStashPopRequest,
) -> Result<Dispatch, ErrorCode> {
    mutation(
        request.cwd.clone(),
        checkout.stash_pop(&request.cwd, request.stash_index),
    )
}

fn stash_list(
    checkout: &Checkout,
    request: protocol::CheckoutStashListRequest,
) -> Result<Dispatch, ErrorCode> {
    let result = match checkout.stashes(&request.cwd, request.paseo_only.unwrap_or(true)) {
        Ok(entries) => protocol::CheckoutStashListResult {
            cwd: request.cwd,
            entries: entries
                .into_iter()
                .map(|entry| protocol::CheckoutStashEntry {
                    index: entry.index,
                    message: entry.message,
                    branch: entry.branch,
                    is_paseo: entry.is_paseo,
                })
                .collect(),
            error: None,
        },
        Err(error) => protocol::CheckoutStashListResult {
            cwd: request.cwd,
            entries: Vec::new(),
            error: Some(protocol_error(error)),
        },
    };
    value(result)
}

fn mutation(
    cwd: String,
    result: Result<(), port::CheckoutRuntimeError>,
) -> Result<Dispatch, ErrorCode> {
    value(protocol::CheckoutMutationResult {
        cwd,
        success: result.is_ok(),
        error: result.err().map(protocol_error),
    })
}

fn protocol_branch_suggestion(
    suggestion: port::CheckoutBranchSuggestion,
) -> protocol::CheckoutBranchSuggestion {
    protocol::CheckoutBranchSuggestion {
        name: suggestion.name,
        committer_date: suggestion.committer_date,
        has_local: suggestion.has_local,
        has_remote: suggestion.has_remote,
        local_ahead: suggestion.local_ahead,
        local_behind: suggestion.local_behind,
    }
}

async fn poll_diff(
    service: Arc<Mutex<Checkout>>,
    jobs: Arc<Semaphore>,
    cwd: String,
    compare: port::CheckoutDiffCompare,
) -> Option<Result<port::CheckoutDiff, port::CheckoutRuntimeError>> {
    let permit = jobs.try_acquire_owned().ok()?;
    tokio::task::spawn_blocking(move || {
        let _permit = permit;
        let checkout = service
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        checkout.diff(&cwd, &compare)
    })
    .await
    .ok()
}

fn protocol_status(
    cwd: &str,
    result: Result<port::CheckoutStatus, port::CheckoutRuntimeError>,
) -> protocol::CheckoutStatusResult {
    match result {
        Ok(status) => protocol::CheckoutStatusResult {
            cwd: cwd.to_owned(),
            is_git: status.is_git,
            repo_root: status.repo_root,
            main_repo_root: status.main_repo_root,
            current_branch: status.current_branch,
            is_dirty: status.is_dirty,
            base_ref: status.base_ref,
            ahead_behind: status.ahead_behind.map(|counts| protocol::AheadBehind {
                ahead: counts.ahead,
                behind: counts.behind,
            }),
            upstream_ref: status.upstream_ref,
            ahead_of_origin: status.ahead_of_origin,
            behind_of_origin: status.behind_of_origin,
            has_remote: status.has_remote,
            remote_url: status.remote_url,
            is_paseo_owned_worktree: status.is_managed_worktree,
            error: None,
        },
        Err(error) => protocol::CheckoutStatusResult {
            cwd: cwd.to_owned(),
            is_git: false,
            repo_root: None,
            main_repo_root: None,
            current_branch: None,
            is_dirty: None,
            base_ref: None,
            ahead_behind: None,
            upstream_ref: None,
            ahead_of_origin: None,
            behind_of_origin: None,
            has_remote: false,
            remote_url: None,
            is_paseo_owned_worktree: false,
            error: Some(protocol_error(error)),
        },
    }
}

fn protocol_diff_result(
    cwd: &str,
    result: Result<port::CheckoutDiff, port::CheckoutRuntimeError>,
) -> protocol::CheckoutDiffResult {
    match result {
        Ok(result) => protocol::CheckoutDiffResult {
            cwd: cwd.to_owned(),
            files: result.files.into_iter().map(protocol_diff_file).collect(),
            error: None,
            diff_too_large: result.diff_too_large.then_some(true),
        },
        Err(error) => protocol::CheckoutDiffResult {
            cwd: cwd.to_owned(),
            files: Vec::new(),
            error: Some(protocol_error(error)),
            diff_too_large: None,
        },
    }
}

fn protocol_diff_file(file: port::ParsedDiffFile) -> protocol::ParsedDiffFile {
    protocol::ParsedDiffFile {
        path: file.path,
        old_path: file.old_path,
        is_new: file.is_new,
        is_deleted: file.is_deleted,
        additions: file.additions,
        deletions: file.deletions,
        hunks: file
            .hunks
            .into_iter()
            .map(|hunk| protocol::DiffHunk {
                old_start: hunk.old_start,
                old_count: hunk.old_count,
                new_start: hunk.new_start,
                new_count: hunk.new_count,
                lines: hunk
                    .lines
                    .into_iter()
                    .map(|line| protocol::DiffLine {
                        kind: match line.kind {
                            port::DiffLineKind::Add => protocol::DiffLineKind::Add,
                            port::DiffLineKind::Remove => protocol::DiffLineKind::Remove,
                            port::DiffLineKind::Context => protocol::DiffLineKind::Context,
                            port::DiffLineKind::Header => protocol::DiffLineKind::Header,
                        },
                        content: line.content,
                        tokens: None,
                    })
                    .collect(),
            })
            .collect(),
        status: file.status.map(|status| match status {
            port::ParsedDiffStatus::Ok => protocol::ParsedDiffStatus::Ok,
            port::ParsedDiffStatus::TooLarge => protocol::ParsedDiffStatus::TooLarge,
            port::ParsedDiffStatus::Binary => protocol::ParsedDiffStatus::Binary,
        }),
    }
}

fn protocol_commit(commit: port::CheckoutCommit) -> protocol::CheckoutCommit {
    protocol::CheckoutCommit {
        sha: commit.sha,
        short_sha: commit.short_sha,
        subject: commit.subject,
        author_name: commit.author_name,
        author_date: commit.author_date,
        is_on_remote: commit.is_on_remote,
        is_on_base: commit.is_on_base,
        files: commit
            .files
            .into_iter()
            .map(|file| protocol::CheckoutCommitFile {
                path: file.path,
                additions: file.additions,
                deletions: file.deletions,
                status: file.status.map(|status| match status {
                    port::CheckoutCommitFileStatus::Added => {
                        protocol::CheckoutCommitFileStatus::Added
                    }
                    port::CheckoutCommitFileStatus::Modified => {
                        protocol::CheckoutCommitFileStatus::Modified
                    }
                    port::CheckoutCommitFileStatus::Deleted => {
                        protocol::CheckoutCommitFileStatus::Deleted
                    }
                    port::CheckoutCommitFileStatus::Renamed => {
                        protocol::CheckoutCommitFileStatus::Renamed
                    }
                }),
            })
            .collect(),
    }
}

fn port_compare(compare: protocol::CheckoutDiffCompare) -> port::CheckoutDiffCompare {
    port::CheckoutDiffCompare {
        mode: match compare.mode {
            protocol::CheckoutDiffMode::Uncommitted => port::CheckoutDiffMode::Uncommitted,
            protocol::CheckoutDiffMode::Base => port::CheckoutDiffMode::Base,
        },
        base_ref: compare.base_ref,
        ignore_whitespace: compare.ignore_whitespace,
    }
}

fn protocol_error(error: port::CheckoutRuntimeError) -> protocol::CheckoutError {
    protocol::CheckoutError {
        code: match error.kind {
            port::CheckoutFailureKind::NotGitRepository => protocol::CheckoutErrorCode::NotGitRepo,
            port::CheckoutFailureKind::NotAllowed => protocol::CheckoutErrorCode::NotAllowed,
            port::CheckoutFailureKind::MergeConflict => protocol::CheckoutErrorCode::MergeConflict,
            port::CheckoutFailureKind::Unknown => protocol::CheckoutErrorCode::Unknown,
        },
        message: error.message,
    }
}

fn decode<T: serde::de::DeserializeOwned>(value: Value) -> Result<T, ErrorCode> {
    serde_json::from_value(value).map_err(|_| ErrorCode::InvalidMessage)
}

fn value(value: impl Serialize) -> Result<Dispatch, ErrorCode> {
    Ok(Dispatch {
        value: encode(value)?,
        subscription: None,
    })
}

fn encode(value: impl Serialize) -> Result<Value, ErrorCode> {
    serde_json::to_value(value).map_err(|_| ErrorCode::ProjectIo)
}

#[cfg(test)]
mod tests;
