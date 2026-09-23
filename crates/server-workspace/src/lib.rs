//! Local Git inspection and independent server ownership leases.

mod checkout;
mod files;
mod forge;
mod git;
mod github_projects;
mod identity;
mod project_config;
mod project_icon;
mod provisioning;
mod workspace_automation;
mod worktrees;

pub use checkout::LocalCheckout;
pub use files::LocalFiles;
pub use forge::LocalForge;
pub use github_projects::LocalGithubProjects;
pub use project_config::LocalProjectConfigStore;
pub use project_icon::LocalProjectIconStore;
pub use provisioning::LocalDirectorySource;
pub use workspace_automation::LocalWorkspaceAutomation;
pub use worktrees::LocalManagedWorktrees;

use std::fs::{File, OpenOptions};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};

use server_domain::{MAX_INSTRUCTION_BYTES, ProjectId};
use server_ports::{IdentityLease, Lease, ProjectError, Workspace, WorkspaceInfo};

/// Local adapter. Every production instance uses the same user-local identity lock directory.
#[derive(Debug)]
pub struct LocalWorkspace {
    identity_locks: PathBuf,
}

impl LocalWorkspace {
    /// Use `HOME/.ait-server-project-locks`, independent of the server data directory.
    ///
    /// # Errors
    /// Rejects missing, relative, or non-directory HOME values.
    pub fn for_user() -> Result<Self, ProjectError> {
        let home = std::env::var_os("HOME")
            .map(PathBuf::from)
            .ok_or(ProjectError::Io)?;
        if !home.is_absolute() || !home.is_dir() {
            return Err(ProjectError::Io);
        }
        Ok(Self::new(home.join(".ait-server-project-locks")))
    }

    /// Select an explicit shared lock directory, primarily for isolated host/test composition.
    /// All instances managing the same projects must use the same directory.
    #[must_use]
    pub fn new(identity_locks: PathBuf) -> Self {
        Self { identity_locks }
    }
}

#[derive(Debug)]
struct FileLease {
    _file: File,
}
impl Lease for FileLease {}

impl Workspace for LocalWorkspace {
    fn inspect(&self, path: &Path) -> Result<WorkspaceInfo, ProjectError> {
        let root = path
            .canonicalize()
            .map_err(|_| ProjectError::UnsupportedWorkspace)?;
        let text = root.to_str().ok_or(ProjectError::Invalid)?;
        if !root.is_dir() || text.len() > 4096 || text.chars().any(char::is_control) {
            return Err(ProjectError::UnsupportedWorkspace);
        }
        for ancestor in root.ancestors() {
            if ancestor
                .file_name()
                .is_some_and(|name| name == ".ait" || name == ".ait-server")
                || ancestor.join(".ait").symlink_metadata().is_ok()
            {
                return Err(ProjectError::LegacyProject);
            }
        }
        // Reject every linked worktree and shared main checkout in this first slice.
        let git_dir = root.join(".git");
        directory(&git_dir, false)?;
        let top = PathBuf::from(git::run(&root, &["rev-parse", "--show-toplevel"])?);
        let common = PathBuf::from(git::run(
            &root,
            &["rev-parse", "--path-format=absolute", "--git-common-dir"],
        )?);
        if top.canonicalize().map_err(|_| ProjectError::Io)? != root
            || common.canonicalize().map_err(|_| ProjectError::Io)? != git_dir
            || git::run(&root, &["worktree", "list", "--porcelain", "-z"])?
                .split('\0')
                .filter(|part| part.starts_with("worktree "))
                .count()
                != 1
            || !git::run(&root, &["ls-files", "--", ".ait-server"])?.is_empty()
        {
            return Err(ProjectError::UnsupportedWorkspace);
        }
        let head = git::run(&root, &["rev-parse", "--verify", "HEAD^{commit}"])?.parse()?;
        let instructions = read_optional(&root.join("AGENTS.md"), MAX_INSTRUCTION_BYTES)?;
        Ok(WorkspaceInfo {
            root,
            head,
            instructions,
        })
    }

    fn acquire_path(&self, info: &WorkspaceInfo) -> Result<Box<dyn Lease>, ProjectError> {
        let current = self.inspect(&info.root)?;
        if current.head != info.head || current.instructions != info.instructions {
            return Err(ProjectError::UnsupportedWorkspace);
        }
        let state = info.root.join(".ait-server");
        directory(&state, true)?;
        let lease = FileLease {
            _file: lock(&state.join("project.lock"))?,
        };
        exclude_runtime(&info.root)?;
        Ok(Box::new(lease))
    }

    fn acquire_identity(&self, id: ProjectId) -> Result<Box<dyn IdentityLease>, ProjectError> {
        directory(&self.identity_locks, true)?;
        let file = lock(&self.identity_locks.join(format!("{id}.lock")))?;
        Ok(Box::new(identity::Identity::new(
            file,
            self.identity_locks.join(format!("{id}.epoch")),
        )))
    }
}

fn directory(path: &Path, create: bool) -> Result<(), ProjectError> {
    if create {
        let mut builder = std::fs::DirBuilder::new();
        #[cfg(unix)]
        {
            use std::os::unix::fs::DirBuilderExt;
            builder.mode(0o700);
        }
        match builder.create(path) {
            Ok(()) =>
            {
                #[cfg(unix)]
                if let Some(parent) = path.parent() {
                    File::open(parent)
                        .and_then(|directory| directory.sync_all())
                        .map_err(|_| ProjectError::Io)?;
                }
            }
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {}
            Err(_) => return Err(ProjectError::Io),
        }
    }
    let metadata = path
        .symlink_metadata()
        .map_err(|_| ProjectError::UnsupportedWorkspace)?;
    if !metadata.is_dir() || metadata.file_type().is_symlink() {
        return Err(ProjectError::UnsupportedWorkspace);
    }
    Ok(())
}

fn regular_file(path: &Path) -> Result<bool, ProjectError> {
    match path.symlink_metadata() {
        Ok(metadata) if metadata.is_file() && !metadata.file_type().is_symlink() => Ok(true),
        Ok(_) => Err(ProjectError::UnsupportedWorkspace),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(false),
        Err(_) => Err(ProjectError::Io),
    }
}

fn lock(path: &Path) -> Result<File, ProjectError> {
    regular_file(path)?;
    let mut options = OpenOptions::new();
    options.read(true).write(true).create(true).truncate(false);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600).custom_flags(libc::O_NOFOLLOW);
    }
    let file = options.open(path).map_err(|_| ProjectError::Io)?;
    file.try_lock().map_err(|error| match error {
        std::fs::TryLockError::WouldBlock => ProjectError::Busy,
        std::fs::TryLockError::Error(_) => ProjectError::Io,
    })?;
    Ok(file)
}

fn read_optional(path: &Path, limit: usize) -> Result<String, ProjectError> {
    if !regular_file(path)? {
        return Ok(String::new());
    }
    let file = File::open(path).map_err(|_| ProjectError::Io)?;
    let mut text = String::new();
    file.take((limit + 1) as u64)
        .read_to_string(&mut text)
        .map_err(|_| ProjectError::Io)?;
    if text.len() > limit {
        return Err(ProjectError::Invalid);
    }
    Ok(text)
}

fn exclude_runtime(root: &Path) -> Result<(), ProjectError> {
    let info = root.join(".git/info");
    directory(&info, true)?;
    let exclude = info.join("exclude");
    let contents = read_optional(&exclude, 64 * 1024)?;
    if !contents.lines().any(|line| line == "/.ait-server/") {
        let addition = b"\n/.ait-server/\n";
        if contents.len() + addition.len() > 64 * 1024 {
            return Err(ProjectError::Invalid);
        }
        let mut file = OpenOptions::new()
            .create(true)
            .append(true)
            .open(&exclude)
            .map_err(|_| ProjectError::Io)?;
        file.write_all(addition).map_err(|_| ProjectError::Io)?;
        file.sync_all().map_err(|_| ProjectError::Io)?;
    }
    // Repository .gitignore rules have higher precedence than info/exclude.
    if git::run(root, &["check-ignore", "--", ".ait-server/"])?.is_empty() {
        return Err(ProjectError::UnsupportedWorkspace);
    }
    Ok(())
}

#[cfg(test)]
mod tests;
