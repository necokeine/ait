//! Bounded local Git reads for checkout status, diff, and commit history.

use std::collections::{BTreeMap, BTreeSet};
use std::fs::File;
use std::io::{Read, Seek, SeekFrom};
use std::path::{Component, Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use crate::ports::checkout::{
    AheadBehind, CheckoutBranchResolution, CheckoutBranchSource, CheckoutBranchSuggestion,
    CheckoutCommit, CheckoutCommitFile, CheckoutCommitFileStatus, CheckoutCommits, CheckoutDiff,
    CheckoutDiffCompare, CheckoutDiffMode, CheckoutFailureKind, CheckoutMergeStrategy,
    CheckoutRuntime, CheckoutRuntimeError, CheckoutStashEntry, CheckoutStatus, DiffHunk, DiffLine,
    DiffLineKind, ParsedDiffFile, ParsedDiffStatus,
};

const READ_TIMEOUT: Duration = Duration::from_secs(10);
const WRITE_TIMEOUT: Duration = Duration::from_secs(120);
const SMALL_OUTPUT_LIMIT: u64 = 256 * 1024;
const DIFF_OUTPUT_LIMIT: u64 = 4 * 1024 * 1024;
const COMMIT_OUTPUT_LIMIT: u64 = 4 * 1024 * 1024;
const BASE_COMMIT_LIMIT: usize = 10;
const COMMIT_FIELD_SEPARATOR: char = '\0';
const COMMIT_RECORD_SEPARATOR: char = '\x1e';

/// Stateless bounded Git adapter. Managed ownership is rooted below one server data directory.
#[derive(Debug, Clone)]
pub struct LocalCheckout {
    managed_worktrees_root: PathBuf,
}

impl LocalCheckout {
    /// Configure the server-owned worktree root.
    #[must_use]
    pub fn new(managed_worktrees_root: PathBuf) -> Self {
        Self {
            managed_worktrees_root,
        }
    }

    fn inspect(&self, cwd: &str) -> Result<CheckoutStatus, CheckoutRuntimeError> {
        let cwd = expanded_directory(cwd)?;
        let repo_root = match git_optional(&cwd, &["rev-parse", "--show-toplevel"])? {
            Some(root) => canonical_git_path(&root)?,
            None => return Ok(non_git_status()),
        };
        let common_dir = git_required(
            &cwd,
            &["rev-parse", "--path-format=absolute", "--git-common-dir"],
            SMALL_OUTPUT_LIMIT,
        )?;
        let common_dir = canonical_git_path(&common_dir)?;
        let main_repo_root = common_dir
            .file_name()
            .is_some_and(|name| name == ".git")
            .then(|| common_dir.parent().map(Path::to_path_buf))
            .flatten();
        let managed = is_below(&self.managed_worktrees_root, &repo_root)
            && main_repo_root
                .as_ref()
                .is_some_and(|main| main != &repo_root);
        let current_branch = git_optional(&cwd, &["symbolic-ref", "--quiet", "--short", "HEAD"])?;
        let dirty = !git_required(
            &cwd,
            &["status", "--porcelain=v1", "--untracked-files=normal"],
            SMALL_OUTPUT_LIMIT,
        )?
        .is_empty();
        let remotes = lines(&git_required(&cwd, &["remote"], SMALL_OUTPUT_LIMIT)?);
        let preferred_remote = remotes
            .iter()
            .find(|remote| remote.as_str() == "origin")
            .or_else(|| remotes.first());
        let remote_url = preferred_remote
            .map(|remote| git_optional(&cwd, &["remote", "get-url", remote]))
            .transpose()?
            .flatten();
        let base_ref = resolve_default_branch(&cwd, current_branch.as_deref())?;
        let ahead_behind = match (&base_ref, &current_branch) {
            (Some(base), Some(_)) => compare_refs(&cwd, base, "HEAD")?,
            _ => None,
        };
        let upstream_ref =
            git_optional(&cwd, &["rev-parse", "--symbolic-full-name", "@{upstream}"])?;
        let upstream_counts = match &upstream_ref {
            Some(upstream) => compare_refs(&cwd, upstream, "HEAD")?,
            None => None,
        };
        let linked_main = main_repo_root
            .filter(|main| main != &repo_root)
            .map(|path| path_text(&path))
            .transpose()?;
        let repo_root = path_text(&repo_root)?;
        Ok(CheckoutStatus {
            is_git: true,
            repo_root: Some(repo_root.clone()),
            main_repo_root: if managed {
                Some(linked_main.clone().unwrap_or(repo_root))
            } else {
                linked_main
            },
            current_branch,
            is_dirty: Some(dirty),
            base_ref,
            ahead_behind,
            upstream_ref,
            ahead_of_origin: upstream_counts.map(|counts| counts.ahead),
            behind_of_origin: upstream_counts.map(|counts| counts.behind),
            has_remote: !remotes.is_empty(),
            remote_url,
            is_managed_worktree: managed,
        })
    }
}

impl CheckoutRuntime for LocalCheckout {
    fn status(&self, cwd: &str) -> Result<CheckoutStatus, CheckoutRuntimeError> {
        self.inspect(cwd)
    }

    fn refresh(&self, cwd: &str) -> Result<(), CheckoutRuntimeError> {
        self.inspect(cwd).map(|_| ())
    }

    fn diff(
        &self,
        cwd: &str,
        compare: &CheckoutDiffCompare,
    ) -> Result<CheckoutDiff, CheckoutRuntimeError> {
        let cwd = require_git_directory(cwd)?;
        let mut arguments = vec![
            "diff".to_owned(),
            "--no-ext-diff".to_owned(),
            "--no-color".to_owned(),
            "--find-renames".to_owned(),
        ];
        if compare.ignore_whitespace {
            arguments.push("--ignore-all-space".to_owned());
        }
        let include_untracked = match compare.mode {
            CheckoutDiffMode::Uncommitted => {
                arguments.push(diff_head(&cwd)?);
                true
            }
            CheckoutDiffMode::Base => {
                let base = compare
                    .base_ref
                    .as_deref()
                    .map(str::trim)
                    .filter(|base| !base.is_empty())
                    .map(str::to_owned)
                    .map_or_else(|| resolve_default_branch(&cwd, None), |base| Ok(Some(base)))?
                    .ok_or_else(|| {
                        checkout_error(
                            CheckoutFailureKind::Unknown,
                            "Unable to resolve comparison base",
                        )
                    })?;
                validate_ref(&base)?;
                verify_commit(&cwd, &base)?;
                let merge_base =
                    git_required(&cwd, &["merge-base", &base, "HEAD"], SMALL_OUTPUT_LIMIT)?;
                arguments.push(merge_base);
                arguments.push("HEAD".to_owned());
                false
            }
        };
        let argument_refs = arguments.iter().map(String::as_str).collect::<Vec<_>>();
        let tracked = match run_git(&cwd, &argument_refs, &[0], DIFF_OUTPUT_LIMIT) {
            Ok(output) => output.stdout,
            Err(error) if error.message == "Git output exceeded the configured limit" => {
                return Ok(CheckoutDiff {
                    files: Vec::new(),
                    diff_too_large: true,
                });
            }
            Err(error) => return Err(error),
        };
        let mut text = tracked;
        if include_untracked {
            let untracked = run_git(
                &cwd,
                &["ls-files", "--others", "--exclude-standard", "-z"],
                &[0],
                SMALL_OUTPUT_LIMIT,
            )?
            .stdout;
            for path in untracked.split('\0').filter(|path| !path.is_empty()) {
                validate_relative_path(path)?;
                let mut untracked_arguments =
                    vec!["diff", "--no-ext-diff", "--no-color", "--no-index"];
                if compare.ignore_whitespace {
                    untracked_arguments.push("--ignore-all-space");
                }
                untracked_arguments.extend(["--", "/dev/null", path]);
                let output = match run_git(&cwd, &untracked_arguments, &[0, 1], DIFF_OUTPUT_LIMIT) {
                    Ok(output) => output.stdout,
                    Err(error) if error.message == "Git output exceeded the configured limit" => {
                        return Ok(CheckoutDiff {
                            files: Vec::new(),
                            diff_too_large: true,
                        });
                    }
                    Err(error) => return Err(error),
                };
                let combined = text.len().saturating_add(output.len());
                if u64::try_from(combined).unwrap_or(u64::MAX) > DIFF_OUTPUT_LIMIT {
                    return Ok(CheckoutDiff {
                        files: Vec::new(),
                        diff_too_large: true,
                    });
                }
                text.push_str(&output);
            }
        }
        let mut files = parse_diff(&text);
        files.sort_by(|left, right| left.path.cmp(&right.path));
        Ok(CheckoutDiff {
            files,
            diff_too_large: false,
        })
    }

    fn commits(&self, cwd: &str) -> Result<CheckoutCommits, CheckoutRuntimeError> {
        let cwd = require_git_directory(cwd)?;
        let Some(current_branch) =
            git_optional(&cwd, &["symbolic-ref", "--quiet", "--short", "HEAD"])?
        else {
            return Ok(CheckoutCommits {
                base_ref: None,
                commits: Vec::new(),
            });
        };
        let default_base = resolve_default_branch(&cwd, Some(&current_branch))?;
        let comparison_base = default_base.filter(|base| base != &current_branch);
        let (workspace_records, base_revision) = if let Some(base) = &comparison_base {
            verify_commit(&cwd, base)?;
            let merge_base = git_optional(&cwd, &["merge-base", base, "HEAD"])?;
            (
                commit_records(&cwd, &format!("{base}..HEAD"), None)?,
                merge_base,
            )
        } else {
            (Vec::new(), Some("HEAD".to_owned()))
        };
        let base_records = match base_revision {
            Some(revision) => commit_records(&cwd, &revision, Some(BASE_COMMIT_LIMIT))?,
            None => Vec::new(),
        };
        let workspace_shas = workspace_records
            .iter()
            .map(|record| record.sha.clone())
            .collect::<BTreeSet<_>>();
        let unpushed = git_required(
            &cwd,
            &["rev-list", "HEAD", "--not", "--remotes"],
            COMMIT_OUTPUT_LIMIT,
        )?
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .map(str::to_owned)
        .collect::<BTreeSet<_>>();
        let commits = workspace_records
            .into_iter()
            .chain(base_records)
            .map(|record| CheckoutCommit {
                is_on_remote: !unpushed.contains(&record.sha),
                is_on_base: !workspace_shas.contains(&record.sha),
                sha: record.sha,
                short_sha: record.short_sha,
                subject: record.subject,
                author_name: record.author_name,
                author_date: record.author_date,
                files: record.files,
            })
            .collect();
        Ok(CheckoutCommits {
            base_ref: comparison_base,
            commits,
        })
    }

    fn commit_file_diff(
        &self,
        cwd: &str,
        sha: &str,
        path: &str,
    ) -> Result<Option<ParsedDiffFile>, CheckoutRuntimeError> {
        let cwd = require_git_directory(cwd)?;
        validate_commit(sha)?;
        validate_relative_path(path)?;
        verify_commit(&cwd, sha)?;
        let output = run_git(
            &cwd,
            &[
                "show",
                sha,
                "--format=",
                "--diff-merges=first-parent",
                "--",
                path,
            ],
            &[0],
            DIFF_OUTPUT_LIMIT,
        )?
        .stdout;
        if output.trim().is_empty() || output.contains("Binary files") {
            return Ok(None);
        }
        Ok(parse_diff(&output)
            .into_iter()
            .find(|file| file.path == path && !file.hunks.is_empty()))
    }

    fn validate_branch(
        &self,
        cwd: &str,
        branch: &str,
    ) -> Result<CheckoutBranchResolution, CheckoutRuntimeError> {
        let cwd = require_git_directory(cwd)?;
        resolve_branch(&cwd, branch)
    }

    fn branch_suggestions(
        &self,
        cwd: &str,
        query: Option<&str>,
        limit: usize,
    ) -> Result<Vec<CheckoutBranchSuggestion>, CheckoutRuntimeError> {
        let cwd = require_git_directory(cwd)?;
        list_branch_suggestions(&cwd, query, limit)
    }

    fn switch_branch(
        &self,
        cwd: &str,
        branch: &str,
    ) -> Result<CheckoutBranchSource, CheckoutRuntimeError> {
        let cwd = require_git_directory(cwd)?;
        require_clean(&cwd)?;
        match resolve_branch(&cwd, branch)? {
            CheckoutBranchResolution::Local(name) => {
                let current = git_optional(&cwd, &["symbolic-ref", "--quiet", "--short", "HEAD"])?;
                if current.as_deref() != Some(name.as_str()) {
                    git_write(&cwd, &["checkout", &name])?;
                }
                Ok(CheckoutBranchSource::Local)
            }
            CheckoutBranchResolution::RemoteOnly { name, remote_ref } => {
                git_write(&cwd, &["checkout", "-b", &name, "--track", &remote_ref])?;
                Ok(CheckoutBranchSource::Remote)
            }
            CheckoutBranchResolution::NotFound => Err(checkout_error(
                CheckoutFailureKind::Unknown,
                format!("Branch not found: {}", branch.trim()),
            )),
        }
    }

    fn rename_branch(&self, cwd: &str, branch: &str) -> Result<String, CheckoutRuntimeError> {
        let cwd = require_git_directory(cwd)?;
        let branch = validate_branch_slug(branch)?;
        let current = git_optional(&cwd, &["symbolic-ref", "--quiet", "--short", "HEAD"])?;
        if current.is_none() {
            return Err(checkout_error(
                CheckoutFailureKind::Unknown,
                "Cannot rename branch in detached HEAD state",
            ));
        }
        git_write(&cwd, &["branch", "-m", &branch])?;
        git_required(
            &cwd,
            &["symbolic-ref", "--quiet", "--short", "HEAD"],
            SMALL_OUTPUT_LIMIT,
        )
    }

    fn commit(&self, cwd: &str, message: &str, add_all: bool) -> Result<(), CheckoutRuntimeError> {
        let cwd = require_git_directory(cwd)?;
        let message = message.trim();
        if message.is_empty() {
            return Err(checkout_error(
                CheckoutFailureKind::Unknown,
                "Commit message is required",
            ));
        }
        if add_all {
            git_write(&cwd, &["add", "-A"])?;
        }
        git_write(&cwd, &["commit", "-m", message]).map(|_| ())
    }

    fn merge_to_base(
        &self,
        cwd: &str,
        base_ref: Option<&str>,
        strategy: CheckoutMergeStrategy,
        require_clean_target: bool,
    ) -> Result<(), CheckoutRuntimeError> {
        let cwd = require_git_directory(cwd)?;
        if require_clean_target {
            require_clean(&cwd)?;
        }
        let current = current_branch(&cwd, "merge")?;
        let base = operation_base(&cwd, base_ref, Some(&current))?;
        let base = local_base_name(&base)?;
        if base == current {
            return Ok(());
        }
        verify_local_branch(&cwd, &base)?;
        let operation_cwd = worktree_for_branch(&cwd, &base)?.unwrap_or_else(|| cwd.clone());
        let same_checkout = operation_cwd == cwd;
        let original = git_optional(
            &operation_cwd,
            &["symbolic-ref", "--quiet", "--short", "HEAD"],
        )?;
        let outcome = (|| {
            git_write(&operation_cwd, &["checkout", &base])?;
            match strategy {
                CheckoutMergeStrategy::Merge => {
                    git_write(&operation_cwd, &["merge", &current]).map(|_| ())
                }
                CheckoutMergeStrategy::Squash => {
                    git_write(&operation_cwd, &["merge", "--squash", &current])?;
                    let message = format!("Squash merge {current} into {base}");
                    git_write(&operation_cwd, &["commit", "-m", &message]).map(|_| ())
                }
            }
        })();
        let outcome = abort_merge_on_conflict(&operation_cwd, outcome);
        if same_checkout
            && original.as_deref().is_some_and(|name| name != base)
            && let Some(original) = original
        {
            let _ = git_write(&operation_cwd, &["checkout", &original]);
        }
        outcome
    }

    fn merge_from_base(
        &self,
        cwd: &str,
        base_ref: Option<&str>,
        require_clean_target: bool,
    ) -> Result<(), CheckoutRuntimeError> {
        let cwd = require_git_directory(cwd)?;
        if require_clean_target {
            require_clean(&cwd)?;
        }
        let current = current_branch(&cwd, "merge")?;
        let base = operation_base(&cwd, base_ref, Some(&current))?;
        let base = most_ahead_base(&cwd, &base)?;
        if base == current {
            return Ok(());
        }
        let outcome = git_write(&cwd, &["merge", &base]).map(|_| ());
        abort_merge_on_conflict(&cwd, outcome)
    }

    fn pull(&self, cwd: &str) -> Result<(), CheckoutRuntimeError> {
        let cwd = require_git_directory(cwd)?;
        current_branch(&cwd, "pull")?;
        require_origin(&cwd)?;
        let outcome = git_write(&cwd, &["pull"]).map(|_| ());
        if outcome.is_err() {
            abort_pull_state(&cwd);
        }
        outcome
    }

    fn push(&self, cwd: &str) -> Result<(), CheckoutRuntimeError> {
        let cwd = require_git_directory(cwd)?;
        let current = current_branch(&cwd, "push")?;
        if let Some((remote, head)) = configured_push_target(&cwd, &current)? {
            return git_write(&cwd, &["push", &remote, &format!("HEAD:refs/heads/{head}")])
                .map(|_| ());
        }
        if let Some(upstream) = git_optional(
            &cwd,
            &[
                "rev-parse",
                "--abbrev-ref",
                "--symbolic-full-name",
                "@{upstream}",
            ],
        )? {
            let (remote, head) = upstream.split_once('/').ok_or_else(|| {
                checkout_error(CheckoutFailureKind::Unknown, "Invalid upstream branch")
            })?;
            return git_write(
                &cwd,
                &["push", "-u", remote, &format!("HEAD:refs/heads/{head}")],
            )
            .map(|_| ());
        }
        require_origin(&cwd)?;
        git_write(&cwd, &["push", "-u", "origin", &current]).map(|_| ())
    }

    fn discard_changes(&self, cwd: &str, paths: &[String]) -> Result<(), CheckoutRuntimeError> {
        let cwd = require_git_directory(cwd)?;
        if paths.is_empty() {
            return Err(checkout_error(
                CheckoutFailureKind::NotAllowed,
                "At least one checkout path is required",
            ));
        }
        for path in paths {
            validate_relative_path(path)?;
        }
        let refs = paths.iter().map(String::as_str).collect::<Vec<_>>();
        discard_paths(&cwd, &refs)
    }

    fn stash_save(&self, cwd: &str, branch: Option<&str>) -> Result<(), CheckoutRuntimeError> {
        let cwd = require_git_directory(cwd)?;
        let branch = branch.map(str::trim).filter(|branch| !branch.is_empty());
        let message = format!("paseo-auto-stash: {}", branch.unwrap_or("unnamed"));
        git_write(
            &cwd,
            &["stash", "push", "--include-untracked", "-m", &message],
        )
        .map(|_| ())
    }

    fn stash_pop(&self, cwd: &str, index: usize) -> Result<(), CheckoutRuntimeError> {
        let cwd = require_git_directory(cwd)?;
        git_write(&cwd, &["stash", "pop", &format!("stash@{{{index}}}")]).map(|_| ())
    }

    fn stashes(
        &self,
        cwd: &str,
        paseo_only: bool,
    ) -> Result<Vec<CheckoutStashEntry>, CheckoutRuntimeError> {
        let cwd = require_git_directory(cwd)?;
        let output = git_required(
            &cwd,
            &["stash", "list", "--format=%gd%x00%s"],
            COMMIT_OUTPUT_LIMIT,
        )?;
        Ok(parse_stashes(&output, paseo_only))
    }
}

#[derive(Debug)]
struct CommandOutput {
    stdout: String,
    exit_code: i32,
}

#[derive(Debug)]
struct CommitRecord {
    sha: String,
    short_sha: String,
    author_name: String,
    author_date: String,
    subject: String,
    files: Vec<CheckoutCommitFile>,
}

fn non_git_status() -> CheckoutStatus {
    CheckoutStatus {
        is_git: false,
        repo_root: None,
        main_repo_root: None,
        current_branch: None,
        is_dirty: None,
        base_ref: None,
        ahead_behind: None,
        upstream_ref: None,
        ahead_of_origin: None,
        behind_of_origin: None,
        has_remote: false,
        remote_url: None,
        is_managed_worktree: false,
    }
}

fn expanded_directory(value: &str) -> Result<PathBuf, CheckoutRuntimeError> {
    let path = if value == "~" {
        std::env::var_os("HOME").map(PathBuf::from).ok_or_else(|| {
            checkout_error(
                CheckoutFailureKind::NotAllowed,
                "Home directory is unavailable",
            )
        })?
    } else if let Some(relative) = value.strip_prefix("~/") {
        std::env::var_os("HOME")
            .map(PathBuf::from)
            .ok_or_else(|| {
                checkout_error(
                    CheckoutFailureKind::NotAllowed,
                    "Home directory is unavailable",
                )
            })?
            .join(relative)
    } else {
        PathBuf::from(value)
    };
    path.canonicalize()
        .map_err(|error| checkout_error(CheckoutFailureKind::NotAllowed, error.to_string()))
        .and_then(|path| {
            path.is_dir().then_some(path).ok_or_else(|| {
                checkout_error(
                    CheckoutFailureKind::NotAllowed,
                    "Checkout cwd is not a directory",
                )
            })
        })
}

fn require_git_directory(value: &str) -> Result<PathBuf, CheckoutRuntimeError> {
    let cwd = expanded_directory(value)?;
    if git_optional(&cwd, &["rev-parse", "--show-toplevel"])?.is_none() {
        return Err(checkout_error(
            CheckoutFailureKind::NotGitRepository,
            "Not a git repository",
        ));
    }
    Ok(cwd)
}

fn canonical_git_path(value: &str) -> Result<PathBuf, CheckoutRuntimeError> {
    PathBuf::from(value.trim()).canonicalize().map_err(|error| {
        checkout_error(
            CheckoutFailureKind::Unknown,
            format!("Resolve Git path: {error}"),
        )
    })
}

fn is_below(root: &Path, candidate: &Path) -> bool {
    let Ok(root) = root.canonicalize() else {
        return false;
    };
    let Ok(candidate) = candidate.canonicalize() else {
        return false;
    };
    let Ok(relative) = candidate.strip_prefix(root) else {
        return false;
    };
    let mut components = relative.components();
    matches!(components.next(), Some(Component::Normal(_)))
        && matches!(components.next(), Some(Component::Normal(_)))
        && components.next().is_none()
}

fn path_text(path: &Path) -> Result<String, CheckoutRuntimeError> {
    path.to_str().map(str::to_owned).ok_or_else(|| {
        checkout_error(
            CheckoutFailureKind::NotAllowed,
            "Checkout path is not valid UTF-8",
        )
    })
}

fn lines(value: &str) -> Vec<String> {
    value
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .map(str::to_owned)
        .collect()
}

fn git_optional(cwd: &Path, arguments: &[&str]) -> Result<Option<String>, CheckoutRuntimeError> {
    match run_git(cwd, arguments, &[0, 1, 128], SMALL_OUTPUT_LIMIT) {
        Ok(output) if output.stdout.trim().is_empty() => Ok(None),
        Ok(output) => Ok(Some(output.stdout.trim().to_owned())),
        Err(error) => Err(error),
    }
}

fn git_required(
    cwd: &Path,
    arguments: &[&str],
    limit: u64,
) -> Result<String, CheckoutRuntimeError> {
    run_git(cwd, arguments, &[0], limit).map(|output| output.stdout.trim_end().to_owned())
}

fn run_git(
    cwd: &Path,
    arguments: &[&str],
    accepted_exit_codes: &[i32],
    output_limit: u64,
) -> Result<CommandOutput, CheckoutRuntimeError> {
    run_git_with_timeout(
        cwd,
        arguments,
        accepted_exit_codes,
        output_limit,
        READ_TIMEOUT,
    )
}

fn run_git_with_timeout(
    cwd: &Path,
    arguments: &[&str],
    accepted_exit_codes: &[i32],
    output_limit: u64,
    timeout: Duration,
) -> Result<CommandOutput, CheckoutRuntimeError> {
    let mut stdout = tempfile::tempfile().map_err(|error| io_error(&error))?;
    let mut stderr = tempfile::tempfile().map_err(|error| io_error(&error))?;
    let mut command = Command::new("git");
    command
        .arg("--no-optional-locks")
        .args(["-c", "core.fsmonitor=false"])
        .args(["-c", "color.ui=false"])
        .args(["-c", "core.quotepath=false"])
        .args(["-c", "diff.noprefix=false"])
        .args(["-c", "diff.mnemonicPrefix=false"])
        .args(arguments)
        .current_dir(cwd)
        .stdin(Stdio::null())
        .stdout(stdout.try_clone().map_err(|error| io_error(&error))?)
        .stderr(stderr.try_clone().map_err(|error| io_error(&error))?);
    for (name, _) in std::env::vars_os() {
        if name.to_string_lossy().starts_with("GIT_") {
            command.env_remove(name);
        }
    }
    let mut child = command.spawn().map_err(|error| io_error(&error))?;
    let deadline = Instant::now() + timeout;
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break status,
            Ok(None)
                if Instant::now() < deadline
                    && stdout
                        .metadata()
                        .is_ok_and(|metadata| metadata.len() <= output_limit)
                    && stderr
                        .metadata()
                        .is_ok_and(|metadata| metadata.len() <= 64 * 1024) =>
            {
                std::thread::sleep(Duration::from_millis(10));
            }
            Ok(None) | Err(_) => {
                let too_large = stdout
                    .metadata()
                    .is_ok_and(|metadata| metadata.len() > output_limit);
                let _ = child.kill();
                let _ = child.wait();
                return Err(checkout_error(
                    CheckoutFailureKind::Unknown,
                    if too_large {
                        "Git output exceeded the configured limit"
                    } else {
                        "Git command timed out"
                    },
                ));
            }
        }
    };
    let code = status.code().unwrap_or(-1);
    if !accepted_exit_codes.contains(&code) {
        let diagnostic = read_bounded(&mut stderr, 64 * 1024)
            .unwrap_or_default()
            .trim()
            .replace(['\r', '\n'], " ");
        let kind = classify_git_error(&diagnostic);
        return Err(checkout_error(
            kind,
            if diagnostic.is_empty() {
                format!("Git command failed with exit code {code}")
            } else {
                diagnostic
            },
        ));
    }
    let bytes = read_bytes_bounded(&mut stdout, output_limit)?;
    Ok(CommandOutput {
        stdout: String::from_utf8_lossy(&bytes).into_owned(),
        exit_code: code,
    })
}

fn git_write(cwd: &Path, arguments: &[&str]) -> Result<String, CheckoutRuntimeError> {
    run_git_with_timeout(cwd, arguments, &[0], COMMIT_OUTPUT_LIMIT, WRITE_TIMEOUT)
        .map(|output| output.stdout.trim_end().to_owned())
}

fn read_bounded(file: &mut File, limit: u64) -> Result<String, CheckoutRuntimeError> {
    Ok(String::from_utf8_lossy(&read_bytes_bounded(file, limit)?).into_owned())
}

fn read_bytes_bounded(file: &mut File, limit: u64) -> Result<Vec<u8>, CheckoutRuntimeError> {
    file.seek(SeekFrom::Start(0))
        .map_err(|error| io_error(&error))?;
    let mut bytes = Vec::new();
    file.take(limit + 1)
        .read_to_end(&mut bytes)
        .map_err(|error| io_error(&error))?;
    if bytes.len() as u64 > limit {
        return Err(checkout_error(
            CheckoutFailureKind::Unknown,
            "Git output exceeded the configured limit",
        ));
    }
    Ok(bytes)
}

fn classify_git_error(message: &str) -> CheckoutFailureKind {
    let lower = message.to_ascii_lowercase();
    if lower.contains("not a git repository") {
        CheckoutFailureKind::NotGitRepository
    } else if lower.contains("conflict") || lower.contains("unmerged") {
        CheckoutFailureKind::MergeConflict
    } else if lower.contains("unsafe") || lower.contains("outside") {
        CheckoutFailureKind::NotAllowed
    } else {
        CheckoutFailureKind::Unknown
    }
}

fn io_error(error: &std::io::Error) -> CheckoutRuntimeError {
    checkout_error(CheckoutFailureKind::Unknown, error.to_string())
}

fn checkout_error(kind: CheckoutFailureKind, message: impl Into<String>) -> CheckoutRuntimeError {
    CheckoutRuntimeError {
        kind,
        message: message.into(),
    }
}

fn resolve_default_branch(
    cwd: &Path,
    current_branch: Option<&str>,
) -> Result<Option<String>, CheckoutRuntimeError> {
    if let Some(remote) = git_optional(
        cwd,
        &[
            "symbolic-ref",
            "--quiet",
            "--short",
            "refs/remotes/origin/HEAD",
        ],
    )? {
        return Ok(Some(
            remote.strip_prefix("origin/").unwrap_or(&remote).to_owned(),
        ));
    }
    for branch in ["main", "master"] {
        if run_git(
            cwd,
            &[
                "show-ref",
                "--verify",
                "--quiet",
                &format!("refs/heads/{branch}"),
            ],
            &[0],
            SMALL_OUTPUT_LIMIT,
        )
        .is_ok()
        {
            return Ok(Some(branch.to_owned()));
        }
    }
    Ok(current_branch.map(str::to_owned))
}

fn compare_refs(
    cwd: &Path,
    base: &str,
    target: &str,
) -> Result<Option<AheadBehind>, CheckoutRuntimeError> {
    let Some(output) = git_optional(
        cwd,
        &[
            "rev-list",
            "--left-right",
            "--count",
            &format!("{base}...{target}"),
        ],
    )?
    else {
        return Ok(None);
    };
    let mut fields = output.split_whitespace();
    let behind = fields.next().and_then(|value| value.parse::<u64>().ok());
    let ahead = fields.next().and_then(|value| value.parse::<u64>().ok());
    Ok(ahead
        .zip(behind)
        .map(|(ahead, behind)| AheadBehind { ahead, behind }))
}

fn validate_ref(value: &str) -> Result<(), CheckoutRuntimeError> {
    if value.is_empty()
        || value.len() > 256
        || value.starts_with('-')
        || value
            .bytes()
            .any(|byte| byte.is_ascii_control() || byte.is_ascii_whitespace())
    {
        return Err(checkout_error(
            CheckoutFailureKind::NotAllowed,
            "Invalid Git ref",
        ));
    }
    Ok(())
}

fn validate_branch_name(value: &str) -> Result<String, CheckoutRuntimeError> {
    let value = value.trim();
    if value.is_empty()
        || value.len() > 256
        || value.contains("..")
        || value.contains("@{")
        || !value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b'/' | b'-'))
    {
        return Err(checkout_error(
            CheckoutFailureKind::NotAllowed,
            format!("Invalid branch: {value}"),
        ));
    }
    Ok(value.to_owned())
}

fn normalize_branch_name(value: &str) -> Result<Option<String>, CheckoutRuntimeError> {
    let validated = validate_branch_name(value)?;
    Ok(normalize_branch_suggestion_name(&validated))
}

fn normalize_branch_suggestion_name(value: &str) -> Option<String> {
    let mut name = value.trim();
    if let Some(stripped) = name.strip_prefix("refs/heads/") {
        name = stripped;
    } else if let Some(stripped) = name.strip_prefix("refs/remotes/") {
        name = stripped;
    }
    if let Some(stripped) = name.strip_prefix("origin/") {
        name = stripped;
    }
    (!name.is_empty() && name != "HEAD" && name != "origin").then(|| name.to_owned())
}

fn validate_branch_slug(value: &str) -> Result<String, CheckoutRuntimeError> {
    let error = if value.is_empty() {
        Some("Branch name cannot be empty")
    } else if value.len() > 100 {
        Some("Branch name too long (max 100 characters)")
    } else if !value.bytes().all(|byte| {
        byte.is_ascii_lowercase() || byte.is_ascii_digit() || matches!(byte, b'-' | b'/')
    }) {
        Some(
            "Branch name must contain only lowercase letters, numbers, hyphens, and forward slashes",
        )
    } else if value.starts_with('-') || value.ends_with('-') {
        Some("Branch name cannot start or end with a hyphen")
    } else if value.contains("--") {
        Some("Branch name cannot have consecutive hyphens")
    } else {
        None
    };
    match error {
        Some(error) => Err(checkout_error(CheckoutFailureKind::Unknown, error)),
        None => Ok(value.to_owned()),
    }
}

fn ref_exists(cwd: &Path, reference: &str) -> Result<bool, CheckoutRuntimeError> {
    Ok(run_git(
        cwd,
        &["show-ref", "--verify", "--quiet", reference],
        &[0, 1],
        SMALL_OUTPUT_LIMIT,
    )?
    .exit_code
        == 0)
}

fn resolve_branch(
    cwd: &Path,
    branch: &str,
) -> Result<CheckoutBranchResolution, CheckoutRuntimeError> {
    let Some(name) = normalize_branch_name(branch)? else {
        return Ok(CheckoutBranchResolution::NotFound);
    };
    if ref_exists(cwd, &format!("refs/heads/{name}"))? {
        return Ok(CheckoutBranchResolution::Local(name));
    }
    let remote_ref = format!("origin/{name}");
    if ref_exists(cwd, &format!("refs/remotes/{remote_ref}"))? {
        return Ok(CheckoutBranchResolution::RemoteOnly { name, remote_ref });
    }
    Ok(CheckoutBranchResolution::NotFound)
}

#[derive(Debug)]
struct BranchMeta {
    committer_date: i64,
    local_oid: Option<String>,
    remote_oid: Option<String>,
}

fn list_branch_suggestions(
    cwd: &Path,
    query: Option<&str>,
    limit: usize,
) -> Result<Vec<CheckoutBranchSuggestion>, CheckoutRuntimeError> {
    let mut branches = BTreeMap::<String, BranchMeta>::new();
    for (prefix, remote) in [("refs/heads", false), ("refs/remotes/origin", true)] {
        let output = git_required(
            cwd,
            &[
                "for-each-ref",
                "--sort=-committerdate",
                "--format=%(refname)%09%(committerdate:unix)%09%(objectname)",
                prefix,
            ],
            COMMIT_OUTPUT_LIMIT,
        )?;
        for line in output.lines() {
            let mut fields = line.split('\t');
            let Some(raw_name) = fields.next() else {
                continue;
            };
            let Some(date) = fields.next().and_then(|value| value.parse::<i64>().ok()) else {
                continue;
            };
            let Some(oid) = fields.next().filter(|value| !value.is_empty()) else {
                continue;
            };
            let Some(name) = normalize_branch_suggestion_name(raw_name) else {
                continue;
            };
            let entry = branches.entry(name).or_insert_with(|| BranchMeta {
                committer_date: 0,
                local_oid: None,
                remote_oid: None,
            });
            entry.committer_date = entry.committer_date.max(date);
            if remote {
                entry.remote_oid = Some(oid.to_owned());
            } else {
                entry.local_oid = Some(oid.to_owned());
            }
        }
    }
    let raw_query = query.unwrap_or_default().trim().to_ascii_lowercase();
    let query = normalize_branch_suggestion_name(&raw_query).unwrap_or(raw_query);
    let mut names = branches
        .keys()
        .filter(|name| query.is_empty() || name.to_ascii_lowercase().contains(&query))
        .cloned()
        .collect::<Vec<_>>();
    names.sort_by(|left, right| {
        let left_prefix = left.to_ascii_lowercase().starts_with(&query);
        let right_prefix = right.to_ascii_lowercase().starts_with(&query);
        right_prefix
            .cmp(&left_prefix)
            .then_with(|| {
                branches[right]
                    .committer_date
                    .cmp(&branches[left].committer_date)
            })
            .then_with(|| left.cmp(right))
    });
    names
        .into_iter()
        .take(limit.clamp(1, 200))
        .map(|name| {
            let meta = &branches[&name];
            let divergence =
                if let (Some(local), Some(remote)) = (&meta.local_oid, &meta.remote_oid) {
                    if local == remote {
                        Some((0, 0))
                    } else {
                        compare_refs(cwd, &format!("origin/{name}"), &name)?
                            .map(|counts| (counts.ahead, counts.behind))
                    }
                } else {
                    None
                };
            Ok(CheckoutBranchSuggestion {
                name,
                committer_date: meta.committer_date,
                has_local: meta.local_oid.is_some(),
                has_remote: meta.remote_oid.is_some(),
                local_ahead: divergence.map(|counts| counts.0),
                local_behind: divergence.map(|counts| counts.1),
            })
        })
        .collect()
}

fn require_clean(cwd: &Path) -> Result<(), CheckoutRuntimeError> {
    let status = git_required(
        cwd,
        &["status", "--porcelain=v1", "--untracked-files=normal"],
        SMALL_OUTPUT_LIMIT,
    )?;
    if status.is_empty() {
        Ok(())
    } else {
        Err(checkout_error(
            CheckoutFailureKind::Unknown,
            "Working directory has uncommitted changes.",
        ))
    }
}

fn current_branch(cwd: &Path, operation: &str) -> Result<String, CheckoutRuntimeError> {
    git_optional(cwd, &["symbolic-ref", "--quiet", "--short", "HEAD"])?.ok_or_else(|| {
        checkout_error(
            CheckoutFailureKind::Unknown,
            format!("Unable to determine current branch for {operation}"),
        )
    })
}

fn operation_base(
    cwd: &Path,
    requested: Option<&str>,
    current: Option<&str>,
) -> Result<String, CheckoutRuntimeError> {
    requested
        .map(str::trim)
        .filter(|base| !base.is_empty())
        .map(str::to_owned)
        .map_or_else(
            || resolve_default_branch(cwd, current),
            |base| Ok(Some(base)),
        )?
        .ok_or_else(|| {
            checkout_error(
                CheckoutFailureKind::Unknown,
                "Unable to determine base branch for merge",
            )
        })
}

fn local_base_name(base: &str) -> Result<String, CheckoutRuntimeError> {
    if base.starts_with("refs/remotes/") && !base.starts_with("refs/remotes/origin/") {
        return Err(checkout_error(
            CheckoutFailureKind::Unknown,
            format!("No local merge target is recorded for base ref {base}"),
        ));
    }
    normalize_branch_name(base)?
        .ok_or_else(|| checkout_error(CheckoutFailureKind::NotAllowed, "Invalid base branch"))
}

fn verify_local_branch(cwd: &Path, branch: &str) -> Result<(), CheckoutRuntimeError> {
    if ref_exists(cwd, &format!("refs/heads/{branch}"))? {
        Ok(())
    } else {
        Err(checkout_error(
            CheckoutFailureKind::Unknown,
            format!("Base branch not found locally: {branch}"),
        ))
    }
}

fn worktree_for_branch(cwd: &Path, branch: &str) -> Result<Option<PathBuf>, CheckoutRuntimeError> {
    let output = git_required(
        cwd,
        &["worktree", "list", "--porcelain"],
        COMMIT_OUTPUT_LIMIT,
    )?;
    let wanted = format!("refs/heads/{branch}");
    let mut path = None;
    for line in output.lines() {
        if let Some(value) = line.strip_prefix("worktree ") {
            path = Some(PathBuf::from(value));
        } else if line.strip_prefix("branch ") == Some(wanted.as_str()) {
            return Ok(path);
        }
    }
    Ok(None)
}

fn most_ahead_base(cwd: &Path, base: &str) -> Result<String, CheckoutRuntimeError> {
    if base.starts_with("refs/heads/") || base.starts_with("refs/remotes/") {
        if ref_exists(cwd, base)? {
            return Ok(base.to_owned());
        }
        return Err(checkout_error(
            CheckoutFailureKind::Unknown,
            format!("Base ref not found: {base}"),
        ));
    }
    let name = local_base_name(base)?;
    let local = ref_exists(cwd, &format!("refs/heads/{name}"))?;
    let origin = ref_exists(cwd, &format!("refs/remotes/origin/{name}"))?;
    match (local, origin) {
        (true, false) => Ok(name),
        (false, true) => Ok(format!("origin/{name}")),
        (false, false) => Err(checkout_error(
            CheckoutFailureKind::Unknown,
            format!("Base branch not found locally or on origin: {name}"),
        )),
        (true, true) => {
            let counts = compare_refs(cwd, &name, &format!("origin/{name}"))?;
            if counts.is_some_and(|counts| counts.ahead > counts.behind) {
                Ok(format!("origin/{name}"))
            } else {
                Ok(name)
            }
        }
    }
}

fn abort_merge_on_conflict(
    cwd: &Path,
    result: Result<(), CheckoutRuntimeError>,
) -> Result<(), CheckoutRuntimeError> {
    let Err(mut error) = result else {
        return Ok(());
    };
    let conflicts = git_optional(cwd, &["diff", "--name-only", "--diff-filter=U"])
        .ok()
        .flatten()
        .is_some_and(|paths| !paths.is_empty());
    if error.kind == CheckoutFailureKind::MergeConflict || conflicts {
        let _ = git_write(cwd, &["merge", "--abort"]);
        error.kind = CheckoutFailureKind::MergeConflict;
    }
    Err(error)
}

fn require_origin(cwd: &Path) -> Result<(), CheckoutRuntimeError> {
    let remotes = lines(&git_required(cwd, &["remote"], SMALL_OUTPUT_LIMIT)?);
    if remotes.iter().any(|remote| remote == "origin") {
        Ok(())
    } else {
        Err(checkout_error(
            CheckoutFailureKind::Unknown,
            "Remote 'origin' is not configured.",
        ))
    }
}

fn abort_pull_state(cwd: &Path) {
    let _ = git_write(cwd, &["merge", "--abort"]);
    let _ = git_write(cwd, &["rebase", "--abort"]);
}

fn configured_push_target(
    cwd: &Path,
    branch: &str,
) -> Result<Option<(String, String)>, CheckoutRuntimeError> {
    let Some(remote) = git_optional(
        cwd,
        &["config", "--get", &format!("branch.{branch}.pushRemote")],
    )?
    else {
        return Ok(None);
    };
    let Some(refspec) = git_optional(cwd, &["config", "--get", &format!("remote.{remote}.push")])?
    else {
        return Ok(None);
    };
    if git_optional(cwd, &["config", "--get", &format!("remote.{remote}.url")])?.is_none() {
        return Ok(None);
    }
    let normalized = refspec.trim().strip_prefix('+').unwrap_or(refspec.trim());
    Ok(normalized
        .strip_prefix("HEAD:refs/heads/")
        .map(str::trim)
        .filter(|head| !head.is_empty())
        .map(|head| (remote, head.to_owned())))
}

fn discard_paths(cwd: &Path, paths: &[&str]) -> Result<(), CheckoutRuntimeError> {
    let mut reset = vec!["--literal-pathspecs", "reset", "-q", "HEAD", "--"];
    reset.extend_from_slice(paths);
    if git_write(cwd, &reset).is_err() {
        let mut remove = vec![
            "--literal-pathspecs",
            "rm",
            "--cached",
            "-r",
            "-q",
            "--ignore-unmatch",
            "--",
        ];
        remove.extend_from_slice(paths);
        git_write(cwd, &remove)?;
    }
    let mut status = vec![
        "--literal-pathspecs",
        "status",
        "--porcelain=v1",
        "-z",
        "--",
    ];
    status.extend_from_slice(paths);
    let output = git_write(cwd, &status)?;
    let mut tracked = Vec::new();
    let mut untracked = Vec::new();
    let mut tokens = output.split('\0');
    while let Some(token) = tokens.next() {
        if token.len() < 4 {
            continue;
        }
        let state = &token[..2];
        let path = &token[3..];
        if state.starts_with('R') || state.starts_with('C') {
            let _ = tokens.next();
        }
        if state == "??" {
            untracked.push(path);
        } else {
            tracked.push(path);
        }
    }
    if !tracked.is_empty() {
        let mut checkout = vec!["--literal-pathspecs", "checkout", "-q", "--"];
        checkout.extend(tracked);
        git_write(cwd, &checkout)?;
    }
    if !untracked.is_empty() {
        let mut clean = vec!["--literal-pathspecs", "clean", "-fd", "-q", "--"];
        clean.extend(untracked);
        git_write(cwd, &clean)?;
    }
    Ok(())
}

fn parse_stashes(output: &str, paseo_only: bool) -> Vec<CheckoutStashEntry> {
    output
        .lines()
        .filter_map(|line| {
            let (reference, message) = line.split_once('\0')?;
            let index = reference
                .split_once('{')?
                .1
                .strip_suffix('}')?
                .parse::<usize>()
                .ok()?;
            let prefix = "paseo-auto-stash:";
            let branch = message
                .find(prefix)
                .map(|position| message[position + prefix.len()..].trim())
                .filter(|branch| !branch.is_empty())
                .map(str::to_owned);
            let is_paseo = message.contains(prefix);
            (!paseo_only || is_paseo).then(|| CheckoutStashEntry {
                index,
                message: message.to_owned(),
                branch,
                is_paseo,
            })
        })
        .collect()
}

fn validate_commit(value: &str) -> Result<(), CheckoutRuntimeError> {
    if !(4..=64).contains(&value.len()) || !value.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return Err(checkout_error(
            CheckoutFailureKind::NotAllowed,
            "Invalid commit identity",
        ));
    }
    Ok(())
}

fn validate_relative_path(value: &str) -> Result<(), CheckoutRuntimeError> {
    let path = Path::new(value);
    if value.is_empty()
        || value.len() > 4096
        || path.is_absolute()
        || path
            .components()
            .any(|component| !matches!(component, Component::Normal(_)))
    {
        return Err(checkout_error(
            CheckoutFailureKind::NotAllowed,
            "Invalid checkout-relative path",
        ));
    }
    Ok(())
}

fn verify_commit(cwd: &Path, revision: &str) -> Result<(), CheckoutRuntimeError> {
    validate_ref(revision)?;
    git_required(
        cwd,
        &["rev-parse", "--verify", &format!("{revision}^{{commit}}")],
        SMALL_OUTPUT_LIMIT,
    )
    .map(|_| ())
}

fn diff_head(cwd: &Path) -> Result<String, CheckoutRuntimeError> {
    if git_optional(cwd, &["rev-parse", "--verify", "HEAD^{commit}"])?.is_some() {
        return Ok("HEAD".to_owned());
    }
    // Git reads empty stdin here. Resolve its empty tree using the repository's object format,
    // so staged and unstaged files remain visible before the first SHA-1 or SHA-256 commit.
    git_required(
        cwd,
        &["hash-object", "-t", "tree", "--stdin"],
        SMALL_OUTPUT_LIMIT,
    )
}

fn parse_diff(text: &str) -> Vec<ParsedDiffFile> {
    text.split("diff --git ")
        .skip(1)
        .filter_map(parse_diff_section)
        .collect()
}

fn parse_diff_section(section: &str) -> Option<ParsedDiffFile> {
    let lines = section.lines().collect::<Vec<_>>();
    let header = *lines.first()?;
    let is_new = section.contains("\nnew file mode ") || section.contains("\n--- /dev/null");
    let is_deleted =
        section.contains("\ndeleted file mode ") || section.contains("\n+++ /dev/null");
    let old_path = lines
        .iter()
        .find_map(|line| line.strip_prefix("rename from "))
        .map(str::to_owned);
    let renamed_path = lines
        .iter()
        .find_map(|line| line.strip_prefix("rename to "));
    let metadata_path = lines
        .iter()
        .find_map(|line| line.strip_prefix("+++ "))
        .filter(|path| *path != "/dev/null")
        .or_else(|| {
            lines
                .iter()
                .find_map(|line| line.strip_prefix("--- "))
                .filter(|path| *path != "/dev/null")
        });
    let path = renamed_path
        .or(metadata_path)
        .map(|path| path.strip_suffix('\t').unwrap_or(path))
        .map(strip_diff_prefix)
        .or_else(|| header.split_once(" b/").map(|(_, path)| path))?
        .to_owned();
    let binary = section
        .lines()
        .any(|line| line.starts_with("Binary files ") || line.starts_with("GIT binary patch"));
    let mut hunks = Vec::new();
    let mut current: Option<DiffHunk> = None;
    let mut additions = 0_u64;
    let mut deletions = 0_u64;
    for line in lines.iter().skip(1) {
        if let Some((old_start, old_count, new_start, new_count, header)) = parse_hunk_header(line)
        {
            if let Some(hunk) = current.take() {
                hunks.push(hunk);
            }
            current = Some(DiffHunk {
                old_start,
                old_count,
                new_start,
                new_count,
                lines: vec![DiffLine {
                    kind: DiffLineKind::Header,
                    content: header,
                }],
            });
            continue;
        }
        let Some(hunk) = current.as_mut() else {
            continue;
        };
        let (kind, content) = if let Some(content) = line.strip_prefix('+') {
            additions = additions.saturating_add(1);
            (DiffLineKind::Add, content)
        } else if let Some(content) = line.strip_prefix('-') {
            deletions = deletions.saturating_add(1);
            (DiffLineKind::Remove, content)
        } else if let Some(content) = line.strip_prefix(' ') {
            (DiffLineKind::Context, content)
        } else {
            continue;
        };
        hunk.lines.push(DiffLine {
            kind,
            content: content.to_owned(),
        });
    }
    if let Some(hunk) = current {
        hunks.push(hunk);
    }
    Some(ParsedDiffFile {
        path,
        old_path,
        is_new,
        is_deleted,
        additions,
        deletions,
        hunks,
        status: binary.then_some(ParsedDiffStatus::Binary),
    })
}

fn strip_diff_prefix(path: &str) -> &str {
    path.strip_prefix("a/")
        .or_else(|| path.strip_prefix("b/"))
        .unwrap_or(path)
}

fn parse_hunk_header(line: &str) -> Option<(u64, u64, u64, u64, String)> {
    let rest = line.strip_prefix("@@ -")?;
    let (old, rest) = rest.split_once(" +")?;
    let (new, suffix) = rest.split_once(" @@")?;
    let (old_start, old_count) = parse_hunk_range(old)?;
    let (new_start, new_count) = parse_hunk_range(new)?;
    Some((
        old_start,
        old_count,
        new_start,
        new_count,
        format!("@@ -{old} +{new} @@{suffix}"),
    ))
}

fn parse_hunk_range(value: &str) -> Option<(u64, u64)> {
    let (start, count) = value.split_once(',').unwrap_or((value, "1"));
    Some((start.parse().ok()?, count.parse().ok()?))
}

fn commit_records(
    cwd: &Path,
    revision: &str,
    max_count: Option<usize>,
) -> Result<Vec<CommitRecord>, CheckoutRuntimeError> {
    let mut arguments = vec!["log".to_owned(), revision.to_owned()];
    if let Some(max_count) = max_count {
        arguments.push(format!("--max-count={max_count}"));
    }
    arguments.extend([
        "--diff-merges=first-parent".to_owned(),
        "--format=%x1e%H%x00%h%x00%an%x00%aI%x00%s".to_owned(),
        "--raw".to_owned(),
        "--numstat".to_owned(),
        "-M".to_owned(),
    ]);
    let refs = arguments.iter().map(String::as_str).collect::<Vec<_>>();
    let output = run_git(cwd, &refs, &[0], COMMIT_OUTPUT_LIMIT)?.stdout;
    Ok(parse_commit_records(&output))
}

fn parse_commit_records(output: &str) -> Vec<CommitRecord> {
    output
        .split(COMMIT_RECORD_SEPARATOR)
        .filter_map(|record| {
            let mut lines = record.lines();
            let fields = lines
                .next()?
                .split(COMMIT_FIELD_SEPARATOR)
                .collect::<Vec<_>>();
            if fields.len() < 5 || fields[0].trim().is_empty() {
                return None;
            }
            let mut stats = BTreeMap::<String, (u64, u64)>::new();
            let mut statuses = BTreeMap::<String, CheckoutCommitFileStatus>::new();
            for line in lines.filter(|line| !line.is_empty()) {
                if line.starts_with(':') {
                    parse_raw_status(line, &mut statuses);
                } else {
                    parse_numstat(line, &mut stats);
                }
            }
            let files = stats
                .into_iter()
                .map(|(path, (additions, deletions))| CheckoutCommitFile {
                    status: statuses.get(&path).copied(),
                    path,
                    additions,
                    deletions,
                })
                .collect();
            Some(CommitRecord {
                sha: fields[0].trim().to_owned(),
                short_sha: fields[1].trim().to_owned(),
                author_name: fields[2].to_owned(),
                author_date: fields[3].trim().to_owned(),
                subject: fields[4].to_owned(),
                files,
            })
        })
        .collect()
}

fn parse_raw_status(line: &str, statuses: &mut BTreeMap<String, CheckoutCommitFileStatus>) {
    let parts = line.split('\t').collect::<Vec<_>>();
    let token = parts[0]
        .rsplit_once(' ')
        .map(|(_, token)| token)
        .unwrap_or_default();
    let letter = token.chars().next().unwrap_or_default();
    let status = match letter {
        'A' | 'C' => CheckoutCommitFileStatus::Added,
        'M' | 'T' => CheckoutCommitFileStatus::Modified,
        'D' => CheckoutCommitFileStatus::Deleted,
        'R' => CheckoutCommitFileStatus::Renamed,
        _ => return,
    };
    let path = if matches!(letter, 'R' | 'C') {
        parts.last()
    } else {
        parts.get(1)
    };
    if let Some(path) = path.filter(|path| !path.is_empty()) {
        statuses.insert((*path).to_owned(), status);
    }
}

fn parse_numstat(line: &str, stats: &mut BTreeMap<String, (u64, u64)>) {
    let mut parts = line.splitn(3, '\t');
    let Some(additions) = parts.next() else {
        return;
    };
    let Some(deletions) = parts.next() else {
        return;
    };
    let Some(path) = parts.next().map(normalize_numstat_path) else {
        return;
    };
    let additions = additions.parse().unwrap_or(0);
    let deletions = deletions.parse().unwrap_or(0);
    stats.insert(path, (additions, deletions));
}

fn normalize_numstat_path(path: &str) -> String {
    let Some((left, right)) = path.rsplit_once(" => ") else {
        return path.to_owned();
    };
    if let Some(open) = left.rfind('{')
        && let Some(close) = right.find('}')
    {
        return format!(
            "{}{}{}",
            &left[..open],
            &right[..close],
            &right[close + 1..]
        );
    }
    right.to_owned()
}

#[cfg(test)]
mod tests;
