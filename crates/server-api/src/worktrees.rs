use chrono::{SecondsFormat, Utc};
use serde::Serialize;
use serde_json::Value;
use server_application::worktrees::{
    ArchiveScope, ArchiveWorktree, CreateAction, CreateWorktree, WorktreeFailureKind, Worktrees,
    WorktreesError,
};
use server_protocol::ErrorCode;
use server_protocol::worktrees::{
    CheckoutError, CheckoutErrorCode, WorktreeArchiveRequest, WorktreeArchiveResult,
    WorktreeArchiveScope, WorktreeCreateAction, WorktreeCreateRequest, WorktreeCreateResult,
    WorktreeListEntry, WorktreeListRequest, WorktreeListResult,
};

use crate::Shared;

/// Completed dispatch plus an optional workspace event sent after the response.
pub(super) struct Dispatched {
    pub(super) value: Value,
    pub(super) event: Option<Value>,
    created_workspace_id: Option<String>,
}

pub(super) async fn dispatch(
    method: &str,
    params: Value,
    state: &Shared,
) -> Result<Dispatched, ErrorCode> {
    let method = method.to_owned();
    let dispatched = crate::jobs::run(
        state,
        state.worktrees.clone(),
        ErrorCode::RegistryIo,
        move |worktrees| execute(worktrees, &method, params),
    )
    .await?;
    if let Some(workspace_id) = dispatched.created_workspace_id.clone() {
        let _ = crate::jobs::run(
            state,
            state.workspace_automation.clone(),
            ErrorCode::RegistryIo,
            move |automation| {
                automation
                    .start_created_setup(&workspace_id)
                    .map(|_| ())
                    .map_err(|_| ErrorCode::RegistryIo)
            },
        )
        .await;
    }
    Ok(dispatched)
}

fn execute(
    worktrees: &mut Worktrees,
    method: &str,
    params: Value,
) -> Result<Dispatched, ErrorCode> {
    match method {
        "workspace.worktree.list.request" => list(worktrees, decode(params)?),
        "workspace.worktree.create.request" => create(worktrees, decode(params)?),
        "workspace.worktree.archive.request" => archive(worktrees, decode(params)?),
        _ => Err(ErrorCode::MethodNotFound),
    }
}

fn list(worktrees: &Worktrees, request: WorktreeListRequest) -> Result<Dispatched, ErrorCode> {
    let Some(cwd) = request.repo_root.or(request.cwd) else {
        return value(WorktreeListResult {
            worktrees: Vec::new(),
            error: Some(CheckoutError {
                code: CheckoutErrorCode::Unknown,
                message: "cwd or repoRoot is required".to_owned(),
            }),
        });
    };
    match worktrees.list(&cwd) {
        Ok(entries) => value(WorktreeListResult {
            worktrees: entries
                .into_iter()
                .map(|entry| WorktreeListEntry {
                    worktree_path: entry.path,
                    created_at: entry.created_at,
                    branch_name: entry.branch_name,
                    head: entry.head,
                })
                .collect(),
            error: None,
        }),
        Err(error) => value(WorktreeListResult {
            worktrees: Vec::new(),
            error: Some(checkout_error(&error)),
        }),
    }
}

fn create(worktrees: &Worktrees, request: WorktreeCreateRequest) -> Result<Dispatched, ErrorCode> {
    let context = request.normalized_first_agent_context();
    let input = CreateWorktree {
        cwd: request.cwd,
        project_id: request.project_id,
        worktree_slug: request.worktree_slug,
        ref_name: request.ref_name,
        action: match request.action.unwrap_or(WorktreeCreateAction::BranchOff) {
            WorktreeCreateAction::BranchOff => CreateAction::BranchOff,
            WorktreeCreateAction::Checkout => CreateAction::Checkout,
        },
        has_change_request_source: request.checkout_source.is_some()
            || request.github_pr_number.is_some(),
        first_agent_prompt: context.as_ref().and_then(|context| context.prompt.clone()),
        expects_initial_agent: context.is_some(),
    };
    match worktrees.create(&input, &timestamp()) {
        Ok(created) => {
            let descriptor =
                crate::directory::workspace_descriptor(&created.workspace, Some(&created.project));
            let event = serde_json::json!({
                "kind": "upsert",
                "workspace": descriptor.clone(),
            });
            dispatched(
                WorktreeCreateResult {
                    workspace: Some(descriptor),
                    error: None,
                    error_code: None,
                    setup_terminal_id: None,
                    setup_skipped_reason: None,
                },
                Some(event),
                Some(created.workspace.workspace_id),
            )
        }
        Err(error) => dispatched(
            WorktreeCreateResult {
                workspace: None,
                error: Some(error.to_string()),
                error_code: Some(create_error_code(&error).to_owned()),
                setup_terminal_id: None,
                setup_skipped_reason: None,
            },
            None,
            None,
        ),
    }
}

fn archive(
    worktrees: &Worktrees,
    request: WorktreeArchiveRequest,
) -> Result<Dispatched, ErrorCode> {
    let input = ArchiveWorktree {
        worktree_path: request.worktree_path,
        repo_root: request.repo_root,
        worktree_slug: None,
        branch_name: request.branch_name,
        workspace_id: request.workspace_id,
        scope: match request.scope {
            WorktreeArchiveScope::Workspace => ArchiveScope::Workspace,
            WorktreeArchiveScope::Worktree => ArchiveScope::Worktree,
        },
    };
    match worktrees.archive(&input, &timestamp()) {
        Ok(_) => value(WorktreeArchiveResult {
            success: true,
            removed_agents: Some(Vec::new()),
            error: None,
        }),
        Err(error) => value(WorktreeArchiveResult {
            success: false,
            removed_agents: Some(Vec::new()),
            error: Some(checkout_error(&error)),
        }),
    }
}

fn checkout_error(error: &WorktreesError) -> CheckoutError {
    let code = checkout_error_code(error.kind());
    CheckoutError {
        code,
        message: error.to_string(),
    }
}

const fn checkout_error_code(kind: WorktreeFailureKind) -> CheckoutErrorCode {
    match kind {
        WorktreeFailureKind::NotGitRepository => CheckoutErrorCode::NotGitRepo,
        WorktreeFailureKind::NotAllowed => CheckoutErrorCode::NotAllowed,
        _ => CheckoutErrorCode::Unknown,
    }
}

fn create_error_code(error: &WorktreesError) -> &'static str {
    create_error_code_for_kind(error.kind())
}

const fn create_error_code_for_kind(kind: WorktreeFailureKind) -> &'static str {
    match kind {
        WorktreeFailureKind::BranchAlreadyCheckedOut => "branch_already_checked_out",
        WorktreeFailureKind::MissingCheckoutTarget => "missing_checkout_target",
        WorktreeFailureKind::UnknownBranch => "unknown_branch",
        _ => "unknown",
    }
}

fn decode<T: serde::de::DeserializeOwned>(value: Value) -> Result<T, ErrorCode> {
    serde_json::from_value(value).map_err(|_| ErrorCode::InvalidMessage)
}

fn value(value: impl Serialize) -> Result<Dispatched, ErrorCode> {
    dispatched(value, None, None)
}

fn dispatched(
    value: impl Serialize,
    event: Option<Value>,
    created_workspace_id: Option<String>,
) -> Result<Dispatched, ErrorCode> {
    Ok(Dispatched {
        value: serde_json::to_value(value).map_err(|_| ErrorCode::RegistryIo)?,
        event,
        created_workspace_id,
    })
}

fn timestamp() -> String {
    Utc::now().to_rfc3339_opts(SecondsFormat::Millis, true)
}

#[cfg(test)]
mod tests;
