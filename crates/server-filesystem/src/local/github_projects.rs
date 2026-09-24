//! Bounded host GitHub CLI discovery and staged Git clone.

use std::path::PathBuf;
use std::process::Command;
use std::time::Duration;

use serde_json::Value;

use crate::local::forge::run_bounded_command;
use crate::ports::forge::ForgeFailureKind;
use crate::ports::github_projects::{
    GithubProjectsError, GithubProjectsRuntime, GithubRepository, GithubRepositoryVisibility,
};

const SEARCH_TIMEOUT: Duration = Duration::from_secs(30);
const CLONE_TIMEOUT: Duration = Duration::from_secs(300);

/// GitHub CLI and Git adapter using only the host's existing credentials.
#[derive(Debug, Clone)]
pub struct LocalGithubProjects {
    gh_executable: PathBuf,
    git_executable: PathBuf,
    home: Option<PathBuf>,
}

impl Default for LocalGithubProjects {
    fn default() -> Self {
        Self::new()
    }
}

impl LocalGithubProjects {
    /// Resolve `gh` and `git` from PATH and use the host HOME for discovery.
    #[must_use]
    pub fn new() -> Self {
        Self {
            gh_executable: PathBuf::from("gh"),
            git_executable: PathBuf::from("git"),
            home: std::env::var_os("HOME").map(PathBuf::from),
        }
    }

    #[cfg(test)]
    fn with_executables(gh: PathBuf, git: PathBuf, home: PathBuf) -> Self {
        Self {
            gh_executable: gh,
            git_executable: git,
            home: Some(home),
        }
    }

    fn gh(&self, arguments: &[&str]) -> Result<String, GithubProjectsError> {
        let home = self
            .home
            .as_ref()
            .ok_or(GithubProjectsError::SearchFailed)?;
        let mut command = Command::new(&self.gh_executable);
        command
            .args(arguments)
            .current_dir(home)
            .env("GH_PROMPT_DISABLED", "1")
            .env("GIT_TERMINAL_PROMPT", "0");
        run_bounded_command(command, SEARCH_TIMEOUT, true).map_err(|error| map_search_error(&error))
    }

    fn clone_protocol(&self) -> Result<&'static str, GithubProjectsError> {
        match self.gh(&["config", "get", "git_protocol", "--host", "github.com"]) {
            Ok(value) if value.trim().eq_ignore_ascii_case("ssh") => Ok("ssh"),
            Ok(_) | Err(GithubProjectsError::SearchFailed) => Ok("https"),
            Err(error) => Err(error),
        }
    }
}

impl GithubProjectsRuntime for LocalGithubProjects {
    fn search_repositories(
        &self,
        query: &str,
        limit: usize,
    ) -> Result<Vec<GithubRepository>, GithubProjectsError> {
        if !(1..=50).contains(&limit) {
            return Err(GithubProjectsError::SearchFailed);
        }
        let limit_text = limit.to_string();
        let trimmed = query.trim();
        let (output, listed) = if trimmed.is_empty() {
            (
                self.gh(&[
                    "repo",
                    "list",
                    "--json",
                    "id,name,nameWithOwner,description,isPrivate,updatedAt,sshUrl,url",
                    "--limit",
                    &limit_text,
                ])?,
                true,
            )
        } else {
            (
                self.gh(&[
                    "search",
                    "repos",
                    trimmed,
                    "--json",
                    "id,name,fullName,description,isPrivate,updatedAt,url",
                    "--sort",
                    "updated",
                    "--order",
                    "desc",
                    "--limit",
                    &limit_text,
                ])?,
                false,
            )
        };
        let protocol = self.clone_protocol()?;
        parse_repositories(&output, listed, protocol)
    }

    fn checkout_path(
        &self,
        target_directory: &str,
        name: &str,
    ) -> Result<String, GithubProjectsError> {
        if target_directory.trim().is_empty() || !valid_segment(name) {
            return Err(GithubProjectsError::InvalidTarget);
        }
        expand_target(target_directory)?
            .join(name)
            .to_str()
            .map(str::to_owned)
            .ok_or(GithubProjectsError::InvalidTarget)
    }

    fn clone_repository(
        &self,
        clone_url: &str,
        target_directory: &str,
        name: &str,
    ) -> Result<String, GithubProjectsError> {
        let reported_checkout = self.checkout_path(target_directory, name)?;
        let parent = expand_target(target_directory)?;
        std::fs::create_dir_all(&parent).map_err(|_| GithubProjectsError::InvalidTarget)?;
        let parent = parent
            .canonicalize()
            .map_err(|_| GithubProjectsError::InvalidTarget)?;
        let checkout = parent.join(name);
        if checkout.symlink_metadata().is_ok() {
            return Err(GithubProjectsError::TargetExists);
        }
        let staging = tempfile::Builder::new()
            .prefix(".paseo-clone-")
            .tempdir_in(&parent)
            .map_err(|_| GithubProjectsError::InvalidTarget)?;
        let mut command = Command::new(&self.git_executable);
        command
            .arg("--no-optional-locks")
            .args(["-c", "core.fsmonitor=false", "-c", "color.ui=false"])
            .args(["clone", "--"])
            .arg(clone_url)
            .arg(staging.path())
            .current_dir(&parent);
        for (name, _) in std::env::vars_os() {
            if name.to_string_lossy().starts_with("GIT_") {
                command.env_remove(name);
            }
        }
        command.env("GIT_TERMINAL_PROMPT", "0");
        run_bounded_command(command, CLONE_TIMEOUT, false)
            .map_err(|_| GithubProjectsError::CloneFailed)?;
        std::fs::rename(staging.path(), &checkout).map_err(|error| {
            if error.kind() == std::io::ErrorKind::AlreadyExists {
                GithubProjectsError::TargetExists
            } else {
                GithubProjectsError::CloneFailed
            }
        })?;
        Ok(reported_checkout)
    }
}

fn expand_target(value: &str) -> Result<PathBuf, GithubProjectsError> {
    let value = value.trim();
    let path = if value == "~" {
        home_path()?
    } else if let Some(remainder) = value.strip_prefix("~/") {
        home_path()?.join(remainder)
    } else {
        PathBuf::from(value)
    };
    let path = if path.is_absolute() {
        path
    } else {
        std::env::current_dir()
            .map_err(|_| GithubProjectsError::InvalidTarget)?
            .join(path)
    };
    if path.to_str().is_none() || path.as_os_str().len() > 4096 {
        return Err(GithubProjectsError::InvalidTarget);
    }
    Ok(path)
}

fn home_path() -> Result<PathBuf, GithubProjectsError> {
    std::env::var_os("HOME")
        .map(PathBuf::from)
        .filter(|path| path.is_absolute())
        .ok_or(GithubProjectsError::InvalidTarget)
}

fn parse_repositories(
    output: &str,
    listed: bool,
    protocol: &str,
) -> Result<Vec<GithubRepository>, GithubProjectsError> {
    let rows: Vec<Value> =
        serde_json::from_str(output).map_err(|_| GithubProjectsError::SearchFailed)?;
    rows.iter()
        .map(|row| parse_repository(row, listed, protocol))
        .collect()
}

fn parse_repository(
    row: &Value,
    listed: bool,
    protocol: &str,
) -> Result<GithubRepository, GithubProjectsError> {
    let invalid = GithubProjectsError::SearchFailed;
    let id = row
        .get("id")
        .and_then(|value| match value {
            Value::String(value) => Some(value.clone()),
            Value::Number(value) => Some(value.to_string()),
            Value::Null | Value::Bool(_) | Value::Array(_) | Value::Object(_) => None,
        })
        .ok_or(invalid)?;
    let id = id.trim().to_owned();
    let name = required_text(row, "name")?;
    let owner_path = required_text(row, if listed { "nameWithOwner" } else { "fullName" })?;
    let (owner, repo_name) = owner_path.split_once('/').ok_or(invalid)?;
    if !valid_segment(owner) || !valid_segment(repo_name) || name != repo_name || id.is_empty() {
        return Err(invalid);
    }
    let description = row
        .get("description")
        .and_then(Value::as_str)
        .map(str::to_owned);
    let private = row
        .get("isPrivate")
        .and_then(Value::as_bool)
        .ok_or(invalid)?;
    let updated_at = required_text(row, "updatedAt")?;
    let https_url = required_text(row, "url")?;
    if https_url != format!("https://github.com/{owner_path}")
        && https_url != format!("https://github.com/{owner_path}.git")
    {
        return Err(invalid);
    }
    let clone_url = if protocol == "ssh" {
        if listed {
            let ssh_url = required_text(row, "sshUrl")?;
            if ssh_url != format!("git@github.com:{owner_path}.git") {
                return Err(invalid);
            }
            ssh_url
        } else {
            format!("git@github.com:{owner_path}.git")
        }
    } else {
        https_url
    };
    Ok(GithubRepository {
        id,
        name,
        name_with_owner: owner_path,
        description,
        visibility: if private {
            GithubRepositoryVisibility::Private
        } else {
            GithubRepositoryVisibility::Public
        },
        updated_at,
        clone_url,
    })
}

fn required_text(row: &Value, name: &str) -> Result<String, GithubProjectsError> {
    row.get(name)
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_owned)
        .ok_or(GithubProjectsError::SearchFailed)
}

fn valid_segment(value: &str) -> bool {
    !value.is_empty()
        && value != "."
        && value != ".."
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b'-'))
}

fn map_search_error(error: &crate::ports::forge::ForgeRuntimeError) -> GithubProjectsError {
    match error.kind {
        ForgeFailureKind::CliMissing => GithubProjectsError::CliMissing,
        ForgeFailureKind::Unauthenticated => GithubProjectsError::Unauthenticated,
        ForgeFailureKind::NotGitRepository
        | ForgeFailureKind::NotAllowed
        | ForgeFailureKind::MergeConflict
        | ForgeFailureKind::NoRemote
        | ForgeFailureKind::NotFound
        | ForgeFailureKind::Forbidden
        | ForgeFailureKind::Invalid
        | ForgeFailureKind::Unknown => GithubProjectsError::SearchFailed,
    }
}

#[cfg(test)]
mod tests;
