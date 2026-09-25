use server_metadata::ports::provisioning::{Checkout, DirectorySource, DirectorySourceError};
use server_metadata::ports::registry::{ProjectRegistry, WorkspaceRegistry};
use server_metadata::service::directory::{Directory, DirectoryDependencies};
use server_metadata::storage::{
    project_config::LocalProjectConfigStore,
    project_icon::LocalProjectIconStore,
    registry::{FileBackedProjectRegistry, FileBackedWorkspaceRegistry},
};

use super::*;
use crate::ports::github_projects::{
    GithubProjectsError, GithubProjectsRuntime, GithubRepository, GithubRepositoryVisibility,
};
#[derive(Debug, Default)]
struct Source;
impl DirectorySource for Source {
    fn inspect(&self, path: &str) -> Result<Checkout, DirectorySourceError> {
        if path.contains("missing") {
            return Err(DirectorySourceError::NotFound);
        }
        let is_git = !path.contains("plain");
        Ok(Checkout {
            cwd: path.to_owned(),
            is_git,
            current_branch: is_git.then(|| "main".to_owned()),
            remote_url: is_git.then(|| "git@github.com:Example/Repo.git".to_owned()),
            worktree_root: is_git.then(|| "/tmp/alpha".to_owned()),
            is_paseo_owned_worktree: false,
            main_repo_root: None,
        })
    }

    fn create_child(&self, parent: &str, name: &str) -> Result<String, DirectorySourceError> {
        if name == "exists" {
            Err(DirectorySourceError::AlreadyExists)
        } else {
            Ok(format!("{parent}/{name}"))
        }
    }

    fn remove_empty(&self, _path: &str) -> Result<(), DirectorySourceError> {
        Ok(())
    }

    fn equivalent(&self, left: &str, right: &str) -> bool {
        left == right
    }

    fn canonical(&self, path: &str) -> Result<String, DirectorySourceError> {
        self.inspect(path).map(|checkout| checkout.cwd)
    }
}

#[derive(Debug, Default)]
struct Github;

impl GithubProjectsRuntime for Github {
    fn search_repositories(
        &self,
        _query: &str,
        _limit: usize,
    ) -> Result<Vec<GithubRepository>, GithubProjectsError> {
        Ok(vec![GithubRepository {
            id: "R_1".to_owned(),
            name: "repo".to_owned(),
            name_with_owner: "owner/repo".to_owned(),
            description: None,
            visibility: GithubRepositoryVisibility::Public,
            updated_at: "2026-09-23T00:00:00Z".to_owned(),
            clone_url: "https://github.com/owner/repo".to_owned(),
        }])
    }

    fn checkout_path(
        &self,
        target_directory: &str,
        name: &str,
    ) -> Result<String, GithubProjectsError> {
        Ok(format!("{target_directory}/{name}"))
    }

    fn clone_repository(
        &self,
        _clone_url: &str,
        target_directory: &str,
        name: &str,
    ) -> Result<String, GithubProjectsError> {
        if name == "exists" {
            return Err(GithubProjectsError::TargetExists);
        }
        Ok(format!("{target_directory}/{name}"))
    }
}

fn directory() -> (tempfile::TempDir, GithubProjects) {
    let temp = tempfile::tempdir().unwrap();
    let projects = FileBackedProjectRegistry::new(temp.path().join("projects.json"));
    let workspaces = FileBackedWorkspaceRegistry::new(temp.path().join("workspaces.json"));
    projects.initialize().unwrap();
    workspaces.initialize().unwrap();
    let directory = Directory::new(DirectoryDependencies {
        projects: Box::new(projects),
        workspaces: Box::new(workspaces),
        source: Box::new(Source),
        config_store: Box::new(LocalProjectConfigStore),
        icon_store: Box::new(LocalProjectIconStore::new(temp.path().join("icons"))),
        server_id: "server-test".to_owned(),
    });
    (temp, GithubProjects::new(directory, Box::new(Github)))
}
#[test]
fn github_clone_normalizes_repo_and_registers_project_without_workspace() {
    let (_temp, directory) = directory();
    let outcome = directory.clone_github_project(
        " owner/repo.git ",
        Some(GithubCloneProtocol::Ssh),
        "/tmp/projects",
        "2026-09-23T00:00:00Z",
    );

    assert_eq!(outcome.repo, "owner/repo");
    assert_eq!(outcome.checkout_path.as_deref(), Some("/tmp/projects/repo"));
    assert_eq!(
        outcome
            .project
            .as_ref()
            .map(|project| project.root_path.as_str()),
        Some("/tmp/projects/repo")
    );
    assert!(outcome.error.is_none());
    assert_eq!(
        directory
            .search_github_repositories("repo", 10)
            .unwrap()
            .len(),
        1
    );
}

#[test]
fn github_clone_rejects_unsafe_input_without_launching_or_registering() {
    let (_temp, directory) = directory();
    for repo in [
        "owner/../repo",
        "https://evil.example/owner/repo",
        "owner/repo",
    ] {
        let outcome = directory.clone_github_project(repo, None, "/tmp/projects", "time");
        assert!(outcome.checkout_path.is_none(), "{repo}");
        assert!(outcome.project.is_none(), "{repo}");
        assert!(outcome.error.is_some(), "{repo}");
    }
}

#[test]
fn github_clone_preserves_completed_checkout_on_registration_failure() {
    let (_temp, directory) = directory();

    let outcome = directory.clone_github_project(
        "owner/missing",
        Some(GithubCloneProtocol::Https),
        "/tmp/projects",
        "time",
    );

    assert_eq!(
        outcome.checkout_path.as_deref(),
        Some("/tmp/projects/missing")
    );
    assert!(outcome.project.is_none());
    assert!(outcome.error.is_some());
}

#[test]
fn github_clone_reports_planned_checkout_path_when_clone_fails() {
    let (_temp, directory) = directory();
    let outcome = directory.clone_github_project(
        "owner/exists",
        Some(GithubCloneProtocol::Https),
        "/tmp/projects",
        "time",
    );

    assert_eq!(
        outcome.checkout_path.as_deref(),
        Some("/tmp/projects/exists")
    );
    assert!(outcome.project.is_none());
    assert_eq!(
        outcome.error.as_deref(),
        Some("Checkout path already exists")
    );
}
