//! Bounded local Git reads for checkout status, diff, and commit history.

use std::collections::{BTreeMap, BTreeSet};
use std::fs::File;
use std::io::{Read, Seek, SeekFrom};
use std::path::{Component, Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use server_ports::checkout::{
    AheadBehind, CheckoutCommit, CheckoutCommitFile, CheckoutCommitFileStatus, CheckoutCommits,
    CheckoutDiff, CheckoutDiffCompare, CheckoutDiffMode, CheckoutFailureKind, CheckoutRuntime,
    CheckoutRuntimeError, CheckoutStatus, DiffHunk, DiffLine, DiffLineKind, ParsedDiffFile,
    ParsedDiffStatus,
};

const READ_TIMEOUT: Duration = Duration::from_secs(10);
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
                arguments.push("HEAD".to_owned());
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
}

#[derive(Debug)]
struct CommandOutput {
    stdout: String,
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
    let deadline = Instant::now() + READ_TIMEOUT;
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
    })
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
