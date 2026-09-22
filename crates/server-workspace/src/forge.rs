//! Bounded GitHub CLI adapter for forge search and pull request operations.

use std::collections::BTreeMap;
use std::fs::File;
use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use chrono::DateTime;
use serde_json::Value;
use server_ports::forge::{
    CheckAnnotation, CheckDetails, CheckFailedJob, CheckOutput, ForgeAuthState, ForgeFailureKind,
    ForgeRuntime, ForgeRuntimeError, ForgeSearch, ForgeSearchItem, ForgeSearchKind,
    PullRequestCheck, PullRequestCreated, PullRequestMergeMethod, PullRequestMergeable,
    PullRequestStatus, PullRequestStatusRead, PullRequestTimeline, PullRequestTimelineItem,
    TimelineCommentLocation, TimelineError, TimelineErrorKind, TimelineReviewState,
};

const READ_TIMEOUT: Duration = Duration::from_secs(30);
const WRITE_TIMEOUT: Duration = Duration::from_secs(120);
const OUTPUT_LIMIT: u64 = 4 * 1024 * 1024;
const STDERR_LIMIT: u64 = 64 * 1024;
const CHECK_ANNOTATION_LIMIT: usize = 20;
const CHECK_JOB_LIMIT: usize = 100;
const FAILED_JOB_LIMIT: usize = 5;
const STATUS_FIELDS: &str = "number,url,title,state,isDraft,baseRefName,headRefName,mergedAt,reviewDecision,mergeable,statusCheckRollup";

const TIMELINE_QUERY: &str = r"
query PullRequestTimeline($owner: String!, $name: String!, $number: Int!) {
  repository(owner: $owner, name: $name) {
    pullRequest(number: $number) {
      number
      reviews(first: 100) {
        nodes { id state body url submittedAt author { login url avatarUrl } }
        pageInfo { hasNextPage }
      }
      comments(first: 100) {
        nodes { id body url createdAt author { login url avatarUrl } }
        pageInfo { hasNextPage }
      }
      reviewThreads(first: 100) {
        nodes {
          id path line startLine isResolved isOutdated
          comments(first: 100) {
            nodes { id body url createdAt author { login url avatarUrl } pullRequestReview { id } }
            pageInfo { hasNextPage }
          }
        }
        pageInfo { hasNextPage }
      }
    }
  }
}";

/// Stateless GitHub CLI adapter. The CLI uses the user's existing authentication.
#[derive(Debug, Clone)]
pub struct LocalForge {
    executable: PathBuf,
}

impl Default for LocalForge {
    fn default() -> Self {
        Self::new()
    }
}

impl LocalForge {
    /// Use `gh` resolved from the server process environment.
    #[must_use]
    pub fn new() -> Self {
        Self {
            executable: PathBuf::from("gh"),
        }
    }

    #[cfg(test)]
    fn with_executable(executable: PathBuf) -> Self {
        Self { executable }
    }

    fn forge_context(cwd: &Path) -> Result<ForgeContext, ForgeRuntimeError> {
        let remote = git_optional(cwd, &["config", "--get", "remote.origin.url"], READ_TIMEOUT)?
            .ok_or_else(|| {
                forge_error(ForgeFailureKind::NoRemote, "No origin remote is configured")
            })?;
        let location = parse_remote(&remote).ok_or_else(|| {
            forge_error(
                ForgeFailureKind::NoRemote,
                "No supported forge remote is configured for this workspace",
            )
        })?;
        Ok(ForgeContext {
            host: location.host,
            project_path: location.project_path,
        })
    }

    fn gh(
        &self,
        cwd: &Path,
        context: &ForgeContext,
        arguments: &[String],
        timeout: Duration,
    ) -> Result<String, ForgeRuntimeError> {
        let mut command = Command::new(&self.executable);
        command
            .args(arguments)
            .current_dir(cwd)
            .env("GIT_TERMINAL_PROMPT", "0")
            .env("GH_PROMPT_DISABLED", "1");
        if context.host != "github.com" {
            command.env("GH_HOST", &context.host);
        }
        run_command(command, timeout, CommandFamily::Forge).map(|output| output.stdout)
    }

    fn status_with_context(
        &self,
        cwd: &Path,
        context: &ForgeContext,
    ) -> Result<PullRequestStatusRead, ForgeRuntimeError> {
        let branch = git_optional(
            cwd,
            &["symbolic-ref", "--quiet", "--short", "HEAD"],
            READ_TIMEOUT,
        )?;
        let Some(branch) = branch else {
            return Ok(PullRequestStatusRead {
                status: None,
                auth_state: ForgeAuthState::NoRemote,
                forge: Some("github".to_owned()),
            });
        };
        let arguments = strings(&["pr", "view", "--json", STATUS_FIELDS]);
        match self.gh(cwd, context, &arguments, READ_TIMEOUT) {
            Ok(output) => Ok(PullRequestStatusRead {
                status: parse_status(&output, &branch, context)?,
                auth_state: ForgeAuthState::Authenticated,
                forge: Some("github".to_owned()),
            }),
            Err(error) if error.kind == ForgeFailureKind::NotFound => Ok(PullRequestStatusRead {
                status: None,
                auth_state: ForgeAuthState::Authenticated,
                forge: Some("github".to_owned()),
            }),
            Err(error) if error.kind == ForgeFailureKind::CliMissing => Ok(PullRequestStatusRead {
                status: None,
                auth_state: ForgeAuthState::CliMissing,
                forge: Some("github".to_owned()),
            }),
            Err(error) if error.kind == ForgeFailureKind::Unauthenticated => {
                Ok(PullRequestStatusRead {
                    status: None,
                    auth_state: ForgeAuthState::Unauthenticated,
                    forge: Some("github".to_owned()),
                })
            }
            Err(error) => Err(error),
        }
    }
}

impl ForgeRuntime for LocalForge {
    fn search(
        &self,
        cwd: &str,
        query: &str,
        limit: usize,
        kinds: &[ForgeSearchKind],
    ) -> Result<ForgeSearch, ForgeRuntimeError> {
        let cwd = require_git_directory(cwd)?;
        let context = match Self::forge_context(&cwd) {
            Ok(context) => context,
            Err(error) if error.kind == ForgeFailureKind::NoRemote => {
                return Ok(unavailable_search(ForgeAuthState::NoRemote));
            }
            Err(error) => return Err(error),
        };
        let limit = limit.clamp(1, 50);
        let mut attempts = Vec::new();
        if kinds.contains(&ForgeSearchKind::Issue) {
            attempts.push((
                ForgeSearchKind::Issue,
                self.gh(
                    &cwd,
                    &context,
                    &strings(&[
                        "issue",
                        "list",
                        "--search",
                        query,
                        "--json",
                        "number,title,url,state,body,labels,updatedAt",
                        "--limit",
                        &limit.to_string(),
                    ]),
                    READ_TIMEOUT,
                ),
            ));
        }
        if kinds.contains(&ForgeSearchKind::ChangeRequest) {
            attempts.push((
                ForgeSearchKind::ChangeRequest,
                self.gh(
                    &cwd,
                    &context,
                    &strings(&[
                        "pr",
                        "list",
                        "--search",
                        query,
                        "--json",
                        "number,title,url,state,body,labels,baseRefName,headRefName,updatedAt",
                        "--limit",
                        &limit.to_string(),
                    ]),
                    READ_TIMEOUT,
                ),
            ));
        }
        let requested = attempts.len();
        let mut items = Vec::new();
        let mut failures = Vec::new();
        for (kind, attempt) in attempts {
            match attempt {
                Ok(output) => items.extend(parse_search_items(&output, kind, &context)?),
                Err(error) => failures.push(error),
            }
        }
        if requested > 0 && failures.len() == requested {
            if failures
                .iter()
                .any(|error| error.kind == ForgeFailureKind::CliMissing)
            {
                return Ok(unavailable_search(ForgeAuthState::CliMissing));
            }
            if failures
                .iter()
                .all(|error| error.kind == ForgeFailureKind::Unauthenticated)
            {
                return Ok(unavailable_search(ForgeAuthState::Unauthenticated));
            }
        }
        items.sort_by(|left, right| right.updated_at.cmp(&left.updated_at));
        items.truncate(limit);
        Ok(ForgeSearch {
            items,
            auth_state: ForgeAuthState::Authenticated,
        })
    }

    fn create_pull_request(
        &self,
        cwd: &str,
        title: &str,
        body: &str,
        base_ref: Option<&str>,
    ) -> Result<PullRequestCreated, ForgeRuntimeError> {
        let cwd = require_git_directory(cwd)?;
        let context = Self::forge_context(&cwd)?;
        let head = git_required(
            &cwd,
            &["symbolic-ref", "--quiet", "--short", "HEAD"],
            READ_TIMEOUT,
        )?;
        let base = resolve_base(&cwd, base_ref, &head)?;
        git_required(&cwd, &["push", "-u", "origin", &head], WRITE_TIMEOUT)?;
        let arguments = vec![
            "api".to_owned(),
            "-X".to_owned(),
            "POST".to_owned(),
            format!("repos/{}/pulls", context.project_path),
            "-f".to_owned(),
            format!("title={title}"),
            "-f".to_owned(),
            format!("head={head}"),
            "-f".to_owned(),
            format!("base={base}"),
            "-f".to_owned(),
            format!("body={body}"),
        ];
        let output = self.gh(&cwd, &context, &arguments, WRITE_TIMEOUT)?;
        let value = parse_json(&output)?;
        let number = integer(&value, "number")?;
        let url = optional_string(&value, "html_url")
            .or_else(|| optional_string(&value, "url"))
            .ok_or_else(|| malformed("Pull request response is missing url"))?;
        Ok(PullRequestCreated { url, number })
    }

    fn current_pull_request_status(
        &self,
        cwd: &str,
    ) -> Result<PullRequestStatusRead, ForgeRuntimeError> {
        let cwd = require_git_directory(cwd)?;
        let context = match Self::forge_context(&cwd) {
            Ok(context) => context,
            Err(error) if error.kind == ForgeFailureKind::NoRemote => {
                return Ok(PullRequestStatusRead {
                    status: None,
                    auth_state: ForgeAuthState::NoRemote,
                    forge: None,
                });
            }
            Err(error) => return Err(error),
        };
        self.status_with_context(&cwd, &context)
    }

    fn merge_current_pull_request(
        &self,
        cwd: &str,
        merge_method: PullRequestMergeMethod,
    ) -> Result<(), ForgeRuntimeError> {
        let cwd = require_git_directory(cwd)?;
        let context = Self::forge_context(&cwd)?;
        let status = self.status_with_context(&cwd, &context)?;
        let number = current_number(&status, "merge")?;
        self.gh(
            &cwd,
            &context,
            &strings(&[
                "pr",
                "merge",
                &number.to_string(),
                merge_method_flag(merge_method),
            ]),
            WRITE_TIMEOUT,
        )?;
        Ok(())
    }

    fn set_current_pull_request_auto_merge(
        &self,
        cwd: &str,
        enabled: bool,
        merge_method: Option<PullRequestMergeMethod>,
    ) -> Result<(), ForgeRuntimeError> {
        let cwd = require_git_directory(cwd)?;
        let context = Self::forge_context(&cwd)?;
        let status = self.status_with_context(&cwd, &context)?;
        let number = current_number(&status, "auto-merge")?;
        let mut arguments = strings(&["pr", "merge", &number.to_string()]);
        if enabled {
            let method = merge_method.ok_or_else(|| {
                forge_error(
                    ForgeFailureKind::Invalid,
                    "mergeMethod is required when enabling auto-merge",
                )
            })?;
            arguments.push("--auto".to_owned());
            arguments.push(merge_method_flag(method).to_owned());
        } else {
            arguments.push("--disable-auto".to_owned());
        }
        self.gh(&cwd, &context, &arguments, WRITE_TIMEOUT)?;
        Ok(())
    }

    fn pull_request_timeline(
        &self,
        cwd: &str,
        pr_number: u64,
        repo_owner: &str,
        repo_name: &str,
    ) -> Result<PullRequestTimeline, ForgeRuntimeError> {
        let cwd = require_git_directory(cwd)?;
        let context = Self::forge_context(&cwd)?;
        let arguments = vec![
            "api".to_owned(),
            "graphql".to_owned(),
            "-f".to_owned(),
            format!("query={TIMELINE_QUERY}"),
            "-F".to_owned(),
            format!("owner={repo_owner}"),
            "-F".to_owned(),
            format!("name={repo_name}"),
            "-F".to_owned(),
            format!("number={pr_number}"),
        ];
        match self.gh(&cwd, &context, &arguments, READ_TIMEOUT) {
            Ok(output) => parse_timeline(&output, pr_number),
            Err(error)
                if matches!(
                    error.kind,
                    ForgeFailureKind::NotFound | ForgeFailureKind::Forbidden
                ) =>
            {
                Ok(PullRequestTimeline {
                    pr_number,
                    items: Vec::new(),
                    truncated: false,
                    error: Some(TimelineError {
                        kind: if error.kind == ForgeFailureKind::NotFound {
                            TimelineErrorKind::NotFound
                        } else {
                            TimelineErrorKind::Forbidden
                        },
                        message: error.message,
                    }),
                    auth_state: ForgeAuthState::Authenticated,
                })
            }
            Err(error) => Err(error),
        }
    }

    fn check_details(
        &self,
        cwd: &str,
        repo_owner: Option<&str>,
        repo_name: Option<&str>,
        check_run_id: Option<u64>,
        workflow_run_id: Option<u64>,
        _change_request_number: Option<u64>,
    ) -> Result<CheckDetails, ForgeRuntimeError> {
        let cwd = require_git_directory(cwd)?;
        let context = Self::forge_context(&cwd)?;
        let owner = repo_owner.ok_or_else(|| {
            forge_error(
                ForgeFailureKind::Invalid,
                "GitHub getCheckDetails requires repoOwner and repoName",
            )
        })?;
        let name = repo_name.ok_or_else(|| {
            forge_error(
                ForgeFailureKind::Invalid,
                "GitHub getCheckDetails requires repoOwner and repoName",
            )
        })?;
        let check_run_id = check_run_id.ok_or_else(|| {
            forge_error(
                ForgeFailureKind::Invalid,
                "GitHub getCheckDetails requires checkRunId",
            )
        })?;
        let repo = format!("repos/{owner}/{name}");
        let check = parse_json(&self.gh(
            &cwd,
            &context,
            &strings(&["api", &format!("{repo}/check-runs/{check_run_id}")]),
            READ_TIMEOUT,
        )?)?;
        let annotations = parse_annotations(&parse_json(&self.gh(
            &cwd,
            &context,
            &strings(&[
                "api",
                &format!("{repo}/check-runs/{check_run_id}/annotations"),
                "-f",
                &format!("per_page={CHECK_ANNOTATION_LIMIT}"),
            ]),
            READ_TIMEOUT,
        )?)?)?;
        let workflow_run_id = workflow_run_id.or_else(|| {
            check
                .pointer("/check_suite/workflow_run/id")
                .and_then(Value::as_u64)
        });
        let (failed_jobs, jobs_truncated) = match workflow_run_id {
            Some(run_id) => parse_failed_jobs(&parse_json(&self.gh(
                &cwd,
                &context,
                &strings(&[
                    "api",
                    &format!("{repo}/actions/runs/{run_id}/jobs"),
                    "-f",
                    &format!("per_page={CHECK_JOB_LIMIT}"),
                ]),
                READ_TIMEOUT,
            )?)?)?,
            None => (Vec::new(), false),
        };
        Ok(CheckDetails {
            check_run_id: integer(&check, "id")?,
            workflow_run_id,
            name: string(&check, "name")?,
            status: optional_string(&check, "status"),
            conclusion: optional_string(&check, "conclusion"),
            url: optional_string(&check, "html_url"),
            details_url: optional_string(&check, "details_url"),
            output: check
                .get("output")
                .filter(|value| !value.is_null())
                .map(|value| CheckOutput {
                    title: optional_string(value, "title"),
                    summary: optional_string(value, "summary"),
                    text: optional_string(value, "text"),
                }),
            truncated: annotations.len() >= CHECK_ANNOTATION_LIMIT || jobs_truncated,
            annotations,
            failed_jobs,
            pipeline: None,
        })
    }
}

#[derive(Debug)]
struct ForgeContext {
    host: String,
    project_path: String,
}

#[derive(Debug)]
struct RemoteLocation {
    host: String,
    project_path: String,
}

#[derive(Debug)]
struct CommandOutput {
    stdout: String,
}

#[derive(Debug, Clone, Copy)]
enum CommandFamily {
    Git,
    Forge,
}

fn unavailable_search(auth_state: ForgeAuthState) -> ForgeSearch {
    ForgeSearch {
        items: Vec::new(),
        auth_state,
    }
}

fn require_git_directory(value: &str) -> Result<PathBuf, ForgeRuntimeError> {
    let path = if value == "~" {
        std::env::var_os("HOME").map(PathBuf::from).ok_or_else(|| {
            forge_error(
                ForgeFailureKind::NotAllowed,
                "Home directory is unavailable",
            )
        })?
    } else if let Some(relative) = value.strip_prefix("~/") {
        std::env::var_os("HOME")
            .map(PathBuf::from)
            .ok_or_else(|| {
                forge_error(
                    ForgeFailureKind::NotAllowed,
                    "Home directory is unavailable",
                )
            })?
            .join(relative)
    } else {
        PathBuf::from(value)
    };
    let path = path
        .canonicalize()
        .map_err(|error| forge_error(ForgeFailureKind::NotAllowed, error.to_string()))?;
    if !path.is_dir() {
        return Err(forge_error(
            ForgeFailureKind::NotAllowed,
            "Checkout cwd is not a directory",
        ));
    }
    if git_optional(&path, &["rev-parse", "--show-toplevel"], READ_TIMEOUT)?.is_none() {
        return Err(forge_error(
            ForgeFailureKind::NotGitRepository,
            "Not a git repository",
        ));
    }
    Ok(path)
}

fn git_optional(
    cwd: &Path,
    arguments: &[&str],
    timeout: Duration,
) -> Result<Option<String>, ForgeRuntimeError> {
    let mut command = git_command(cwd, arguments);
    match run_command_with_codes(&mut command, timeout, CommandFamily::Git, &[0, 1, 128]) {
        Ok(output) if output.stdout.trim().is_empty() => Ok(None),
        Ok(output) => Ok(Some(output.stdout.trim().to_owned())),
        Err(error) => Err(error),
    }
}

fn git_required(
    cwd: &Path,
    arguments: &[&str],
    timeout: Duration,
) -> Result<String, ForgeRuntimeError> {
    let mut command = git_command(cwd, arguments);
    run_command_with_codes(&mut command, timeout, CommandFamily::Git, &[0])
        .map(|output| output.stdout.trim_end().to_owned())
}

fn git_command(cwd: &Path, arguments: &[&str]) -> Command {
    let mut command = Command::new("git");
    command
        .arg("--no-optional-locks")
        .args(["-c", "core.fsmonitor=false"])
        .args(["-c", "color.ui=false"])
        .args(arguments)
        .current_dir(cwd);
    for (name, _) in std::env::vars_os() {
        if name.to_string_lossy().starts_with("GIT_") {
            command.env_remove(name);
        }
    }
    command
}

fn run_command(
    mut command: Command,
    timeout: Duration,
    family: CommandFamily,
) -> Result<CommandOutput, ForgeRuntimeError> {
    run_command_with_codes(&mut command, timeout, family, &[0])
}

fn run_command_with_codes(
    command: &mut Command,
    timeout: Duration,
    family: CommandFamily,
    accepted_codes: &[i32],
) -> Result<CommandOutput, ForgeRuntimeError> {
    let mut stdout = tempfile::tempfile().map_err(|error| io_error(&error))?;
    let mut stderr = tempfile::tempfile().map_err(|error| io_error(&error))?;
    command
        .stdin(Stdio::null())
        .stdout(stdout.try_clone().map_err(|error| io_error(&error))?)
        .stderr(stderr.try_clone().map_err(|error| io_error(&error))?);
    let mut child = command.spawn().map_err(|error| {
        if error.kind() == std::io::ErrorKind::NotFound && matches!(family, CommandFamily::Forge) {
            forge_error(ForgeFailureKind::CliMissing, "GitHub CLI is not installed")
        } else {
            io_error(&error)
        }
    })?;
    let deadline = Instant::now() + timeout;
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break status,
            Ok(None)
                if Instant::now() < deadline
                    && within_limit(&stdout, OUTPUT_LIMIT)
                    && within_limit(&stderr, STDERR_LIMIT) =>
            {
                std::thread::sleep(Duration::from_millis(10));
            }
            Ok(None) | Err(_) => {
                let too_large = !within_limit(&stdout, OUTPUT_LIMIT);
                let _ = child.kill();
                let _ = child.wait();
                return Err(forge_error(
                    ForgeFailureKind::Unknown,
                    if too_large {
                        "Forge command output exceeded the configured limit"
                    } else {
                        "Forge command timed out"
                    },
                ));
            }
        }
    };
    let code = status.code().unwrap_or(-1);
    if !accepted_codes.contains(&code) {
        let diagnostic = read_bounded(&mut stderr, STDERR_LIMIT)?
            .trim()
            .replace(['\r', '\n'], " ");
        let kind = classify_command_error(family, &diagnostic);
        return Err(forge_error(
            kind,
            if diagnostic.is_empty() {
                format!("Command failed with exit code {code}")
            } else {
                diagnostic
            },
        ));
    }
    Ok(CommandOutput {
        stdout: read_bounded(&mut stdout, OUTPUT_LIMIT)?,
    })
}

fn within_limit(file: &File, limit: u64) -> bool {
    file.metadata()
        .is_ok_and(|metadata| metadata.len() <= limit)
}

fn read_bounded(file: &mut File, limit: u64) -> Result<String, ForgeRuntimeError> {
    file.seek(SeekFrom::Start(0))
        .map_err(|error| io_error(&error))?;
    let mut bytes = Vec::new();
    file.take(limit + 1)
        .read_to_end(&mut bytes)
        .map_err(|error| io_error(&error))?;
    if bytes.len() as u64 > limit {
        return Err(forge_error(
            ForgeFailureKind::Unknown,
            "Forge command output exceeded the configured limit",
        ));
    }
    Ok(String::from_utf8_lossy(&bytes).into_owned())
}

fn classify_command_error(family: CommandFamily, message: &str) -> ForgeFailureKind {
    let lower = message.to_ascii_lowercase();
    if lower.contains("not a git repository") {
        ForgeFailureKind::NotGitRepository
    } else if lower.contains("conflict") || lower.contains("unmerged") {
        ForgeFailureKind::MergeConflict
    } else if lower.contains("not logged")
        || lower.contains("authentication")
        || lower.contains("authenticate")
        || lower.contains("gh auth login")
        || lower.contains("http 401")
        || lower.contains("bad credentials")
    {
        ForgeFailureKind::Unauthenticated
    } else if lower.contains("forbidden")
        || lower.contains("resource not accessible")
        || lower.contains("permission")
        || lower.contains("access denied")
        || lower.contains("http 403")
    {
        ForgeFailureKind::Forbidden
    } else if matches!(family, CommandFamily::Forge)
        && (lower.contains("no pull requests found")
            || lower.contains("pull request not found")
            || lower.contains("could not resolve to a pullrequest"))
    {
        ForgeFailureKind::NotFound
    } else {
        ForgeFailureKind::Unknown
    }
}

fn parse_remote(remote: &str) -> Option<RemoteLocation> {
    let remote = remote.trim();
    let (host, path) = if let Some(rest) = remote.strip_prefix("https://") {
        rest.split_once('/')?
    } else if let Some(rest) = remote.strip_prefix("http://") {
        rest.split_once('/')?
    } else if let Some(rest) = remote.strip_prefix("ssh://") {
        let (_, location) = rest.rsplit_once('@').unwrap_or(("", rest));
        location.split_once('/')?
    } else if let Some((authority, path)) = remote.split_once(':') {
        (
            authority
                .rsplit_once('@')
                .map_or(authority, |(_, host)| host),
            path,
        )
    } else {
        return None;
    };
    let host = host.split(':').next()?.trim();
    let project_path = path.trim_matches('/').trim_end_matches(".git");
    if host.is_empty()
        || project_path.split('/').count() < 2
        || project_path.chars().any(char::is_control)
    {
        return None;
    }
    Some(RemoteLocation {
        host: host.to_ascii_lowercase(),
        project_path: project_path.to_owned(),
    })
}

fn resolve_base(
    cwd: &Path,
    requested: Option<&str>,
    head: &str,
) -> Result<String, ForgeRuntimeError> {
    if let Some(requested) = requested.map(str::trim).filter(|value| !value.is_empty()) {
        return Ok(normalize_branch(requested));
    }
    if let Some(reference) = git_optional(
        cwd,
        &[
            "symbolic-ref",
            "--quiet",
            "--short",
            "refs/remotes/origin/HEAD",
        ],
        READ_TIMEOUT,
    )? {
        return Ok(normalize_branch(&reference));
    }
    for candidate in ["main", "master"] {
        if candidate != head
            && git_optional(
                cwd,
                &["rev-parse", "--verify", &format!("{candidate}^{{commit}}")],
                READ_TIMEOUT,
            )?
            .is_some()
        {
            return Ok(candidate.to_owned());
        }
    }
    Err(forge_error(
        ForgeFailureKind::Unknown,
        "Unable to determine base branch for PR",
    ))
}

fn normalize_branch(value: &str) -> String {
    value
        .strip_prefix("refs/heads/")
        .or_else(|| value.strip_prefix("refs/remotes/origin/"))
        .or_else(|| value.strip_prefix("origin/"))
        .unwrap_or(value)
        .to_owned()
}

fn parse_search_items(
    output: &str,
    kind: ForgeSearchKind,
    context: &ForgeContext,
) -> Result<Vec<ForgeSearchItem>, ForgeRuntimeError> {
    let value = parse_json(output)?;
    let items = value
        .as_array()
        .ok_or_else(|| malformed("Forge search response is not an array"))?;
    items
        .iter()
        .map(|item| {
            Ok(ForgeSearchItem {
                kind,
                forge: Some("github".to_owned()),
                number: integer(item, "number")?,
                title: string(item, "title")?,
                url: string(item, "url")?,
                state: string(item, "state")?.to_ascii_lowercase(),
                body: optional_string(item, "body"),
                labels: item
                    .get("labels")
                    .and_then(Value::as_array)
                    .map(|labels| {
                        labels
                            .iter()
                            .filter_map(|label| {
                                label
                                    .as_str()
                                    .map(str::to_owned)
                                    .or_else(|| optional_string(label, "name"))
                            })
                            .collect()
                    })
                    .unwrap_or_default(),
                project_path: Some(context.project_path.clone()),
                base_ref_name: optional_string(item, "baseRefName"),
                head_ref_name: optional_string(item, "headRefName"),
                updated_at: optional_string(item, "updatedAt"),
            })
        })
        .collect()
}

fn parse_status(
    output: &str,
    fallback_head: &str,
    context: &ForgeContext,
) -> Result<Option<PullRequestStatus>, ForgeRuntimeError> {
    let value = parse_json(output)?;
    let Some(url) = optional_string(&value, "url") else {
        return Ok(None);
    };
    let Some(title) = optional_string(&value, "title") else {
        return Ok(None);
    };
    let merged = optional_string(&value, "mergedAt").is_some();
    let state = if merged {
        "merged".to_owned()
    } else {
        optional_string(&value, "state")
            .unwrap_or_default()
            .to_ascii_lowercase()
    };
    let checks = parse_checks(value.get("statusCheckRollup"));
    let checks_status = if checks.is_empty() {
        "none"
    } else if checks.iter().any(|check| check.status == "failure") {
        "failure"
    } else if checks.iter().any(|check| check.status == "pending") {
        "pending"
    } else {
        "success"
    };
    let (repo_owner, repo_name) = parse_repo_from_pull_url(&url)
        .map_or((None, None), |(owner, name)| (Some(owner), Some(name)));
    Ok(Some(PullRequestStatus {
        forge: "github".to_owned(),
        project_path: Some(context.project_path.clone()),
        number: value.get("number").and_then(Value::as_u64),
        url,
        title,
        state,
        base_ref_name: optional_string(&value, "baseRefName").unwrap_or_default(),
        head_ref_name: optional_string(&value, "headRefName")
            .unwrap_or_else(|| fallback_head.to_owned()),
        is_merged: merged,
        is_draft: value
            .get("isDraft")
            .and_then(Value::as_bool)
            .unwrap_or(false),
        mergeable: match value.get("mergeable").and_then(Value::as_str) {
            Some("MERGEABLE") => PullRequestMergeable::Mergeable,
            Some("CONFLICTING") => PullRequestMergeable::Conflicting,
            _ => PullRequestMergeable::Unknown,
        },
        checks,
        checks_status: checks_status.to_owned(),
        review_decision: value
            .get("reviewDecision")
            .and_then(Value::as_str)
            .and_then(|decision| match decision {
                "APPROVED" => Some("approved"),
                "CHANGES_REQUESTED" => Some("changes_requested"),
                "REVIEW_REQUIRED" => Some("pending"),
                _ => None,
            })
            .map(str::to_owned),
        repo_owner,
        repo_name,
        github: None,
        forge_specific: None,
    }))
}

fn parse_checks(value: Option<&Value>) -> Vec<PullRequestCheck> {
    let values = value
        .and_then(|value| {
            value
                .as_array()
                .or_else(|| value.pointer("/contexts/nodes").and_then(Value::as_array))
        })
        .cloned()
        .unwrap_or_default();
    let mut checks = BTreeMap::new();
    for value in values {
        let check = match value.get("__typename").and_then(Value::as_str) {
            Some("CheckRun") => {
                let Some(name) = optional_string(&value, "name") else {
                    continue;
                };
                PullRequestCheck {
                    name,
                    status: check_run_status(
                        value.get("status").and_then(Value::as_str),
                        value.get("conclusion").and_then(Value::as_str),
                    )
                    .to_owned(),
                    url: optional_string(&value, "detailsUrl"),
                    workflow: optional_string(&value, "workflowName"),
                    duration: check_duration(&value),
                    check_run_id: value.get("databaseId").and_then(Value::as_u64),
                    workflow_run_id: value
                        .pointer("/checkSuite/workflowRun/databaseId")
                        .and_then(Value::as_u64),
                    traits: None,
                }
            }
            Some("StatusContext") => {
                let Some(name) = optional_string(&value, "context") else {
                    continue;
                };
                PullRequestCheck {
                    name,
                    status: status_context_state(value.get("state").and_then(Value::as_str))
                        .to_owned(),
                    url: optional_string(&value, "targetUrl"),
                    workflow: None,
                    duration: None,
                    check_run_id: None,
                    workflow_run_id: None,
                    traits: None,
                }
            }
            _ => continue,
        };
        checks.insert(check.name.clone(), check);
    }
    checks.into_values().collect()
}

fn check_run_status(status: Option<&str>, conclusion: Option<&str>) -> &'static str {
    if status != Some("COMPLETED") {
        return "pending";
    }
    match conclusion {
        Some("SUCCESS") => "success",
        Some("FAILURE" | "TIMED_OUT" | "ACTION_REQUIRED") => "failure",
        Some("CANCELLED") => "cancelled",
        Some("SKIPPED" | "NEUTRAL") => "skipped",
        _ => "pending",
    }
}

fn status_context_state(state: Option<&str>) -> &'static str {
    match state {
        Some("SUCCESS") => "success",
        Some("FAILURE" | "ERROR") => "failure",
        _ => "pending",
    }
}

fn check_duration(value: &Value) -> Option<String> {
    let started = value
        .get("startedAt")
        .and_then(Value::as_str)
        .and_then(parse_time)?;
    let ended = value
        .get("completedAt")
        .and_then(Value::as_str)
        .and_then(parse_time)
        .unwrap_or_else(current_time_millis);
    let seconds = ended.checked_sub(started)?.checked_div(1000)?;
    if seconds < 0 {
        return None;
    }
    let hours = seconds / 3600;
    let minutes = seconds % 3600 / 60;
    let seconds = seconds % 60;
    let mut parts = Vec::new();
    if hours > 0 {
        parts.push(format!("{hours}h"));
    }
    if minutes > 0 {
        parts.push(format!("{minutes}m"));
    }
    if seconds > 0 || parts.is_empty() {
        parts.push(format!("{seconds}s"));
    }
    Some(parts.join(" "))
}

fn parse_timeline(
    output: &str,
    requested_number: u64,
) -> Result<PullRequestTimeline, ForgeRuntimeError> {
    let value = parse_json(output)?;
    let Some(pr) = value.pointer("/data/repository/pullRequest") else {
        return Ok(PullRequestTimeline {
            pr_number: requested_number,
            items: Vec::new(),
            truncated: false,
            error: Some(TimelineError {
                kind: TimelineErrorKind::NotFound,
                message: "Pull request not found".to_owned(),
            }),
            auth_state: ForgeAuthState::Authenticated,
        });
    };
    if pr.is_null() {
        return Ok(PullRequestTimeline {
            pr_number: requested_number,
            items: Vec::new(),
            truncated: false,
            error: Some(TimelineError {
                kind: TimelineErrorKind::NotFound,
                message: "Pull request not found".to_owned(),
            }),
            auth_state: ForgeAuthState::Authenticated,
        });
    }
    let mut items = Vec::new();
    for review in nodes(pr, "reviews") {
        if let Some(review_state) = review_state(review) {
            items.push(PullRequestTimelineItem::Review {
                id: optional_string(review, "id").unwrap_or_default(),
                author: author_field(review, "login").unwrap_or_else(|| "unknown".to_owned()),
                author_url: author_field(review, "url"),
                avatar_url: author_field(review, "avatarUrl"),
                body: optional_string(review, "body").unwrap_or_default(),
                created_at: optional_string(review, "submittedAt")
                    .as_deref()
                    .and_then(parse_time)
                    .unwrap_or(0),
                url: optional_string(review, "url").unwrap_or_default(),
                review_state,
            });
        }
    }
    let mut threaded_ids = Vec::new();
    for thread in nodes(pr, "reviewThreads") {
        for comment in nodes(thread, "comments") {
            let id = optional_string(comment, "id").unwrap_or_default();
            threaded_ids.push(id.clone());
            items.push(timeline_comment(
                comment,
                Some(TimelineCommentLocation {
                    path: optional_string(thread, "path").unwrap_or_default(),
                    line: thread.get("line").and_then(Value::as_u64),
                    start_line: thread.get("startLine").and_then(Value::as_u64),
                    thread_id: optional_string(thread, "id"),
                    is_resolved: thread.get("isResolved").and_then(Value::as_bool),
                    is_outdated: thread.get("isOutdated").and_then(Value::as_bool),
                }),
            ));
        }
    }
    for comment in nodes(pr, "comments") {
        let id = optional_string(comment, "id").unwrap_or_default();
        if !threaded_ids.contains(&id) {
            items.push(timeline_comment(comment, None));
        }
    }
    items.sort_by(|left, right| timeline_key(left).cmp(&timeline_key(right)));
    let truncated = page_has_next(pr, "reviews")
        || page_has_next(pr, "comments")
        || page_has_next(pr, "reviewThreads")
        || nodes(pr, "reviewThreads")
            .iter()
            .any(|thread| page_has_next(thread, "comments"));
    Ok(PullRequestTimeline {
        pr_number: pr
            .get("number")
            .and_then(Value::as_u64)
            .unwrap_or(requested_number),
        items,
        truncated,
        error: None,
        auth_state: ForgeAuthState::Authenticated,
    })
}

fn nodes<'a>(value: &'a Value, field: &str) -> Vec<&'a Value> {
    value
        .get(field)
        .and_then(|value| value.get("nodes"))
        .and_then(Value::as_array)
        .map(|values| values.iter().collect())
        .unwrap_or_default()
}

fn page_has_next(value: &Value, field: &str) -> bool {
    value
        .get(field)
        .and_then(|value| value.pointer("/pageInfo/hasNextPage"))
        .and_then(Value::as_bool)
        .unwrap_or(false)
}

fn review_state(value: &Value) -> Option<TimelineReviewState> {
    match value.get("state").and_then(Value::as_str) {
        Some("APPROVED") => Some(TimelineReviewState::Approved),
        Some("CHANGES_REQUESTED") => Some(TimelineReviewState::ChangesRequested),
        Some("COMMENTED") => Some(TimelineReviewState::Commented),
        _ if optional_string(value, "body").is_some_and(|body| !body.trim().is_empty()) => {
            Some(TimelineReviewState::Commented)
        }
        _ => None,
    }
}

fn timeline_comment(
    value: &Value,
    location: Option<TimelineCommentLocation>,
) -> PullRequestTimelineItem {
    PullRequestTimelineItem::Comment {
        id: optional_string(value, "id").unwrap_or_default(),
        author: author_field(value, "login").unwrap_or_else(|| "unknown".to_owned()),
        author_url: author_field(value, "url"),
        avatar_url: author_field(value, "avatarUrl"),
        body: optional_string(value, "body").unwrap_or_default(),
        created_at: optional_string(value, "createdAt")
            .as_deref()
            .and_then(parse_time)
            .unwrap_or(0),
        url: optional_string(value, "url").unwrap_or_default(),
        review_id: value
            .pointer("/pullRequestReview/id")
            .and_then(Value::as_str)
            .map(str::to_owned),
        thread_id: None,
        thread_is_resolved: None,
        location,
    }
}

fn timeline_key(item: &PullRequestTimelineItem) -> (i64, &str) {
    match item {
        PullRequestTimelineItem::Review { created_at, id, .. }
        | PullRequestTimelineItem::Comment { created_at, id, .. } => (*created_at, id),
    }
}

fn author_field(value: &Value, field: &str) -> Option<String> {
    value
        .get("author")
        .and_then(|author| optional_string(author, field))
}

fn parse_annotations(value: &Value) -> Result<Vec<CheckAnnotation>, ForgeRuntimeError> {
    let values = value
        .as_array()
        .ok_or_else(|| malformed("Check annotations response is not an array"))?;
    Ok(values
        .iter()
        .map(|annotation| CheckAnnotation {
            path: optional_string(annotation, "path"),
            start_line: annotation.get("start_line").and_then(Value::as_u64),
            end_line: annotation.get("end_line").and_then(Value::as_u64),
            annotation_level: optional_string(annotation, "annotation_level"),
            message: optional_string(annotation, "message"),
            title: optional_string(annotation, "title"),
            raw_details: optional_string(annotation, "raw_details"),
        })
        .collect())
}

fn parse_failed_jobs(value: &Value) -> Result<(Vec<CheckFailedJob>, bool), ForgeRuntimeError> {
    let values = value
        .get("jobs")
        .and_then(Value::as_array)
        .ok_or_else(|| malformed("Workflow jobs response is missing jobs"))?;
    let failed = values
        .iter()
        .filter(|job| {
            matches!(
                job.get("conclusion").and_then(Value::as_str),
                Some("failure" | "timed_out" | "action_required" | "cancelled")
            )
        })
        .collect::<Vec<_>>();
    let truncated = values.len() >= CHECK_JOB_LIMIT || failed.len() > FAILED_JOB_LIMIT;
    let jobs = failed
        .into_iter()
        .take(FAILED_JOB_LIMIT)
        .map(|job| {
            Ok(CheckFailedJob {
                job_id: integer(job, "id")?,
                name: string(job, "name")?,
                status: optional_string(job, "status"),
                conclusion: optional_string(job, "conclusion"),
                url: optional_string(job, "html_url"),
                log_tail: None,
                log_truncated: None,
            })
        })
        .collect::<Result<Vec<_>, ForgeRuntimeError>>()?;
    Ok((jobs, truncated))
}

fn current_number(
    status: &PullRequestStatusRead,
    operation: &str,
) -> Result<u64, ForgeRuntimeError> {
    status
        .status
        .as_ref()
        .and_then(|status| status.number)
        .ok_or_else(|| {
            forge_error(
                ForgeFailureKind::Unknown,
                format!("Unable to determine current change request number for {operation}"),
            )
        })
}

const fn merge_method_flag(method: PullRequestMergeMethod) -> &'static str {
    match method {
        PullRequestMergeMethod::Merge => "--merge",
        PullRequestMergeMethod::Squash => "--squash",
        PullRequestMergeMethod::Rebase => "--rebase",
    }
}

fn parse_repo_from_pull_url(url: &str) -> Option<(String, String)> {
    let (_, rest) = url.split_once("://")?;
    let (_, path) = rest.split_once('/')?;
    let mut parts = path.split('/');
    let owner = parts.next()?;
    let name = parts.next()?;
    (parts.next()? == "pull").then(|| (owner.to_owned(), name.to_owned()))
}

fn parse_time(value: &str) -> Option<i64> {
    DateTime::parse_from_rfc3339(value)
        .ok()
        .map(|time| time.timestamp_millis())
}

fn current_time_millis() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .ok()
        .and_then(|duration| i64::try_from(duration.as_millis()).ok())
        .unwrap_or(0)
}

fn parse_json(value: &str) -> Result<Value, ForgeRuntimeError> {
    serde_json::from_str(value).map_err(|error| malformed(format!("Invalid forge JSON: {error}")))
}

fn string(value: &Value, field: &str) -> Result<String, ForgeRuntimeError> {
    optional_string(value, field)
        .ok_or_else(|| malformed(format!("Forge response is missing {field}")))
}

fn optional_string(value: &Value, field: &str) -> Option<String> {
    value.get(field).and_then(Value::as_str).map(str::to_owned)
}

fn integer(value: &Value, field: &str) -> Result<u64, ForgeRuntimeError> {
    value
        .get(field)
        .and_then(Value::as_u64)
        .ok_or_else(|| malformed(format!("Forge response is missing {field}")))
}

fn strings(values: &[&str]) -> Vec<String> {
    values.iter().map(|value| (*value).to_owned()).collect()
}

fn malformed(message: impl Into<String>) -> ForgeRuntimeError {
    forge_error(ForgeFailureKind::Unknown, message)
}

fn io_error(error: &std::io::Error) -> ForgeRuntimeError {
    forge_error(ForgeFailureKind::Unknown, error.to_string())
}

fn forge_error(kind: ForgeFailureKind, message: impl Into<String>) -> ForgeRuntimeError {
    ForgeRuntimeError {
        kind,
        message: message.into(),
    }
}

#[cfg(test)]
mod tests;
