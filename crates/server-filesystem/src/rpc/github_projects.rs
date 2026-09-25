//! GitHub provisioning payload validation and projection.
use chrono::{SecondsFormat, Utc};
use serde::Serialize;
use serde_json::Value;
use server_metadata::rpc::directory::project_descriptor;

use crate::protocol::github_projects::{
    GithubCloneProtocol as WireCloneProtocol, GithubRepositoryPayload,
    GithubRepositorySearchRequest, GithubRepositorySearchResult, GithubRepositorySearchStatus,
    GithubRepositoryVisibility as WireVisibility, ProjectGithubCloneRequest,
    ProjectGithubCloneResult,
};
use crate::rpc::ErrorCode;
use crate::service::github_projects::{
    GithubCloneProtocol, GithubProjects, GithubProjectsError, GithubRepositoryVisibility,
};
/// Execute a GitHub repository search or clone request.
///
/// # Errors
/// Rejects unknown methods, invalid parameters, or result encoding failures.
pub fn execute(
    directory: &GithubProjects,
    method: &str,
    params: Value,
) -> Result<Value, ErrorCode> {
    match method {
        "project.github.clone.request" => project_github_clone(directory, &decode(params)?),
        "workspace.github.search_repositories.request" => {
            github_repository_search(directory, &decode(params)?)
        }
        _ => Err(ErrorCode::MethodNotFound),
    }
}
fn github_repository_search(
    directory: &GithubProjects,
    request: &GithubRepositorySearchRequest,
) -> Result<Value, ErrorCode> {
    let limit = request.limit.unwrap_or(20);
    if !(1..=50).contains(&limit) {
        return Err(ErrorCode::InvalidMessage);
    }
    match directory.search_github_repositories(&request.query, limit) {
        Ok(repositories) => encode(GithubRepositorySearchResult {
            status: GithubRepositorySearchStatus::Success,
            repositories: repositories
                .into_iter()
                .map(|repository| GithubRepositoryPayload {
                    id: repository.id,
                    name: repository.name,
                    name_with_owner: repository.name_with_owner,
                    description: repository.description,
                    visibility: match repository.visibility {
                        GithubRepositoryVisibility::Public => WireVisibility::Public,
                        GithubRepositoryVisibility::Private => WireVisibility::Private,
                    },
                    updated_at: repository.updated_at,
                    clone_url: repository.clone_url,
                })
                .collect(),
            available: true,
            reason: None,
            error: None,
        }),
        Err(error) => {
            let (status, available, reason) = match error {
                GithubProjectsError::CliMissing => (
                    GithubRepositorySearchStatus::Unavailable,
                    false,
                    Some("gh_missing"),
                ),
                GithubProjectsError::Unauthenticated => {
                    (GithubRepositorySearchStatus::Unauthenticated, false, None)
                }
                GithubProjectsError::SearchFailed
                | GithubProjectsError::InvalidTarget
                | GithubProjectsError::TargetExists
                | GithubProjectsError::CloneFailed => {
                    (GithubRepositorySearchStatus::Error, true, None)
                }
            };
            encode(GithubRepositorySearchResult {
                status,
                repositories: Vec::new(),
                available,
                reason,
                error: Some(error.to_string()),
            })
        }
    }
}

fn project_github_clone(
    directory: &GithubProjects,
    request: &ProjectGithubCloneRequest,
) -> Result<Value, ErrorCode> {
    if request.repo.trim().len() < 3 || request.target_directory.trim().is_empty() {
        return Err(ErrorCode::InvalidMessage);
    }
    let protocol = request.clone_protocol.map(|protocol| match protocol {
        WireCloneProtocol::Https => GithubCloneProtocol::Https,
        WireCloneProtocol::Ssh => GithubCloneProtocol::Ssh,
    });
    let outcome = directory.clone_github_project(
        &request.repo,
        protocol,
        &request.target_directory,
        &timestamp(),
    );
    encode(ProjectGithubCloneResult {
        repo: outcome.repo,
        checkout_path: outcome.checkout_path,
        project: outcome.project.as_ref().map(project_descriptor),
        error: outcome.error,
    })
}

fn decode<T: serde::de::DeserializeOwned>(value: Value) -> Result<T, ErrorCode> {
    serde_json::from_value(value).map_err(|_| ErrorCode::InvalidMessage)
}
fn encode(value: impl Serialize) -> Result<Value, ErrorCode> {
    serde_json::to_value(value).map_err(|_| ErrorCode::RegistryIo)
}
fn timestamp() -> String {
    Utc::now().to_rfc3339_opts(SecondsFormat::Millis, true)
}
