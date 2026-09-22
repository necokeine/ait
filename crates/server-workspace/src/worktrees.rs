use std::fs::{File, OpenOptions};
use std::io::{Read, Seek, SeekFrom};
use std::path::{Component, Path, PathBuf};
use std::process::{Command, ExitStatus, Stdio};
use std::time::{Duration, Instant, SystemTime};

use chrono::{DateTime, Utc};
use server_ports::worktrees::{
    CreatedManagedWorktree, ManagedWorktreeCreate, ManagedWorktreeInfo, ManagedWorktrees,
    OwnedWorktree, WorktreeCreateMode, WorktreeError,
};
use sha2::{Digest, Sha256};

const OUTPUT_LIMIT: u64 = 64 * 1024;
const READ_TIMEOUT: Duration = Duration::from_secs(10);
const WRITE_TIMEOUT: Duration = Duration::from_secs(120);

/// Local Git adapter that owns worktrees below one server data directory.
#[derive(Debug, Clone)]
pub struct LocalManagedWorktrees {
    root: PathBuf,
}

impl LocalManagedWorktrees {
    /// Select the base directory whose `<repository-hash>/<slug>` children are server-owned.
    #[must_use]
    pub fn new(root: PathBuf) -> Self {
        Self { root }
    }

    fn repository(cwd: &str) -> Result<Repository, WorktreeError> {
        let cwd = canonical_directory(cwd)?;
        let checkout_root = git_path(&cwd, &["rev-parse", "--show-toplevel"])?;
        let common_dir = git_path(
            &cwd,
            &["rev-parse", "--path-format=absolute", "--git-common-dir"],
        )?;
        let repo_root = if common_dir.file_name().is_some_and(|name| name == ".git") {
            common_dir
                .parent()
                .map(Path::to_path_buf)
                .ok_or(WorktreeError::NotGitRepository)?
        } else {
            common_dir
        };
        let relative_cwd = cwd
            .strip_prefix(&checkout_root)
            .map(Path::to_path_buf)
            .map_err(|_| {
                WorktreeError::Invalid("Workspace cwd is outside its source worktree".to_owned())
            })?;
        let remote_url = optional_git(&repo_root, &["remote", "get-url", "origin"]);
        Ok(Repository {
            source_cwd: cwd,
            repo_root,
            relative_cwd,
            remote_url,
        })
    }

    fn project_root(&self, repo_root: &Path) -> PathBuf {
        self.root.join(repository_hash(repo_root))
    }

    fn normalize_owned(&self, path: &str) -> Result<OwnedWorktree, WorktreeError> {
        let base = normalized_absolute(&self.root)?;
        let target = normalized_absolute(Path::new(path))?;
        let relative = target
            .strip_prefix(&base)
            .map_err(|_| WorktreeError::NotAllowed)?;
        let mut components = relative.components();
        let Some(Component::Normal(project)) = components.next() else {
            return Err(WorktreeError::NotAllowed);
        };
        let Some(Component::Normal(slug)) = components.next() else {
            return Err(WorktreeError::NotAllowed);
        };
        let worktree_path = base.join(project).join(slug);
        let repo_root = infer_repo_root(&worktree_path).and_then(|path| path_text(&path).ok());
        Ok(OwnedWorktree {
            path: path_text(&worktree_path)?,
            repo_root,
        })
    }
}

impl ManagedWorktrees for LocalManagedWorktrees {
    fn list(&self, cwd: &str) -> Result<Vec<ManagedWorktreeInfo>, WorktreeError> {
        let repository = Self::repository(cwd)?;
        let project_root = normalized_absolute(&self.project_root(&repository.repo_root))?;
        let output = git(
            &repository.repo_root,
            &["worktree", "list", "--porcelain"],
            READ_TIMEOUT,
            &[0],
        )?;
        Ok(parse_worktree_list(&output.stdout)
            .into_iter()
            .filter_map(|entry| {
                let path = normalized_absolute(Path::new(&entry.path)).ok()?;
                path.strip_prefix(&project_root).ok()?;
                Some(ManagedWorktreeInfo {
                    created_at: created_at(&path),
                    path: path_text(&path).ok()?,
                    branch_name: entry.branch_name,
                    head: entry.head,
                })
            })
            .collect::<Vec<_>>())
    }

    fn create(
        &self,
        input: &ManagedWorktreeCreate,
    ) -> Result<CreatedManagedWorktree, WorktreeError> {
        validate_slug(&input.slug)?;
        let repository = Self::repository(&input.cwd)?;
        let project_root = self.project_root(&repository.repo_root);
        create_private_directory(&self.root)?;
        create_private_directory(&project_root)?;

        let plan = create_plan(&repository.repo_root, &input.slug, &input.mode)?;
        let initial_path = project_root.join(&input.slug);
        let target = available_path(&initial_path)?;
        if let Some(parent) = target.parent() {
            create_private_directory(parent)?;
        }
        let target_text = path_text(&target)?;
        let arguments = plan
            .arguments
            .iter()
            .map(String::as_str)
            .collect::<Vec<_>>();
        let mut git_arguments = vec!["worktree", "add", target_text.as_str()];
        git_arguments.extend(arguments);
        git(&repository.repo_root, &git_arguments, WRITE_TIMEOUT, &[0])?;

        let result = (|| {
            let worktree_path = target
                .canonicalize()
                .map_err(|error| WorktreeError::Io(error.to_string()))?;
            let workspace_cwd = worktree_path.join(&repository.relative_cwd);
            if !workspace_cwd.is_dir() {
                return Err(WorktreeError::Invalid(format!(
                    "Selected project directory is missing from the worktree: {}",
                    workspace_cwd.display()
                )));
            }
            seed_config(&repository.source_cwd, &workspace_cwd)?;
            Ok(CreatedManagedWorktree {
                repo_root: path_text(&repository.repo_root)?,
                source_cwd: path_text(&repository.source_cwd)?,
                workspace_cwd: path_text(&workspace_cwd)?,
                worktree_path: path_text(&worktree_path)?,
                branch_name: plan.branch_name,
                comparison_base_ref: plan.comparison_base_ref,
                remote_url: repository.remote_url,
            })
        })();
        if result.is_err() {
            let owned = OwnedWorktree {
                path: target_text,
                repo_root: path_text(&repository.repo_root).ok(),
            };
            let _ = self.remove(&owned);
        }
        result
    }

    fn owned(&self, path: &str) -> Result<OwnedWorktree, WorktreeError> {
        self.normalize_owned(path)
    }

    fn path_for_slug(&self, repo_root: &str, slug: &str) -> Result<String, WorktreeError> {
        validate_slug(slug)?;
        let repository = Self::repository(repo_root)?;
        path_text(&self.project_root(&repository.repo_root).join(slug))
    }

    fn contains(&self, root: &str, candidate: &str) -> bool {
        let Ok(root) = normalized_absolute(Path::new(root)) else {
            return false;
        };
        let Ok(candidate) = normalized_absolute(Path::new(candidate)) else {
            return false;
        };
        candidate == root || candidate.starts_with(root)
    }

    fn remove(&self, worktree: &OwnedWorktree) -> Result<(), WorktreeError> {
        let owned = self.normalize_owned(&worktree.path)?;
        let path = PathBuf::from(&owned.path);
        let repo_root = worktree
            .repo_root
            .as_deref()
            .or(owned.repo_root.as_deref())
            .map(PathBuf::from);
        if let Some(repo_root) = &repo_root {
            let _ = git(
                repo_root,
                &["worktree", "remove", &owned.path, "--force"],
                WRITE_TIMEOUT,
                &[0],
            );
        }
        match std::fs::remove_dir_all(&path) {
            Ok(()) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => return Err(WorktreeError::Io(error.to_string())),
        }
        if path.exists() {
            return Err(WorktreeError::Io(format!(
                "Failed to remove worktree directory: {}",
                path.display()
            )));
        }
        if let Some(repo_root) = repo_root {
            let _ = git(&repo_root, &["worktree", "prune"], READ_TIMEOUT, &[0]);
        }
        Ok(())
    }
}

#[derive(Debug)]
struct Repository {
    source_cwd: PathBuf,
    repo_root: PathBuf,
    relative_cwd: PathBuf,
    remote_url: Option<String>,
}

#[derive(Debug)]
struct CreatePlan {
    branch_name: String,
    comparison_base_ref: Option<String>,
    arguments: Vec<String>,
}

fn create_plan(
    repo_root: &Path,
    slug: &str,
    mode: &WorktreeCreateMode,
) -> Result<CreatePlan, WorktreeError> {
    match mode {
        WorktreeCreateMode::BranchOff {
            base_ref,
            branch_name,
        } => {
            validate_branch(repo_root, branch_name)?;
            let base_name = base_ref
                .as_deref()
                .map(str::trim)
                .filter(|value| !value.is_empty())
                .map(str::to_owned)
                .map_or_else(|| default_branch(repo_root), Ok)?;
            if base_name == "HEAD" {
                return Err(WorktreeError::Invalid(
                    "Base branch cannot be HEAD when creating a Paseo worktree".to_owned(),
                ));
            }
            let resolved_base = resolve_base(repo_root, &base_name)?;
            let branch_exists = local_branch_exists(repo_root, branch_name);
            let base = if branch_exists {
                branch_name.to_owned()
            } else {
                resolved_base.clone()
            };
            let candidate = if branch_exists { slug } else { branch_name };
            let actual_branch = unique_branch(repo_root, candidate);
            Ok(CreatePlan {
                branch_name: actual_branch.clone(),
                comparison_base_ref: Some(resolved_base),
                arguments: vec![
                    "-b".to_owned(),
                    actual_branch,
                    "--no-track".to_owned(),
                    base,
                ],
            })
        }
        WorktreeCreateMode::Checkout { branch_name } => {
            validate_branch(repo_root, branch_name)?;
            ensure_local_branch(repo_root, branch_name)?;
            if branch_checked_out(repo_root, branch_name)? {
                let actual_branch = unique_branch(repo_root, branch_name);
                return Ok(CreatePlan {
                    branch_name: actual_branch.clone(),
                    comparison_base_ref: None,
                    arguments: vec![
                        "-b".to_owned(),
                        actual_branch,
                        "--no-track".to_owned(),
                        branch_name.clone(),
                    ],
                });
            }
            Ok(CreatePlan {
                branch_name: branch_name.clone(),
                comparison_base_ref: None,
                arguments: vec![branch_name.clone()],
            })
        }
    }
}

fn validate_slug(slug: &str) -> Result<(), WorktreeError> {
    if slug.is_empty()
        || slug.len() > 100
        || !slug
            .bytes()
            .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || b"-/".contains(&byte))
        || slug.starts_with('-')
        || slug.ends_with('-')
        || slug.contains("--")
        || slug.split('/').any(str::is_empty)
    {
        return Err(WorktreeError::Invalid("Invalid worktree name".to_owned()));
    }
    Ok(())
}

fn validate_branch(repo_root: &Path, branch: &str) -> Result<(), WorktreeError> {
    if git(
        repo_root,
        &["check-ref-format", "--branch", branch],
        READ_TIMEOUT,
        &[0, 1, 128],
    )?
    .status
    .success()
    {
        Ok(())
    } else {
        Err(WorktreeError::Invalid(format!(
            "Invalid branch name: Git rejected ref name '{branch}'"
        )))
    }
}

fn default_branch(repo_root: &Path) -> Result<String, WorktreeError> {
    if let Some(remote) = optional_git(
        repo_root,
        &[
            "symbolic-ref",
            "--quiet",
            "--short",
            "refs/remotes/origin/HEAD",
        ],
    ) && let Some(branch) = remote.strip_prefix("origin/")
        && !branch.is_empty()
    {
        return Ok(branch.to_owned());
    }
    if let Some(branch) = optional_git(repo_root, &["symbolic-ref", "--quiet", "--short", "HEAD"])
        && !branch.is_empty()
    {
        return Ok(branch);
    }
    for branch in ["main", "master"] {
        if local_branch_exists(repo_root, branch) {
            return Ok(branch.to_owned());
        }
    }
    Err(WorktreeError::Io(
        "Unable to resolve repository default branch".to_owned(),
    ))
}

fn resolve_base(repo_root: &Path, requested: &str) -> Result<String, WorktreeError> {
    let exact = if requested.starts_with("refs/") {
        Some(requested.to_owned())
    } else {
        requested
            .strip_prefix("origin/")
            .map(|_| format!("refs/remotes/{requested}"))
    };
    let candidates = exact.map_or_else(
        || {
            vec![
                format!("refs/heads/{requested}"),
                format!("refs/remotes/origin/{requested}"),
                requested.to_owned(),
            ]
        },
        |exact| vec![exact],
    );
    candidates
        .into_iter()
        .find(|candidate| {
            git(
                repo_root,
                &["rev-parse", "--verify", candidate],
                READ_TIMEOUT,
                &[0],
            )
            .is_ok()
        })
        .ok_or_else(|| WorktreeError::Io(format!("Base branch not found: {requested}")))
}

fn ensure_local_branch(repo_root: &Path, branch: &str) -> Result<(), WorktreeError> {
    if local_branch_exists(repo_root, branch) {
        return Ok(());
    }
    let refspec = format!("{branch}:{branch}");
    git(
        repo_root,
        &["fetch", "origin", &refspec],
        WRITE_TIMEOUT,
        &[0],
    )
    .map(|_| ())
    .map_err(|_| WorktreeError::UnknownBranch(branch.to_owned()))
}

fn local_branch_exists(repo_root: &Path, branch: &str) -> bool {
    let reference = format!("refs/heads/{branch}");
    git(
        repo_root,
        &["show-ref", "--verify", "--quiet", &reference],
        READ_TIMEOUT,
        &[0],
    )
    .is_ok()
}

fn unique_branch(repo_root: &Path, candidate: &str) -> String {
    if !local_branch_exists(repo_root, candidate) {
        return candidate.to_owned();
    }
    (1_u64..=u64::MAX)
        .map(|suffix| format!("{candidate}-{suffix}"))
        .find(|branch| !local_branch_exists(repo_root, branch))
        .expect("unbounded branch suffix iterator")
}

fn branch_checked_out(repo_root: &Path, branch: &str) -> Result<bool, WorktreeError> {
    let output = git(
        repo_root,
        &["worktree", "list", "--porcelain"],
        READ_TIMEOUT,
        &[0],
    )?;
    Ok(parse_worktree_list(&output.stdout)
        .iter()
        .any(|entry| entry.branch_name.as_deref() == Some(branch)))
}

fn seed_config(source_cwd: &Path, target_cwd: &Path) -> Result<(), WorktreeError> {
    let source = source_cwd.join("paseo.json");
    let target = target_cwd.join("paseo.json");
    let metadata = match source.symlink_metadata() {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(WorktreeError::Io(error.to_string())),
    };
    if !metadata.is_file() || metadata.file_type().is_symlink() {
        return Err(WorktreeError::Io(
            "paseo.json is not a regular file".to_owned(),
        ));
    }
    let mut input = File::open(source).map_err(|error| WorktreeError::Io(error.to_string()))?;
    let mut output = match OpenOptions::new().write(true).create_new(true).open(target) {
        Ok(file) => file,
        Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => return Ok(()),
        Err(error) => return Err(WorktreeError::Io(error.to_string())),
    };
    std::io::copy(&mut input, &mut output).map_err(|error| WorktreeError::Io(error.to_string()))?;
    output
        .sync_all()
        .map_err(|error| WorktreeError::Io(error.to_string()))
}

fn create_private_directory(path: &Path) -> Result<(), WorktreeError> {
    let mut builder = std::fs::DirBuilder::new();
    builder.recursive(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::DirBuilderExt;
        builder.mode(0o700);
    }
    builder
        .create(path)
        .map_err(|error| WorktreeError::Io(error.to_string()))?;
    let metadata = path
        .symlink_metadata()
        .map_err(|error| WorktreeError::Io(error.to_string()))?;
    if !metadata.is_dir() || metadata.file_type().is_symlink() {
        return Err(WorktreeError::Io(
            "managed worktree root is not a directory".to_owned(),
        ));
    }
    Ok(())
}

fn available_path(path: &Path) -> Result<PathBuf, WorktreeError> {
    if !path
        .try_exists()
        .map_err(|error| WorktreeError::Io(error.to_string()))?
    {
        return Ok(path.to_path_buf());
    }
    (1_u64..=u64::MAX)
        .map(|suffix| PathBuf::from(format!("{}-{suffix}", path.display())))
        .find(|candidate| !candidate.exists())
        .ok_or_else(|| WorktreeError::Io("unable to allocate worktree path".to_owned()))
}

fn infer_repo_root(worktree: &Path) -> Option<PathBuf> {
    let common = git_path(
        worktree,
        &["rev-parse", "--path-format=absolute", "--git-common-dir"],
    )
    .ok()?;
    if common.file_name().is_some_and(|name| name == ".git") {
        common.parent().map(Path::to_path_buf)
    } else {
        Some(common)
    }
}

fn repository_hash(repo_root: &Path) -> String {
    let digest = Sha256::digest(repo_root.to_string_lossy().as_bytes());
    let mut first = [0_u8; 8];
    first.copy_from_slice(&digest[..8]);
    let mut value = u64::from_be_bytes(first);
    let mut encoded = Vec::new();
    while value > 0 {
        let digit = u8::try_from(value % 36).expect("base36 digit");
        encoded.push(if digit < 10 {
            b'0' + digit
        } else {
            b'a' + digit - 10
        });
        value /= 36;
    }
    while encoded.len() < 13 {
        encoded.push(b'0');
    }
    encoded.reverse();
    String::from_utf8(encoded)
        .expect("base36 is ASCII")
        .chars()
        .take(8)
        .collect()
}

#[derive(Debug)]
struct ParsedWorktree {
    path: String,
    branch_name: Option<String>,
    head: Option<String>,
}

fn parse_worktree_list(output: &str) -> Vec<ParsedWorktree> {
    let mut entries = Vec::new();
    let mut current: Option<ParsedWorktree> = None;
    for line in output.lines().chain(std::iter::once("")) {
        if let Some(path) = line.strip_prefix("worktree ") {
            if let Some(entry) = current.take() {
                entries.push(entry);
            }
            current = Some(ParsedWorktree {
                path: path.trim().to_owned(),
                branch_name: None,
                head: None,
            });
        } else if let Some(entry) = current.as_mut() {
            if let Some(branch) = line.strip_prefix("branch ") {
                entry.branch_name = Some(
                    branch
                        .trim()
                        .strip_prefix("refs/heads/")
                        .unwrap_or(branch.trim())
                        .to_owned(),
                );
            } else if let Some(head) = line.strip_prefix("HEAD ") {
                entry.head = Some(head.trim().to_owned());
            } else if line.is_empty()
                && let Some(entry) = current.take()
            {
                entries.push(entry);
            }
        }
    }
    entries
}

fn created_at(path: &Path) -> String {
    let time = path
        .metadata()
        .ok()
        .and_then(|metadata| metadata.created().or_else(|_| metadata.modified()).ok())
        .unwrap_or(SystemTime::UNIX_EPOCH);
    DateTime::<Utc>::from(time).to_rfc3339_opts(chrono::SecondsFormat::Millis, true)
}

fn canonical_directory(path: &str) -> Result<PathBuf, WorktreeError> {
    let path = Path::new(path)
        .canonicalize()
        .map_err(|error| match error.kind() {
            std::io::ErrorKind::NotFound | std::io::ErrorKind::NotADirectory => {
                WorktreeError::NotGitRepository
            }
            _ => WorktreeError::Io(error.to_string()),
        })?;
    if path.is_dir() {
        Ok(path)
    } else {
        Err(WorktreeError::NotGitRepository)
    }
}

fn git_path(cwd: &Path, arguments: &[&str]) -> Result<PathBuf, WorktreeError> {
    let output = git(cwd, arguments, READ_TIMEOUT, &[0])?;
    let path = PathBuf::from(output.stdout.trim());
    let absolute = if path.is_absolute() {
        path
    } else {
        cwd.join(path)
    };
    normalized_absolute(&absolute)
}

fn optional_git(cwd: &Path, arguments: &[&str]) -> Option<String> {
    git(cwd, arguments, READ_TIMEOUT, &[0])
        .ok()
        .map(|output| output.stdout.trim().to_owned())
        .filter(|value| !value.is_empty())
}

struct GitOutput {
    status: ExitStatus,
    stdout: String,
}

fn git(
    cwd: &Path,
    arguments: &[&str],
    timeout: Duration,
    accepted: &[i32],
) -> Result<GitOutput, WorktreeError> {
    let mut stdout = tempfile::tempfile().map_err(|error| WorktreeError::Io(error.to_string()))?;
    let mut stderr = tempfile::tempfile().map_err(|error| WorktreeError::Io(error.to_string()))?;
    let mut command = Command::new("git");
    command
        .arg("--no-optional-locks")
        .args(["-c", "core.fsmonitor=false"])
        .args(arguments)
        .current_dir(cwd)
        .stdin(Stdio::null())
        .stdout(
            stdout
                .try_clone()
                .map_err(|error| WorktreeError::Io(error.to_string()))?,
        )
        .stderr(
            stderr
                .try_clone()
                .map_err(|error| WorktreeError::Io(error.to_string()))?,
        )
        .env("GIT_TERMINAL_PROMPT", "0")
        .env("GIT_SSH_COMMAND", "ssh -oBatchMode=yes");
    for (name, _) in std::env::vars_os() {
        if name.to_string_lossy().starts_with("GIT_") {
            command.env_remove(name);
        }
    }
    command
        .env("GIT_TERMINAL_PROMPT", "0")
        .env("GIT_SSH_COMMAND", "ssh -oBatchMode=yes");
    let mut child = command
        .spawn()
        .map_err(|error| WorktreeError::Io(error.to_string()))?;
    let deadline = Instant::now() + timeout;
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break status,
            Ok(None)
                if Instant::now() < deadline
                    && stdout
                        .metadata()
                        .is_ok_and(|metadata| metadata.len() <= OUTPUT_LIMIT)
                    && stderr
                        .metadata()
                        .is_ok_and(|metadata| metadata.len() <= OUTPUT_LIMIT) =>
            {
                std::thread::sleep(Duration::from_millis(10));
            }
            Ok(None) | Err(_) => {
                let _ = child.kill();
                let _ = child.wait();
                return Err(WorktreeError::Io("Git command timed out".to_owned()));
            }
        }
    };
    let stdout_text = read_output(&mut stdout)?;
    let stderr_text = read_output(&mut stderr)?;
    if !accepted.contains(&status.code().unwrap_or(-1)) {
        if arguments.first() == Some(&"rev-parse") {
            return Err(WorktreeError::NotGitRepository);
        }
        return Err(WorktreeError::Io(if stderr_text.trim().is_empty() {
            "Git command failed".to_owned()
        } else {
            stderr_text.trim().to_owned()
        }));
    }
    Ok(GitOutput {
        status,
        stdout: stdout_text,
    })
}

fn read_output(file: &mut File) -> Result<String, WorktreeError> {
    file.seek(SeekFrom::Start(0))
        .map_err(|error| WorktreeError::Io(error.to_string()))?;
    let mut text = String::new();
    file.take(OUTPUT_LIMIT + 1)
        .read_to_string(&mut text)
        .map_err(|error| WorktreeError::Io(error.to_string()))?;
    if text.len() as u64 > OUTPUT_LIMIT {
        return Err(WorktreeError::Io(
            "Git command output exceeded limit".to_owned(),
        ));
    }
    Ok(text)
}

fn normalized_absolute(path: &Path) -> Result<PathBuf, WorktreeError> {
    if let Ok(canonical) = path.canonicalize() {
        return Ok(canonical);
    }
    let absolute = if path.is_absolute() {
        path.to_path_buf()
    } else {
        std::env::current_dir()
            .map_err(|error| WorktreeError::Io(error.to_string()))?
            .join(path)
    };
    let mut normalized = PathBuf::new();
    for component in absolute.components() {
        match component {
            Component::CurDir => {}
            Component::ParentDir => {
                normalized.pop();
            }
            Component::Prefix(_) | Component::RootDir | Component::Normal(_) => {
                normalized.push(component.as_os_str());
            }
        }
    }
    Ok(normalized)
}

fn path_text(path: &Path) -> Result<String, WorktreeError> {
    path.to_str()
        .map(str::to_owned)
        .ok_or_else(|| WorktreeError::Invalid("worktree path is not UTF-8".to_owned()))
}

#[cfg(test)]
mod tests;
