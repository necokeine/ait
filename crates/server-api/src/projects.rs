use std::path::Path;

use serde_json::Value;
use server_application::{ProjectError, ProjectView, Projects};
use server_domain::OwnerEpoch;
use server_protocol::{ErrorCode, project};

use crate::Shared;

pub(super) async fn dispatch(
    method: &str,
    params: Value,
    state: &Shared,
) -> Result<Value, ErrorCode> {
    let projects = state
        .projects
        .as_ref()
        .ok_or(ErrorCode::UnsupportedCapability)?
        .clone();
    // One admitted blocking project job per server, with no unbounded waiting queue.
    // Its tracking token survives cancellation of the connection's response future.
    let admission = state
        .admission
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    if state.cancellation.is_cancelled() {
        return Err(ErrorCode::ServerDraining);
    }
    let permit = state
        .project_jobs
        .clone()
        .try_acquire_owned()
        .map_err(|_| ErrorCode::ResourceExhausted)?;
    let tracking = state.tasks.token();
    let method = method.to_owned();
    let job = tokio::task::spawn_blocking(move || {
        let (_tracking, _permit) = (tracking, permit);
        let mut projects = projects.lock().map_err(|_| ErrorCode::ProjectIo)?;
        execute(&mut projects, &method, params)
    });
    drop(admission);
    job.await.map_err(|_| ErrorCode::ProjectIo)?
}

fn execute(projects: &mut Projects, method: &str, params: Value) -> Result<Value, ErrorCode> {
    match method {
        "project.open" => {
            let request: project::Open =
                serde_json::from_value(params).map_err(|_| ErrorCode::InvalidMessage)?;
            let path = Path::new(&request.path);
            if !path.is_absolute()
                || request.path.len() > 4096
                || request.path.chars().any(char::is_control)
            {
                return Err(ErrorCode::InvalidMessage);
            }
            let receipt = projects
                .open(path, &request.idempotency_key)
                .map_err(error)?;
            encode(project::Receipt {
                operation_id: receipt.operation_id.to_string(),
                project_id: receipt.project_id.to_string(),
            })
        }
        "project.get" => {
            let request: project::Get =
                serde_json::from_value(params).map_err(|_| ErrorCode::InvalidMessage)?;
            let id = request
                .project_id
                .parse()
                .map_err(|_| ErrorCode::InvalidMessage)?;
            encode(view(projects.get(id).map_err(error)?))
        }
        "project.list" => {
            let request: project::List =
                serde_json::from_value(params).map_err(|_| ErrorCode::InvalidMessage)?;
            let after = request
                .after
                .as_deref()
                .map(str::parse)
                .transpose()
                .map_err(|_| ErrorCode::InvalidMessage)?;
            let entries = projects.list(after, request.limit).map_err(error)?;
            let next_after = if entries.len() == request.limit {
                entries.last().map(|entry| entry.entry.id.to_string())
            } else {
                None
            };
            encode(project::Page {
                projects: entries.into_iter().map(view).collect(),
                next_after,
            })
        }
        "project.close" => {
            let request: project::Close =
                serde_json::from_value(params).map_err(|_| ErrorCode::InvalidMessage)?;
            let id = request
                .project_id
                .parse()
                .map_err(|_| ErrorCode::InvalidMessage)?;
            let epoch =
                OwnerEpoch::new(request.owner_epoch).map_err(|_| ErrorCode::InvalidMessage)?;
            let receipt = projects
                .close(id, epoch, &request.idempotency_key)
                .map_err(error)?;
            encode(project::Receipt {
                operation_id: receipt.operation_id.to_string(),
                project_id: receipt.project_id.to_string(),
            })
        }
        _ => Err(ErrorCode::MethodNotFound),
    }
}

fn encode(value: impl serde::Serialize) -> Result<Value, ErrorCode> {
    serde_json::to_value(value).map_err(|_| ErrorCode::ProjectIo)
}

fn view(view: ProjectView) -> project::Project {
    let entry = view.entry;
    project::Project {
        project_id: entry.id.to_string(),
        path: entry.path.to_string_lossy().into_owned(),
        name: entry.name,
        base_commit: entry.base_commit.to_string(),
        root_message_id: entry.root_message_id.to_string(),
        created_at: entry.created_at,
        owner_epoch: view.owner_epoch.map(OwnerEpoch::value),
    }
}

fn error(error: ProjectError) -> ErrorCode {
    match error {
        ProjectError::Invalid => ErrorCode::InvalidMessage,
        ProjectError::UnsupportedWorkspace => ErrorCode::UnsupportedWorkspace,
        ProjectError::LegacyProject => ErrorCode::LegacyProject,
        ProjectError::Busy => ErrorCode::ProjectBusy,
        ProjectError::UnsupportedFormat => ErrorCode::UnsupportedFormat,
        ProjectError::IdempotencyConflict => ErrorCode::IdempotencyConflict,
        ProjectError::IdentityConflict => ErrorCode::IdentityConflict,
        ProjectError::NotFound => ErrorCode::ProjectNotFound,
        ProjectError::NotOpen => ErrorCode::ProjectNotOpen,
        ProjectError::StaleOwner => ErrorCode::StaleOwner,
        ProjectError::Io => ErrorCode::ProjectIo,
    }
}

#[cfg(test)]
mod tests;
