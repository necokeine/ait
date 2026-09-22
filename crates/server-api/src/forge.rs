use serde::Serialize;
use serde_json::Value;
use server_application::forge::{self as port, Forge};
use server_protocol::ErrorCode;
use server_protocol::checkout::{CheckoutError, CheckoutErrorCode};
use server_protocol::forge as protocol;

use crate::Shared;

pub(super) async fn dispatch(
    method: &str,
    params: Value,
    state: &Shared,
) -> Result<Value, ErrorCode> {
    let method = method.to_owned();
    crate::jobs::run(
        state,
        state.forge.clone(),
        ErrorCode::ProjectIo,
        move |forge| execute(forge, &method, params),
    )
    .await
}

fn execute(forge: &Forge, method: &str, params: Value) -> Result<Value, ErrorCode> {
    match method {
        "forge.search.request" => search(forge, &decode(params)?, false),
        "github.search.request" => search(forge, &decode(params)?, true),
        "checkout.pr.create.request" => create(forge, decode(params)?),
        "checkout.pr.merge.request" => merge(forge, &decode(params)?),
        "checkout.pr.status.request" => status(forge, &decode(params)?),
        "checkout.pr.timeline.request" => timeline(forge, decode(params)?),
        "checkout.forge.set_auto_merge.request" | "checkout.github.set_auto_merge.request" => {
            auto_merge(forge, decode(params)?)
        }
        "checkout.forge.get_check_details.request"
        | "checkout.github.get_check_details.request" => check_details(forge, decode(params)?),
        _ => Err(ErrorCode::MethodNotFound),
    }
}

fn search(
    forge: &Forge,
    request: &protocol::ForgeSearchRequest,
    github_compatibility: bool,
) -> Result<Value, ErrorCode> {
    let limit = request.limit.unwrap_or(20);
    if !(1..=50).contains(&limit) {
        return Err(ErrorCode::InvalidMessage);
    }
    let kinds = normalized_kinds(request.kinds.as_deref());
    let result = forge.search(&request.cwd, &request.query, limit, &kinds);
    match (github_compatibility, result) {
        (false, Ok(result)) => encode(protocol::ForgeSearchResult {
            items: result
                .items
                .into_iter()
                .map(|item| protocol_search_item(item, false))
                .collect(),
            auth_state: Some(protocol_auth_state(result.auth_state)),
            error: None,
        }),
        (true, Ok(result)) => {
            let enabled = result.auth_state == port::ForgeAuthState::Authenticated;
            encode(protocol::GithubSearchResult {
                items: result
                    .items
                    .into_iter()
                    .map(|item| protocol_search_item(item, true))
                    .collect(),
                features_enabled: enabled,
                auth_state: Some(protocol_auth_state(result.auth_state)),
                github_features_enabled: enabled,
                error: None,
            })
        }
        (false, Err(error)) => encode(protocol::ForgeSearchResult {
            items: Vec::new(),
            auth_state: Some(protocol_auth_state(auth_state_for_error(&error))),
            error: Some(error.message),
        }),
        (true, Err(error)) => encode(protocol::GithubSearchResult {
            items: Vec::new(),
            features_enabled: false,
            auth_state: Some(protocol_auth_state(auth_state_for_error(&error))),
            github_features_enabled: false,
            error: Some(error.message),
        }),
    }
}

fn create(forge: &Forge, request: protocol::PullRequestCreateRequest) -> Result<Value, ErrorCode> {
    let result = forge.create_pull_request(
        &request.cwd,
        request.title.as_deref().unwrap_or_default(),
        request.body.as_deref().unwrap_or_default(),
        request.base_ref.as_deref(),
    );
    match result {
        Ok(created) => encode(protocol::PullRequestCreateResult {
            cwd: request.cwd,
            url: Some(created.url),
            number: Some(created.number),
            error: None,
        }),
        Err(error) => encode(protocol::PullRequestCreateResult {
            cwd: request.cwd,
            url: None,
            number: None,
            error: Some(protocol_error(error)),
        }),
    }
}

fn merge(forge: &Forge, request: &protocol::PullRequestMergeRequest) -> Result<Value, ErrorCode> {
    mutation(
        &request.cwd,
        forge.merge_current_pull_request(&request.cwd, port_merge_method(request.merge_method)),
    )
}

fn auto_merge(
    forge: &Forge,
    request: protocol::PullRequestAutoMergeRequest,
) -> Result<Value, ErrorCode> {
    let result = forge.set_current_pull_request_auto_merge(
        &request.cwd,
        request.enabled,
        request.merge_method.map(port_merge_method),
    );
    match result {
        Ok(()) => encode(protocol::PullRequestAutoMergeResult {
            cwd: request.cwd,
            enabled: request.enabled,
            success: true,
            error: None,
        }),
        Err(error) => encode(protocol::PullRequestAutoMergeResult {
            cwd: request.cwd,
            enabled: request.enabled,
            success: false,
            error: Some(protocol_error(error)),
        }),
    }
}

fn status(forge: &Forge, request: &protocol::ForgePathRequest) -> Result<Value, ErrorCode> {
    let result = forge.current_pull_request_status(&request.cwd);
    match result {
        Ok(read) => encode(protocol::PullRequestStatusResult {
            cwd: request.cwd.clone(),
            status: read.status.map(protocol_status),
            github_features_enabled: matches!(
                read.auth_state,
                port::ForgeAuthState::Authenticated | port::ForgeAuthState::Error
            ),
            auth_state: Some(protocol_auth_state(read.auth_state)),
            forge: read.forge,
            error: None,
        }),
        Err(error) => {
            let auth_state = auth_state_for_error(&error);
            encode(protocol::PullRequestStatusResult {
                cwd: request.cwd.clone(),
                status: None,
                github_features_enabled: matches!(
                    auth_state,
                    port::ForgeAuthState::Authenticated | port::ForgeAuthState::Error
                ),
                auth_state: Some(protocol_auth_state(auth_state)),
                forge: (auth_state != port::ForgeAuthState::NoRemote).then(|| "github".to_owned()),
                error: Some(protocol_error(error)),
            })
        }
    }
}

fn timeline(
    forge: &Forge,
    request: protocol::PullRequestTimelineRequest,
) -> Result<Value, ErrorCode> {
    if request.pr_number == 0
        || !valid_repo_segment(&request.repo_owner)
        || !valid_repo_segment(&request.repo_name)
    {
        return encode(protocol::PullRequestTimelineResult {
            cwd: request.cwd,
            pr_number: Some(request.pr_number),
            items: Vec::new(),
            truncated: false,
            error: Some(protocol::TimelineError {
                kind: protocol::TimelineErrorKind::Unknown,
                message: "Pull request timeline request has invalid PR identity".to_owned(),
            }),
            github_features_enabled: true,
            auth_state: None,
        });
    }
    match forge.pull_request_timeline(
        &request.cwd,
        request.pr_number,
        &request.repo_owner,
        &request.repo_name,
    ) {
        Ok(timeline) => encode(protocol::PullRequestTimelineResult {
            cwd: request.cwd,
            pr_number: Some(timeline.pr_number),
            items: timeline
                .items
                .into_iter()
                .map(protocol_timeline_item)
                .collect(),
            truncated: timeline.truncated,
            error: timeline.error.map(protocol_timeline_error),
            github_features_enabled: timeline.auth_state == port::ForgeAuthState::Authenticated,
            auth_state: (timeline.auth_state != port::ForgeAuthState::Authenticated)
                .then(|| protocol_auth_state(timeline.auth_state)),
        }),
        Err(error) => {
            let auth_state = auth_state_for_error(&error);
            let features = !matches!(
                auth_state,
                port::ForgeAuthState::NoRemote
                    | port::ForgeAuthState::CliMissing
                    | port::ForgeAuthState::Unauthenticated
            );
            encode(protocol::PullRequestTimelineResult {
                cwd: request.cwd,
                pr_number: Some(request.pr_number),
                items: Vec::new(),
                truncated: false,
                error: Some(protocol::TimelineError {
                    kind: protocol::TimelineErrorKind::Unknown,
                    message: error.message,
                }),
                github_features_enabled: features,
                auth_state: Some(protocol_auth_state(auth_state)),
            })
        }
    }
}

fn check_details(
    forge: &Forge,
    request: protocol::CheckDetailsRequest,
) -> Result<Value, ErrorCode> {
    if request
        .repo_owner
        .as_deref()
        .is_some_and(|value| !valid_repo_segment(value))
        || request
            .repo_name
            .as_deref()
            .is_some_and(|value| !valid_repo_segment(value))
        || request.check_run_id == Some(0)
        || request.workflow_run_id == Some(0)
        || request.change_request_number == Some(0)
    {
        return Err(ErrorCode::InvalidMessage);
    }
    let result = forge.check_details(
        &request.cwd,
        request.repo_owner.as_deref(),
        request.repo_name.as_deref(),
        request.check_run_id,
        request.workflow_run_id,
        request.change_request_number,
    );
    match result {
        Ok(details) => encode(protocol::CheckDetailsResult {
            cwd: request.cwd,
            success: true,
            details: Some(protocol_check_details(details)),
            error: None,
        }),
        Err(error) => encode(protocol::CheckDetailsResult {
            cwd: request.cwd,
            success: false,
            details: None,
            error: Some(protocol_error(error)),
        }),
    }
}

fn mutation(cwd: &str, result: Result<(), port::ForgeRuntimeError>) -> Result<Value, ErrorCode> {
    match result {
        Ok(()) => encode(protocol::PullRequestMutationResult {
            cwd: cwd.to_owned(),
            success: true,
            error: None,
        }),
        Err(error) => encode(protocol::PullRequestMutationResult {
            cwd: cwd.to_owned(),
            success: false,
            error: Some(protocol_error(error)),
        }),
    }
}

fn normalized_kinds(kinds: Option<&[protocol::ForgeSearchKind]>) -> Vec<port::ForgeSearchKind> {
    let values = kinds.unwrap_or(&[
        protocol::ForgeSearchKind::Issue,
        protocol::ForgeSearchKind::ChangeRequest,
    ]);
    let mut normalized = Vec::new();
    for kind in values {
        let kind = match kind {
            protocol::ForgeSearchKind::Issue | protocol::ForgeSearchKind::GithubIssue => {
                port::ForgeSearchKind::Issue
            }
            protocol::ForgeSearchKind::ChangeRequest
            | protocol::ForgeSearchKind::GithubPr
            | protocol::ForgeSearchKind::Pr => port::ForgeSearchKind::ChangeRequest,
        };
        if !normalized.contains(&kind) {
            normalized.push(kind);
        }
    }
    normalized
}

fn protocol_search_item(item: port::ForgeSearchItem, legacy: bool) -> protocol::ForgeSearchItem {
    protocol::ForgeSearchItem {
        kind: match item.kind {
            port::ForgeSearchKind::Issue => "issue",
            port::ForgeSearchKind::ChangeRequest if legacy => "pr",
            port::ForgeSearchKind::ChangeRequest => "change_request",
        }
        .to_owned(),
        forge: item.forge,
        number: item.number,
        title: item.title,
        url: item.url,
        state: item.state,
        body: item.body,
        labels: item.labels,
        project_path: item.project_path,
        base_ref_name: item.base_ref_name,
        head_ref_name: item.head_ref_name,
        updated_at: item.updated_at,
    }
}

fn protocol_status(status: port::PullRequestStatus) -> protocol::PullRequestStatus {
    protocol::PullRequestStatus {
        forge: status.forge,
        project_path: status.project_path,
        number: status.number,
        url: status.url,
        title: status.title,
        state: status.state,
        base_ref_name: status.base_ref_name,
        head_ref_name: status.head_ref_name,
        is_merged: status.is_merged,
        is_draft: status.is_draft,
        mergeable: match status.mergeable {
            port::PullRequestMergeable::Mergeable => protocol::PullRequestMergeable::Mergeable,
            port::PullRequestMergeable::Conflicting => protocol::PullRequestMergeable::Conflicting,
            port::PullRequestMergeable::Unknown => protocol::PullRequestMergeable::Unknown,
        },
        checks: status
            .checks
            .into_iter()
            .map(|check| protocol::PullRequestCheck {
                name: check.name,
                status: check.status,
                url: check.url,
                workflow: check.workflow,
                duration: check.duration,
                check_run_id: check.check_run_id,
                workflow_run_id: check.workflow_run_id,
                traits: check.traits,
            })
            .collect(),
        checks_status: status.checks_status,
        review_decision: status.review_decision,
        repo_owner: status.repo_owner,
        repo_name: status.repo_name,
        github: status.github,
        forge_specific: status.forge_specific,
    }
}

fn protocol_timeline_item(
    item: port::PullRequestTimelineItem,
) -> protocol::PullRequestTimelineItem {
    match item {
        port::PullRequestTimelineItem::Review {
            id,
            author,
            author_url,
            avatar_url,
            body,
            created_at,
            url,
            review_state,
        } => protocol::PullRequestTimelineItem::Review {
            id,
            author,
            author_url,
            avatar_url,
            body,
            created_at,
            url,
            review_state: match review_state {
                port::TimelineReviewState::Approved => protocol::TimelineReviewState::Approved,
                port::TimelineReviewState::ChangesRequested => {
                    protocol::TimelineReviewState::ChangesRequested
                }
                port::TimelineReviewState::Commented => protocol::TimelineReviewState::Commented,
            },
        },
        port::PullRequestTimelineItem::Comment {
            id,
            author,
            author_url,
            avatar_url,
            body,
            created_at,
            url,
            review_id,
            thread_id,
            thread_is_resolved,
            location,
        } => protocol::PullRequestTimelineItem::Comment {
            id,
            author,
            author_url,
            avatar_url,
            body,
            created_at,
            url,
            review_id,
            thread_id,
            thread_is_resolved,
            location: location.map(|location| protocol::TimelineCommentLocation {
                path: location.path,
                line: location.line,
                start_line: location.start_line,
                thread_id: location.thread_id,
                is_resolved: location.is_resolved,
                is_outdated: location.is_outdated,
            }),
        },
    }
}

fn protocol_timeline_error(error: port::TimelineError) -> protocol::TimelineError {
    protocol::TimelineError {
        kind: match error.kind {
            port::TimelineErrorKind::NotFound => protocol::TimelineErrorKind::NotFound,
            port::TimelineErrorKind::Forbidden => protocol::TimelineErrorKind::Forbidden,
            port::TimelineErrorKind::Unknown => protocol::TimelineErrorKind::Unknown,
        },
        message: error.message,
    }
}

fn protocol_check_details(details: port::CheckDetails) -> protocol::CheckDetails {
    protocol::CheckDetails {
        check_run_id: details.check_run_id,
        workflow_run_id: details.workflow_run_id,
        name: details.name,
        status: details.status,
        conclusion: details.conclusion,
        url: details.url,
        details_url: details.details_url,
        output: details.output.map(|output| protocol::CheckOutput {
            title: output.title,
            summary: output.summary,
            text: output.text,
        }),
        annotations: details
            .annotations
            .into_iter()
            .map(|annotation| protocol::CheckAnnotation {
                path: annotation.path,
                start_line: annotation.start_line,
                end_line: annotation.end_line,
                annotation_level: annotation.annotation_level,
                message: annotation.message,
                title: annotation.title,
                raw_details: annotation.raw_details,
            })
            .collect(),
        failed_jobs: details
            .failed_jobs
            .into_iter()
            .map(|job| protocol::CheckFailedJob {
                job_id: job.job_id,
                name: job.name,
                status: job.status,
                conclusion: job.conclusion,
                url: job.url,
                log_tail: job.log_tail,
                log_truncated: job.log_truncated,
            })
            .collect(),
        truncated: details.truncated,
        pipeline: details.pipeline,
    }
}

const fn port_merge_method(
    method: protocol::PullRequestMergeMethod,
) -> port::PullRequestMergeMethod {
    match method {
        protocol::PullRequestMergeMethod::Merge => port::PullRequestMergeMethod::Merge,
        protocol::PullRequestMergeMethod::Squash => port::PullRequestMergeMethod::Squash,
        protocol::PullRequestMergeMethod::Rebase => port::PullRequestMergeMethod::Rebase,
    }
}

const fn protocol_auth_state(state: port::ForgeAuthState) -> protocol::ForgeAuthState {
    match state {
        port::ForgeAuthState::Authenticated => protocol::ForgeAuthState::Authenticated,
        port::ForgeAuthState::Unauthenticated => protocol::ForgeAuthState::Unauthenticated,
        port::ForgeAuthState::CliMissing => protocol::ForgeAuthState::CliMissing,
        port::ForgeAuthState::NoRemote => protocol::ForgeAuthState::NoRemote,
        port::ForgeAuthState::Error => protocol::ForgeAuthState::Error,
    }
}

const fn auth_state_for_error(error: &port::ForgeRuntimeError) -> port::ForgeAuthState {
    match error.kind {
        port::ForgeFailureKind::CliMissing => port::ForgeAuthState::CliMissing,
        port::ForgeFailureKind::Unauthenticated => port::ForgeAuthState::Unauthenticated,
        port::ForgeFailureKind::NoRemote => port::ForgeAuthState::NoRemote,
        _ => port::ForgeAuthState::Error,
    }
}

fn protocol_error(error: port::ForgeRuntimeError) -> CheckoutError {
    CheckoutError {
        code: match error.kind {
            port::ForgeFailureKind::NotGitRepository => CheckoutErrorCode::NotGitRepo,
            port::ForgeFailureKind::NotAllowed => CheckoutErrorCode::NotAllowed,
            port::ForgeFailureKind::MergeConflict => CheckoutErrorCode::MergeConflict,
            _ => CheckoutErrorCode::Unknown,
        },
        message: error.message,
    }
}

fn valid_repo_segment(value: &str) -> bool {
    !value.is_empty()
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b'-'))
}

fn decode<T: serde::de::DeserializeOwned>(value: Value) -> Result<T, ErrorCode> {
    serde_json::from_value(value).map_err(|_| ErrorCode::InvalidMessage)
}

fn encode(value: impl Serialize) -> Result<Value, ErrorCode> {
    serde_json::to_value(value).map_err(|_| ErrorCode::ProjectIo)
}

#[cfg(test)]
mod tests;
