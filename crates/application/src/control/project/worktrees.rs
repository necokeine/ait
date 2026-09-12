//! Manager-owned Session worktree preparation and validation.
use crate::control::catalog::require_agent;
use crate::control::conversation::derive_reuses_source;
use crate::control::conversation::messages::{
    message_workspace_commit, validate_message_text, validate_session_message,
};
use crate::control::errors::error;
use crate::control::project::archive::{validate_import_conflicts, validate_project_export};
use crate::control::project::git::{ensure_git_head, git_head, git_stdout, prepare_git_root};
use crate::control::project::require_project_view;
use crate::control::state::WorkingSet;
use ait_contracts::{ApiError, Command, ProjectExport, ProjectView, RunView};
use ait_domain::ErrorCode;
use std::fs::OpenOptions;
use std::io::Write as IoWrite;
use std::path::{Path, PathBuf};

pub(in crate::control) fn validate_session_path_component(id: &str) -> Result<(), ApiError> {
    // Session worktrees share .ait with the Project database and its sidecars.
    if [
        "project.sqlite3",
        "project.sqlite3-wal",
        "project.sqlite3-shm",
        "project.sqlite3-journal",
    ]
    .iter()
    .any(|reserved| {
        id.trim_end_matches([' ', '.'])
            .eq_ignore_ascii_case(reserved)
    }) {
        return Err(error(
            ErrorCode::InvalidSession,
            "session id is reserved for project storage",
            false,
        ));
    }
    if id.trim().is_empty()
        || id == "."
        || id == ".."
        || id.contains('/')
        || id.contains('\\')
        || id.chars().any(char::is_control)
    {
        return Err(error(
            ErrorCode::InvalidSession,
            "session id must be a nonempty safe path component",
            false,
        ));
    }
    Ok(())
}

pub(in crate::control) fn session_worktree_path(
    project_workdir: &str,
    session_id: &str,
) -> Result<PathBuf, ApiError> {
    validate_session_path_component(session_id)?;
    Ok(Path::new(project_workdir).join(".ait").join(session_id))
}

pub(in crate::control) fn run_workdir(
    state: &WorkingSet,
    run: &RunView,
) -> Result<PathBuf, ApiError> {
    let project = state
        .projects
        .iter()
        .find(|project| project.id == run.project_id)
        .ok_or_else(|| error(ErrorCode::InvalidProject, "project not found", false))?;
    let Some(session_id) = run.session_id.as_deref() else {
        return Ok(PathBuf::from(&project.workdir));
    };
    let session = state
        .sessions
        .iter()
        .find(|session| session.id == session_id && session.project_id == run.project_id)
        .ok_or_else(|| error(ErrorCode::SessionNotFound, "run Session not found", false))?;
    let expected = session_worktree_path(&project.workdir, session_id)?;
    if Path::new(&session.workdir) != expected {
        return Err(error(
            ErrorCode::InvalidSession,
            "Session workdir is outside its manager-owned worktree path",
            false,
        ));
    }
    Ok(expected)
}

pub(in crate::control) fn prepare_command_session_worktrees(
    state: &WorkingSet,
    command: &Command,
    created: &mut Vec<PathBuf>,
) -> Result<(), ApiError> {
    match command {
        Command::ImportProject { archive, workdir } => {
            prepare_import_session_worktrees(state, archive, workdir, created)
        }
        Command::CreateSession {
            id,
            project_id,
            agent_id,
            at_message_id,
        } => {
            let project = require_project_view(state, project_id)?;
            let message_id = at_message_id
                .as_deref()
                .unwrap_or(project.root_message_id.as_str());
            prepare_new_session_worktree(state, id, project_id, agent_id, message_id, created)
        }
        Command::ForkSession {
            id,
            project_id,
            agent_id,
            at_message_id,
            text,
        } => {
            validate_message_text(text)?;
            prepare_new_session_worktree(state, id, project_id, agent_id, at_message_id, created)
        }
        Command::DeriveSession {
            id,
            project_id,
            source_session_id,
            agent_id,
            at_message_id,
            text,
        } => prepare_derived_session_worktree(
            state,
            id,
            project_id,
            source_session_id,
            agent_id,
            at_message_id,
            text,
            created,
        ),
        Command::SendMessage { session_id, .. } => {
            prepare_existing_session_worktree(state, session_id, created)
        }
        _ => Ok(()),
    }
}

fn prepare_import_session_worktrees(
    state: &WorkingSet,
    archive: &ProjectExport,
    workdir: &str,
    created: &mut Vec<PathBuf>,
) -> Result<(), ApiError> {
    validate_project_export(archive)?;
    validate_import_conflicts(state, archive)?;
    let canonical = prepare_git_root(Path::new(workdir))?;
    let mut project = archive.project.clone();
    project.workdir = canonical.to_string_lossy().into_owned();
    project.base_commit = ensure_git_head(&canonical)?;
    for session in &archive.sessions {
        ensure_session_worktree(&project, &session.id, &project.base_commit, created)?;
    }
    Ok(())
}

fn prepare_new_session_worktree(
    state: &WorkingSet,
    id: &str,
    project_id: &str,
    agent_id: &str,
    message_id: &str,
    created: &mut Vec<PathBuf>,
) -> Result<(), ApiError> {
    validate_session_path_component(id)?;
    if state.sessions.iter().any(|session| session.id == id) {
        return Err(error(
            ErrorCode::InvalidSession,
            "session id is already registered",
            false,
        ));
    }
    require_agent(state, agent_id)?;
    let project = require_project_view(state, project_id)?;
    validate_session_message(state, project_id, message_id)?;
    let baseline = message_workspace_commit(state, project, message_id)?;
    ensure_session_worktree(project, id, &baseline, created)
}

#[allow(clippy::too_many_arguments)]
fn prepare_derived_session_worktree(
    state: &WorkingSet,
    id: &str,
    project_id: &str,
    source_session_id: &str,
    agent_id: &str,
    at_message_id: &str,
    text: &str,
    created: &mut Vec<PathBuf>,
) -> Result<(), ApiError> {
    validate_message_text(text)?;
    validate_session_path_component(id)?;
    let source = state
        .sessions
        .iter()
        .find(|session| session.id == source_session_id)
        .ok_or_else(|| error(ErrorCode::SessionNotFound, "session not found", false))?;
    if !derive_reuses_source(state, id, project_id, source, agent_id, at_message_id) {
        return prepare_new_session_worktree(
            state,
            id,
            project_id,
            agent_id,
            at_message_id,
            created,
        );
    }
    let project = require_project_view(state, project_id)?;
    let baseline = message_workspace_commit(state, project, &source.current_message_id)?;
    ensure_session_worktree(project, &source.id, &baseline, created)
}

fn prepare_existing_session_worktree(
    state: &WorkingSet,
    session_id: &str,
    created: &mut Vec<PathBuf>,
) -> Result<(), ApiError> {
    let session = state
        .sessions
        .iter()
        .find(|session| session.id == session_id)
        .ok_or_else(|| error(ErrorCode::SessionNotFound, "session not found", false))?;
    let project = require_project_view(state, &session.project_id)?;
    let baseline =
        git_head(Path::new(&session.workdir))?.unwrap_or_else(|| project.base_commit.clone());
    ensure_session_worktree(project, &session.id, &baseline, created)
}

fn ensure_session_worktree(
    project: &ProjectView,
    session_id: &str,
    baseline: &str,
    created: &mut Vec<PathBuf>,
) -> Result<(), ApiError> {
    let primary = Path::new(&project.workdir);
    let worktree = session_worktree_path(&project.workdir, session_id)?;
    let parent = worktree.parent().expect("Session worktree has .ait parent");
    validate_session_worktree_parent(parent)?;
    if validate_existing_session_worktree(&worktree)? {
        return Ok(());
    }
    if created.iter().any(|path| path == &worktree) {
        return Ok(());
    }
    ensure_ait_excluded(primary)?;
    ensure_session_worktree_parent(parent)?;
    validate_session_baseline(primary, baseline)?;
    add_session_worktree(primary, &worktree, baseline)?;
    if let Err(mut failure) = git_stdout(&worktree, &["reset", "--hard", baseline]) {
        failure.message = format!(
            "{}; partial Session worktree retained at {}",
            failure.message,
            worktree.display()
        );
        return Err(failure);
    }
    created.push(worktree);
    Ok(())
}

fn validate_session_worktree_parent(parent: &Path) -> Result<bool, ApiError> {
    match std::fs::symlink_metadata(parent) {
        Ok(metadata) if !metadata.file_type().is_symlink() && metadata.is_dir() => Ok(true),
        Ok(_) => Err(error(
            ErrorCode::InvalidSession,
            "Project .ait path must be a real directory",
            false,
        )),
        Err(failure) if failure.kind() == std::io::ErrorKind::NotFound => Ok(false),
        Err(failure) => Err(error(
            ErrorCode::ProjectGitInitFailed,
            format!("cannot inspect Project .ait directory: {failure}"),
            false,
        )),
    }
}

fn validate_existing_session_worktree(worktree: &Path) -> Result<bool, ApiError> {
    let metadata = match std::fs::symlink_metadata(worktree) {
        Ok(metadata) => metadata,
        Err(failure) if failure.kind() == std::io::ErrorKind::NotFound => return Ok(false),
        Err(failure) => {
            return Err(error(
                ErrorCode::ProjectPathNotFound,
                format!("cannot inspect Session worktree: {failure}"),
                false,
            ));
        }
    };
    if metadata.file_type().is_symlink() || !metadata.is_dir() {
        return Err(error(
            ErrorCode::InvalidSession,
            "Session worktree must be a real directory",
            false,
        ));
    }
    let top = git_stdout(worktree, &["rev-parse", "--show-toplevel"])?;
    let canonical = worktree.canonicalize().map_err(|failure| {
        error(
            ErrorCode::ProjectPathNotFound,
            format!("cannot resolve Session worktree: {failure}"),
            false,
        )
    })?;
    let top = PathBuf::from(top).canonicalize().map_err(|failure| {
        error(
            ErrorCode::InvalidSession,
            format!("cannot resolve Session Git root: {failure}"),
            false,
        )
    })?;
    if top != canonical {
        return Err(error(
            ErrorCode::InvalidSession,
            "Session workdir is not its own linked Git worktree",
            false,
        ));
    }
    Ok(true)
}

fn ensure_session_worktree_parent(parent: &Path) -> Result<(), ApiError> {
    if validate_session_worktree_parent(parent)? {
        return Ok(());
    }
    match std::fs::create_dir(parent) {
        Ok(()) => Ok(()),
        Err(failure) if failure.kind() == std::io::ErrorKind::AlreadyExists => {
            validate_session_worktree_parent(parent).map(|_| ())
        }
        Err(failure) => Err(error(
            ErrorCode::ProjectGitInitFailed,
            format!("cannot create Project .ait directory: {failure}"),
            false,
        )),
    }
}

fn validate_session_baseline(primary: &Path, baseline: &str) -> Result<(), ApiError> {
    let commit_expression = format!("{baseline}^{{commit}}");
    let verified = git_stdout(primary, &["rev-parse", "--verify", &commit_expression])?;
    if verified != baseline {
        return Err(error(
            ErrorCode::ProjectGitHeadUnavailable,
            "Session baseline does not resolve to the recorded commit",
            false,
        ));
    }
    Ok(())
}

fn add_session_worktree(primary: &Path, worktree: &Path, baseline: &str) -> Result<(), ApiError> {
    let worktree_text = worktree.to_string_lossy().into_owned();
    git_stdout(
        primary,
        &[
            "worktree",
            "add",
            "--detach",
            "--no-checkout",
            &worktree_text,
            baseline,
        ],
    )?;
    Ok(())
}

fn ensure_ait_excluded(primary: &Path) -> Result<(), ApiError> {
    let common = git_stdout(primary, &["rev-parse", "--git-common-dir"])?;
    let common = PathBuf::from(common);
    let common = if common.is_absolute() {
        common
    } else {
        primary.join(common)
    };
    let info = common.join("info");
    std::fs::create_dir_all(&info).map_err(|failure| {
        error(
            ErrorCode::ProjectGitInitFailed,
            format!("cannot create Git info directory: {failure}"),
            false,
        )
    })?;
    let exclude = info.join("exclude");
    let existing = std::fs::read_to_string(&exclude).unwrap_or_default();
    if existing.lines().any(|line| line.trim() == "/.ait/") {
        return Ok(());
    }
    let mut file = OpenOptions::new()
        .create(true)
        .append(true)
        .open(&exclude)
        .map_err(|failure| {
            error(
                ErrorCode::ProjectGitInitFailed,
                format!("cannot update Git info/exclude: {failure}"),
                false,
            )
        })?;
    if !existing.is_empty() && !existing.ends_with('\n') {
        IoWrite::write_all(&mut file, b"\n").map_err(|failure| {
            error(
                ErrorCode::ProjectGitInitFailed,
                format!("cannot update Git info/exclude: {failure}"),
                false,
            )
        })?;
    }
    IoWrite::write_all(&mut file, b"/.ait/\n").map_err(|failure| {
        error(
            ErrorCode::ProjectGitInitFailed,
            format!("cannot update Git info/exclude: {failure}"),
            false,
        )
    })
}
