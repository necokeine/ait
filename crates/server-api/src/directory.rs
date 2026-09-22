use std::cmp::Ordering;
use std::path::Path;

use base64::Engine;
use chrono::{SecondsFormat, Utc};
use serde::Serialize;
use serde_json::Value;
use server_application::directory::{Directory, DirectoryError};
use server_domain::registry::{
    PersistedProjectKind, PersistedProjectRecord, PersistedWorkspaceKind, PersistedWorkspaceRecord,
};
use server_protocol::ErrorCode;
use server_protocol::directory::{
    ProjectAddRequest, ProjectAddResult, ProjectCreateDirectoryRequest,
    ProjectCreateDirectoryResult, ProjectListRequest, ProjectListResult, ProjectRemoveRequest,
    ProjectRemoveResult, ProjectRenameRequest, ProjectRenameResult, SortDirection,
    WorkspaceArchiveRequest, WorkspaceArchiveResult, WorkspaceCreateRequest, WorkspaceCreateResult,
    WorkspaceCreateSource, WorkspaceListRequest, WorkspaceListResult, WorkspaceOpenRequest,
    WorkspaceOpenResult, WorkspacePageInfo, WorkspacePinSetRequest, WorkspacePinSetResult,
    WorkspaceSort, WorkspaceSortKey, WorkspaceTitleSetRequest, WorkspaceTitleSetResult,
};
use server_protocol::project_config::{
    PaseoConfigRaw, PaseoConfigRevision, ProjectConfigReadRequest, ProjectConfigReadResult,
    ProjectConfigRpcError, ProjectConfigWriteRequest, ProjectConfigWriteResult,
};
use server_protocol::project_icon::{
    ProjectIconGetRequest, ProjectIconGetResult, ProjectIconPayload, ProjectIconSetRequest,
    ProjectIconSetResult, ProjectIconSource,
};
use server_protocol::workspace::{
    ProjectKind, WorkspaceDescriptorPayload, WorkspaceKind, WorkspaceProjectDescriptorPayload,
    WorkspaceStateBucket,
};

use crate::Shared;

pub(super) async fn dispatch(
    method: &str,
    params: Value,
    state: &Shared,
) -> Result<Value, ErrorCode> {
    let method = method.to_owned();
    crate::jobs::run(
        state,
        state.directory.clone(),
        ErrorCode::RegistryIo,
        move |directory| execute(directory, &method, params),
    )
    .await
}

fn execute(directory: &mut Directory, method: &str, params: Value) -> Result<Value, ErrorCode> {
    match method {
        "project.add.request" => project_add(directory, &decode(params)?),
        "project.create_directory.request" => project_create_directory(directory, &decode(params)?),
        "project.config.read.request" => project_config_read(directory, decode(params)?),
        "project.config.write.request" => project_config_write(directory, decode(params)?),
        "project.icon.set.request" => project_icon_set(directory, decode(params)?),
        "project.icon.get.request" => project_icon_get(directory, decode(params)?),
        "project.list.request" => project_list(directory, &decode(params)?),
        "project.rename.request" => project_rename(directory, decode(params)?),
        "project.remove.request" => project_remove(directory, decode(params)?),
        "workspace.open.request" => workspace_open(directory, &decode(params)?),
        "workspace.create.request" => workspace_create(directory, decode(params)?),
        "workspace.list.request" => workspace_list(directory, &decode(params)?),
        "workspace.archive.request" => workspace_archive(directory, decode(params)?),
        "workspace.title.set.request" => workspace_title_set(directory, decode(params)?),
        "workspace.pin.set.request" => workspace_pin_set(directory, decode(params)?),
        _ => Err(ErrorCode::MethodNotFound),
    }
}

fn project_icon_set(
    directory: &Directory,
    request: ProjectIconSetRequest,
) -> Result<Value, ErrorCode> {
    let upload = match request.source {
        ProjectIconSource::Automatic => None,
        ProjectIconSource::Upload { data } => {
            let Ok(bytes) = base64::engine::general_purpose::STANDARD.decode(data) else {
                return encode(ProjectIconSetResult {
                    project_id: request.project_id,
                    accepted: false,
                    error: Some("Unsupported or invalid icon file".to_owned()),
                });
            };
            Some(bytes)
        }
    };
    let timestamp = timestamp();
    match directory.set_project_icon(&request.project_id, upload.as_deref(), &timestamp) {
        Ok(Some(_)) => encode(ProjectIconSetResult {
            project_id: request.project_id,
            accepted: true,
            error: None,
        }),
        Ok(None) | Err(DirectoryError::UnknownProject) => encode(ProjectIconSetResult {
            project_id: request.project_id,
            accepted: false,
            error: Some("Project not found".to_owned()),
        }),
        Err(DirectoryError::Registry) => Err(ErrorCode::RegistryIo),
        Err(error) => encode(ProjectIconSetResult {
            project_id: request.project_id,
            accepted: false,
            error: Some(error.to_string()),
        }),
    }
}

fn project_icon_get(
    directory: &Directory,
    request: ProjectIconGetRequest,
) -> Result<Value, ErrorCode> {
    match directory.get_project_icon(&request.project_id) {
        Ok(icon) => encode(ProjectIconGetResult {
            project_id: request.project_id,
            icon: icon.map(|icon| ProjectIconPayload {
                data: base64::engine::general_purpose::STANDARD.encode(icon.bytes),
                mime_type: icon.mime_type,
            }),
            error: None,
        }),
        Err(DirectoryError::UnknownProject) => encode(ProjectIconGetResult {
            project_id: request.project_id,
            icon: None,
            error: Some("Project not found".to_owned()),
        }),
        Err(DirectoryError::Registry) => Err(ErrorCode::RegistryIo),
        Err(error) => encode(ProjectIconGetResult {
            project_id: request.project_id,
            icon: None,
            error: Some(error.to_string()),
        }),
    }
}

fn project_config_read(
    directory: &Directory,
    request: ProjectConfigReadRequest,
) -> Result<Value, ErrorCode> {
    match directory.read_project_config(&request.repo_root) {
        Ok(result) => {
            let config = match result.config {
                Some(config) => match PaseoConfigRaw::new(config) {
                    Ok(config) => Some(config),
                    Err(_) => {
                        return encode(ProjectConfigReadResult::Failure {
                            repo_root: result.repo_root,
                            error: ProjectConfigRpcError::InvalidProjectConfig,
                        });
                    }
                },
                None => None,
            };
            encode(ProjectConfigReadResult::Success {
                repo_root: result.repo_root,
                config,
                revision: result.revision.map(protocol_revision),
            })
        }
        Err(DirectoryError::UnknownProject) => encode(ProjectConfigReadResult::Failure {
            repo_root: request.repo_root,
            error: ProjectConfigRpcError::ProjectNotFound,
        }),
        Err(DirectoryError::Registry) => Err(ErrorCode::RegistryIo),
        Err(
            DirectoryError::DirectoryNotFound
            | DirectoryError::InvalidDirectoryName
            | DirectoryError::ParentDirectoryNotFound
            | DirectoryError::DirectoryExists
            | DirectoryError::PermissionDenied
            | DirectoryError::FileSystem
            | DirectoryError::RegistrationFailed { .. }
            | DirectoryError::ArchivedProject
            | DirectoryError::InvalidProjectConfig
            | DirectoryError::StaleProjectConfig { .. }
            | DirectoryError::ProjectConfigWriteFailed
            | DirectoryError::InvalidProjectIcon
            | DirectoryError::ProjectIconStorage,
        ) => encode(ProjectConfigReadResult::Failure {
            repo_root: request.repo_root,
            error: ProjectConfigRpcError::InvalidProjectConfig,
        }),
    }
}

fn project_config_write(
    directory: &Directory,
    request: ProjectConfigWriteRequest,
) -> Result<Value, ErrorCode> {
    let expected_revision = request.expected_revision.map(port_revision);
    match directory.write_project_config(
        &request.repo_root,
        request.config.value(),
        expected_revision,
    ) {
        Ok(result) => encode(ProjectConfigWriteResult::Success {
            repo_root: result.repo_root,
            config: PaseoConfigRaw::new(result.config).map_err(|_| ErrorCode::RegistryIo)?,
            revision: protocol_revision(result.revision),
        }),
        Err(DirectoryError::UnknownProject) => encode(ProjectConfigWriteResult::Failure {
            repo_root: request.repo_root,
            error: ProjectConfigRpcError::ProjectNotFound,
        }),
        Err(DirectoryError::StaleProjectConfig { current_revision }) => {
            encode(ProjectConfigWriteResult::Failure {
                repo_root: request.repo_root,
                error: ProjectConfigRpcError::StaleProjectConfig {
                    current_revision: current_revision.map(protocol_revision),
                },
            })
        }
        Err(DirectoryError::InvalidProjectConfig) => encode(ProjectConfigWriteResult::Failure {
            repo_root: request.repo_root,
            error: ProjectConfigRpcError::InvalidProjectConfig,
        }),
        Err(DirectoryError::ProjectConfigWriteFailed) => {
            encode(ProjectConfigWriteResult::Failure {
                repo_root: request.repo_root,
                error: ProjectConfigRpcError::WriteFailed,
            })
        }
        Err(DirectoryError::Registry) => Err(ErrorCode::RegistryIo),
        Err(_) => encode(ProjectConfigWriteResult::Failure {
            repo_root: request.repo_root,
            error: ProjectConfigRpcError::WriteFailed,
        }),
    }
}

fn project_add(directory: &Directory, request: &ProjectAddRequest) -> Result<Value, ErrorCode> {
    let timestamp = timestamp();
    match directory.add_project(&request.cwd, &timestamp) {
        Ok(project) => encode(ProjectAddResult {
            project: Some(project_descriptor(&project)),
            error: None,
            error_code: None,
        }),
        Err(DirectoryError::DirectoryNotFound) => encode(ProjectAddResult {
            project: None,
            error: Some(format!("Directory not found: {}", request.cwd)),
            error_code: Some("directory_not_found".to_owned()),
        }),
        Err(DirectoryError::Registry) => Err(ErrorCode::RegistryIo),
        Err(error) => encode(ProjectAddResult {
            project: None,
            error: Some(error.to_string()),
            error_code: None,
        }),
    }
}

fn project_create_directory(
    directory: &Directory,
    request: &ProjectCreateDirectoryRequest,
) -> Result<Value, ErrorCode> {
    let timestamp = timestamp();
    match directory.create_project_directory(&request.parent_path, &request.name, &timestamp) {
        Ok((directory_path, project)) => encode(ProjectCreateDirectoryResult {
            directory_path: Some(directory_path),
            project: Some(project_descriptor(&project)),
            error: None,
            error_code: None,
        }),
        Err(DirectoryError::Registry) => Err(ErrorCode::RegistryIo),
        Err(error) => {
            let directory_path = match &error {
                DirectoryError::RegistrationFailed { directory_path, .. } => {
                    Some(directory_path.clone())
                }
                _ => None,
            };
            encode(ProjectCreateDirectoryResult {
                directory_path,
                project: None,
                error: Some(error.to_string()),
                error_code: Some(directory_create_error_code(&error).to_owned()),
            })
        }
    }
}

fn workspace_open(
    directory: &Directory,
    request: &WorkspaceOpenRequest,
) -> Result<Value, ErrorCode> {
    let timestamp = timestamp();
    match directory.open_workspace(&request.cwd, &timestamp) {
        Ok(workspace) => encode(WorkspaceOpenResult {
            workspace: Some(describe_workspace(directory, &workspace)?),
            error: None,
            error_code: None,
        }),
        Err(DirectoryError::DirectoryNotFound) => encode(WorkspaceOpenResult {
            workspace: None,
            error: Some(format!("Directory not found: {}", request.cwd)),
            error_code: Some("directory_not_found".to_owned()),
        }),
        Err(DirectoryError::Registry) => Err(ErrorCode::RegistryIo),
        Err(error) => encode(WorkspaceOpenResult {
            workspace: None,
            error: Some(error.to_string()),
            error_code: None,
        }),
    }
}

fn workspace_create(
    directory: &Directory,
    request: WorkspaceCreateRequest,
) -> Result<Value, ErrorCode> {
    if request.agent.is_some()
        || request.subscribe == Some(true)
        || request.idempotency_key.is_some()
        || request.first_agent_context.is_some()
    {
        return Err(ErrorCode::UnsupportedCapability);
    }
    let WorkspaceCreateSource::Directory { path, project_id } = request.source else {
        return Err(ErrorCode::UnsupportedCapability);
    };
    let timestamp = timestamp();
    match directory.create_workspace(
        &path,
        request.title,
        project_id.as_deref(),
        request.workspace_id,
        false,
        &timestamp,
    ) {
        Ok(workspace) => encode(WorkspaceCreateResult {
            workspace: Some(describe_workspace(directory, &workspace)?),
            setup_terminal_id: None,
            error: None,
            error_code: None,
        }),
        Err(DirectoryError::Registry) => Err(ErrorCode::RegistryIo),
        Err(error) => encode(WorkspaceCreateResult {
            workspace: None,
            setup_terminal_id: None,
            error: Some(error.to_string()),
            error_code: workspace_create_error_code(&error).map(str::to_owned),
        }),
    }
}

fn project_list(directory: &Directory, request: &ProjectListRequest) -> Result<Value, ErrorCode> {
    if request.sync.is_some() {
        return Err(ErrorCode::UnsupportedCapability);
    }
    let projects = directory
        .list_projects()
        .map_err(directory_error)?
        .into_iter()
        .filter(active_project)
        .map(|project| project_descriptor(&project))
        .collect();
    encode(ProjectListResult { projects })
}

fn project_rename(
    directory: &Directory,
    request: ProjectRenameRequest,
) -> Result<Value, ErrorCode> {
    let custom_name = normalize_optional_text(request.custom_name);
    let timestamp = timestamp();
    let updated = directory
        .rename_project(&request.project_id, custom_name.as_deref(), &timestamp)
        .map_err(directory_error)?;
    encode(match updated {
        Some(_) => ProjectRenameResult {
            project_id: request.project_id,
            accepted: true,
            custom_name,
            error: None,
        },
        None => ProjectRenameResult {
            project_id: request.project_id,
            accepted: false,
            custom_name: None,
            error: Some("Project not found".to_owned()),
        },
    })
}

fn project_remove(
    directory: &Directory,
    request: ProjectRemoveRequest,
) -> Result<Value, ErrorCode> {
    let timestamp = timestamp();
    let active_workspace_ids = directory
        .remove_project(&request.project_id, &timestamp)
        .map_err(directory_error)?;
    encode(ProjectRemoveResult {
        project_id: request.project_id,
        accepted: true,
        removed_workspace_ids: active_workspace_ids,
        error: None,
    })
}

fn workspace_list(
    directory: &Directory,
    request: &WorkspaceListRequest,
) -> Result<Value, ErrorCode> {
    if request.subscribe.is_some() || request.sync.is_some() {
        return Err(ErrorCode::UnsupportedCapability);
    }
    let projects = directory
        .list_projects()
        .map_err(directory_error)?
        .into_iter()
        .filter(active_project)
        .map(|project| (project.project_id.clone(), project))
        .collect::<std::collections::BTreeMap<_, _>>();
    let all_active = directory
        .list_workspaces()
        .map_err(directory_error)?
        .into_iter()
        .filter(active_workspace)
        .collect::<Vec<_>>();
    let mut entries = all_active
        .iter()
        .filter(|workspace| matches_filter(workspace, projects.get(&workspace.project_id), request))
        .map(|workspace| workspace_descriptor(workspace, projects.get(&workspace.project_id)))
        .collect::<Vec<_>>();
    sort_workspaces(&mut entries, request.sort.as_deref().unwrap_or_default());
    let (start, limit) = page_bounds(request.page.as_ref(), entries.len())?;
    let end = start.saturating_add(limit).min(entries.len());
    let has_more = end < entries.len();
    let page_entries = entries[start..end].to_vec();
    let active_project_ids = all_active
        .iter()
        .map(|workspace| workspace.project_id.as_str())
        .collect::<std::collections::BTreeSet<_>>();
    let empty_projects = if start == 0 {
        projects
            .values()
            .filter(|project| !active_project_ids.contains(project.project_id.as_str()))
            .filter(|project| project_matches_filter(project, request))
            .map(project_descriptor)
            .collect()
    } else {
        Vec::new()
    };
    encode(WorkspaceListResult {
        entries: page_entries,
        empty_projects,
        page_info: WorkspacePageInfo {
            next_cursor: has_more.then(|| end.to_string()),
            prev_cursor: (start > 0).then(|| start.saturating_sub(limit).to_string()),
            has_more,
        },
    })
}

fn workspace_archive(
    directory: &Directory,
    request: WorkspaceArchiveRequest,
) -> Result<Value, ErrorCode> {
    let archived_at = timestamp();
    if directory
        .archive_workspace(&request.workspace_id, &archived_at)
        .map_err(directory_error)?
        .is_none()
    {
        return encode(WorkspaceArchiveResult {
            workspace_id: request.workspace_id.clone(),
            archived_at: None,
            error: Some(format!("Workspace not found: {}", request.workspace_id)),
        });
    }
    encode(WorkspaceArchiveResult {
        workspace_id: request.workspace_id,
        archived_at: Some(archived_at),
        error: None,
    })
}

fn workspace_title_set(
    directory: &Directory,
    request: WorkspaceTitleSetRequest,
) -> Result<Value, ErrorCode> {
    let title = normalize_optional_text(request.title);
    let timestamp = timestamp();
    let updated = directory
        .set_workspace_title(&request.workspace_id, title.as_deref(), &timestamp)
        .map_err(directory_error)?;
    encode(match updated {
        Some(_) => WorkspaceTitleSetResult {
            workspace_id: request.workspace_id,
            accepted: true,
            title,
            error: None,
        },
        None => WorkspaceTitleSetResult {
            workspace_id: request.workspace_id,
            accepted: false,
            title: None,
            error: Some("Workspace not found".to_owned()),
        },
    })
}

fn workspace_pin_set(
    directory: &Directory,
    request: WorkspacePinSetRequest,
) -> Result<Value, ErrorCode> {
    let timestamp = timestamp();
    let pinned_at = request.pinned.then(|| timestamp.clone());
    let updated = directory
        .set_workspace_pin(&request.workspace_id, pinned_at.as_deref(), &timestamp)
        .map_err(directory_error)?;
    encode(match updated {
        Some(_) => WorkspacePinSetResult {
            workspace_id: request.workspace_id,
            accepted: true,
            pinned_at,
            error: None,
        },
        None => WorkspacePinSetResult {
            workspace_id: request.workspace_id,
            accepted: false,
            pinned_at: None,
            error: Some("Workspace not found".to_owned()),
        },
    })
}

fn project_descriptor(project: &PersistedProjectRecord) -> WorkspaceProjectDescriptorPayload {
    WorkspaceProjectDescriptorPayload {
        project_id: project.project_id.clone(),
        project_key: project.project_key.clone(),
        project_display_name: project.display_name().to_owned(),
        project_custom_name: Some(project.custom_name.clone()),
        project_custom_icon_revision: Some(project.custom_icon_revision.clone()),
        project_icon_revision: None,
        project_root_path: project.root_path.clone(),
        project_kind: project_kind(project.kind),
        sync_seq: None,
    }
}

pub(super) fn workspace_descriptor(
    workspace: &PersistedWorkspaceRecord,
    project: Option<&PersistedProjectRecord>,
) -> WorkspaceDescriptorPayload {
    let project_display_name = project.map_or(workspace.project_id.as_str(), |project| {
        project.display_name()
    });
    let project_root_path = project.map_or_else(
        || {
            workspace
                .main_repo_root
                .clone()
                .unwrap_or_else(|| workspace.cwd.clone())
        },
        |project| project.root_path.clone(),
    );
    WorkspaceDescriptorPayload {
        id: workspace.workspace_id.clone(),
        project_id: workspace.project_id.clone(),
        project_display_name: project_display_name.to_owned(),
        project_custom_name: Some(project.and_then(|project| project.custom_name.clone())),
        project_custom_icon_revision: Some(
            project.and_then(|project| project.custom_icon_revision.clone()),
        ),
        project_root_path,
        workspace_directory: workspace.cwd.clone(),
        worktree_slug: workspace
            .worktree_root
            .as_deref()
            .and_then(|root| Path::new(root).file_name())
            .and_then(|name| name.to_str())
            .map(str::to_owned),
        project_kind: project.map_or_else(
            || match workspace.kind {
                PersistedWorkspaceKind::Directory => ProjectKind::NonGit,
                PersistedWorkspaceKind::LocalCheckout | PersistedWorkspaceKind::Worktree => {
                    ProjectKind::Git
                }
            },
            |project| project_kind(project.kind),
        ),
        workspace_kind: workspace_kind(workspace.kind),
        name: workspace.display_name().to_owned(),
        title: Some(workspace.title.clone()),
        pinned_at: Some(workspace.pinned_at.clone()),
        labels: workspace.labels.clone().filter(|labels| !labels.is_empty()),
        archiving_at: None,
        status: WorkspaceStateBucket::Done,
        status_entered_at: Some(workspace.created_at.clone()),
        activity_at: None,
        diff_stat: None,
        scripts: Vec::new(),
        git_runtime: None,
        github_runtime: None,
        forge: None,
        project: None,
        sync_seq: None,
    }
}

fn describe_workspace(
    directory: &Directory,
    workspace: &PersistedWorkspaceRecord,
) -> Result<WorkspaceDescriptorPayload, ErrorCode> {
    let project = directory
        .list_projects()
        .map_err(directory_error)?
        .into_iter()
        .find(|project| project.project_id == workspace.project_id);
    Ok(workspace_descriptor(workspace, project.as_ref()))
}

fn matches_filter(
    workspace: &PersistedWorkspaceRecord,
    project: Option<&PersistedProjectRecord>,
    request: &WorkspaceListRequest,
) -> bool {
    let Some(filter) = &request.filter else {
        return true;
    };
    if filter
        .project_id
        .as_deref()
        .is_some_and(|project_id| project_id != workspace.project_id)
    {
        return false;
    }
    let Some(query) = filter
        .query
        .as_deref()
        .map(str::trim)
        .filter(|q| !q.is_empty())
    else {
        return true;
    };
    let query = query.to_lowercase();
    [
        Some(workspace.display_name()),
        Some(workspace.cwd.as_str()),
        workspace.branch.as_deref(),
        project.map(PersistedProjectRecord::display_name),
    ]
    .into_iter()
    .flatten()
    .any(|candidate| candidate.to_lowercase().contains(&query))
}

fn project_matches_filter(
    project: &PersistedProjectRecord,
    request: &WorkspaceListRequest,
) -> bool {
    let Some(filter) = &request.filter else {
        return true;
    };
    if filter
        .project_id
        .as_deref()
        .is_some_and(|project_id| project_id != project.project_id)
    {
        return false;
    }
    filter
        .query
        .as_deref()
        .map(str::trim)
        .filter(|query| !query.is_empty())
        .is_none_or(|query| {
            let query = query.to_lowercase();
            project.display_name().to_lowercase().contains(&query)
                || project.root_path.to_lowercase().contains(&query)
        })
}

fn sort_workspaces(entries: &mut [WorkspaceDescriptorPayload], clauses: &[WorkspaceSort]) {
    entries.sort_by(|left, right| {
        clauses
            .iter()
            .map(|clause| {
                let ordering = match clause.key {
                    WorkspaceSortKey::StatusPriority => Ordering::Equal,
                    WorkspaceSortKey::ActivityAt => left.activity_at.cmp(&right.activity_at),
                    WorkspaceSortKey::Name => left.name.cmp(&right.name),
                    WorkspaceSortKey::ProjectId => left.project_id.cmp(&right.project_id),
                };
                match clause.direction {
                    SortDirection::Asc => ordering,
                    SortDirection::Desc => ordering.reverse(),
                }
            })
            .find(|ordering| *ordering != Ordering::Equal)
            .unwrap_or(Ordering::Equal)
    });
}

fn page_bounds(
    page: Option<&server_protocol::directory::WorkspacePage>,
    entry_count: usize,
) -> Result<(usize, usize), ErrorCode> {
    let Some(page) = page else {
        return Ok((0, entry_count));
    };
    if !(1..=200).contains(&page.limit) {
        return Err(ErrorCode::InvalidMessage);
    }
    let start = page
        .cursor
        .as_deref()
        .map(str::parse::<usize>)
        .transpose()
        .map_err(|_| ErrorCode::InvalidMessage)?
        .unwrap_or(0);
    if start > entry_count {
        return Err(ErrorCode::InvalidMessage);
    }
    Ok((start, page.limit))
}

fn active_project(project: &PersistedProjectRecord) -> bool {
    project.archived_at.as_ref().is_none_or(String::is_empty)
}

fn active_workspace(workspace: &PersistedWorkspaceRecord) -> bool {
    workspace.archived_at.as_ref().is_none_or(String::is_empty)
}

const fn project_kind(kind: PersistedProjectKind) -> ProjectKind {
    match kind {
        PersistedProjectKind::Git => ProjectKind::Git,
        PersistedProjectKind::NonGit => ProjectKind::NonGit,
    }
}

const fn workspace_kind(kind: PersistedWorkspaceKind) -> WorkspaceKind {
    match kind {
        PersistedWorkspaceKind::LocalCheckout => WorkspaceKind::LocalCheckout,
        PersistedWorkspaceKind::Worktree => WorkspaceKind::Worktree,
        PersistedWorkspaceKind::Directory => WorkspaceKind::Directory,
    }
}

fn normalize_optional_text(value: Option<String>) -> Option<String> {
    value
        .map(|value| value.trim().to_owned())
        .filter(|value| !value.is_empty())
}

fn timestamp() -> String {
    Utc::now().to_rfc3339_opts(SecondsFormat::Millis, true)
}

const fn directory_create_error_code(error: &DirectoryError) -> &'static str {
    match error {
        DirectoryError::InvalidDirectoryName => "invalid_name",
        DirectoryError::ParentDirectoryNotFound | DirectoryError::DirectoryNotFound => {
            "parent_directory_not_found"
        }
        DirectoryError::DirectoryExists => "directory_exists",
        DirectoryError::PermissionDenied => "permission_denied",
        DirectoryError::RegistrationFailed { .. }
        | DirectoryError::UnknownProject
        | DirectoryError::ArchivedProject => "registration_failed",
        DirectoryError::FileSystem
        | DirectoryError::Registry
        | DirectoryError::InvalidProjectConfig
        | DirectoryError::StaleProjectConfig { .. }
        | DirectoryError::ProjectConfigWriteFailed
        | DirectoryError::InvalidProjectIcon
        | DirectoryError::ProjectIconStorage => "filesystem_error",
    }
}

const fn workspace_create_error_code(error: &DirectoryError) -> Option<&'static str> {
    match error {
        DirectoryError::DirectoryNotFound => Some("directory_not_found"),
        DirectoryError::UnknownProject => Some("unknown_project"),
        DirectoryError::ArchivedProject => Some("archived_project"),
        DirectoryError::InvalidDirectoryName
        | DirectoryError::ParentDirectoryNotFound
        | DirectoryError::DirectoryExists
        | DirectoryError::PermissionDenied
        | DirectoryError::FileSystem
        | DirectoryError::RegistrationFailed { .. }
        | DirectoryError::Registry
        | DirectoryError::InvalidProjectConfig
        | DirectoryError::StaleProjectConfig { .. }
        | DirectoryError::ProjectConfigWriteFailed
        | DirectoryError::InvalidProjectIcon
        | DirectoryError::ProjectIconStorage => None,
    }
}

const fn protocol_revision(
    revision: server_application::directory::ProjectConfigRevision,
) -> PaseoConfigRevision {
    PaseoConfigRevision {
        mtime_ms: revision.mtime_ms,
        size: revision.size,
    }
}

const fn port_revision(
    revision: PaseoConfigRevision,
) -> server_application::directory::ProjectConfigRevision {
    server_application::directory::ProjectConfigRevision {
        mtime_ms: revision.mtime_ms,
        size: revision.size,
    }
}

fn decode<T: serde::de::DeserializeOwned>(value: Value) -> Result<T, ErrorCode> {
    serde_json::from_value(value).map_err(|_| ErrorCode::InvalidMessage)
}

fn encode(value: impl Serialize) -> Result<Value, ErrorCode> {
    serde_json::to_value(value).map_err(|_| ErrorCode::RegistryIo)
}

fn directory_error(_error: DirectoryError) -> ErrorCode {
    ErrorCode::RegistryIo
}
