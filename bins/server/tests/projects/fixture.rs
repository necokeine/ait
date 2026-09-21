use std::path::{Path, PathBuf};
use std::process::Command;

use server_application::Projects;
use server_storage::{SqliteCatalog, SqliteProjects};
use server_workspace::LocalWorkspace;

pub(super) struct Fixture {
    pub directory: tempfile::TempDir,
    pub repo: PathBuf,
}

impl Fixture {
    pub fn new() -> Self {
        let directory = tempfile::tempdir().unwrap();
        let repo = repository(directory.path(), "repo");
        std::fs::write(repo.join("AGENTS.md"), "original project instructions").unwrap();
        Self { directory, repo }
    }

    pub fn repository(&self, name: &str) -> PathBuf {
        repository(self.directory.path(), name)
    }

    pub fn state(&self, name: &str) -> PathBuf {
        let path = self.directory.path().join(name);
        std::fs::create_dir_all(&path).unwrap();
        path
    }

    pub fn workspace(&self) -> LocalWorkspace {
        LocalWorkspace::new(self.directory.path().join("identity-locks"))
    }

    pub fn application(&self, name: &str) -> Projects {
        Projects::new(
            Box::new(SqliteCatalog::open(&self.state(name)).unwrap()),
            Box::new(SqliteProjects),
            Box::new(self.workspace()),
        )
    }
}

fn repository(parent: &Path, name: &str) -> PathBuf {
    let root = parent.join(name);
    std::fs::create_dir(&root).unwrap();
    git(&root, &["init", "--quiet"]);
    git(
        &root,
        &["commit", "--quiet", "--allow-empty", "-m", "baseline"],
    );
    root
}

pub(super) fn git(root: &Path, args: &[&str]) {
    let output = Command::new("git")
        .args([
            "-c",
            "user.name=Server Test",
            "-c",
            "user.email=server@example.invalid",
        ])
        .args(args)
        .current_dir(root)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
}
