use std::fs;
use std::path::Path;
use std::process::Command;

#[cfg(unix)]
use std::os::unix::fs::PermissionsExt;

use crate::ports::forge::{
    ForgeAuthState, ForgeRuntime, ForgeSearchKind, PullRequestMergeMethod, PullRequestTimelineItem,
};

use super::*;

#[cfg(unix)]
mod commands;
mod paseo;

#[cfg(unix)]
#[test]
fn github_cli_adapter_parses_search_status_timeline_checks_and_mutations() {
    let root = tempfile::tempdir().unwrap();
    let repository = root.path().join("repo");
    create_repository(&repository, true);
    let log = root.path().join("gh.log");
    let executable = fake_gh(root.path(), &log);
    let forge = LocalForge::with_executable(executable);

    let search = forge
        .search(
            repository.to_str().unwrap(),
            "fix",
            20,
            &[ForgeSearchKind::Issue, ForgeSearchKind::ChangeRequest],
        )
        .unwrap();
    assert_eq!(search.auth_state, ForgeAuthState::Authenticated);
    assert_eq!(search.items.len(), 2);
    assert_eq!(search.items[0].number, 2);
    assert_eq!(search.items[1].labels, ["bug"]);

    let status = forge
        .current_pull_request_status(repository.to_str().unwrap())
        .unwrap();
    let status = status.status.unwrap();
    assert_eq!(status.number, Some(2));
    assert_eq!(status.repo_owner.as_deref(), Some("acme"));
    assert_eq!(status.checks[0].status, "failure");
    assert_eq!(status.checks_status, "failure");

    forge
        .merge_current_pull_request(repository.to_str().unwrap(), PullRequestMergeMethod::Squash)
        .unwrap();
    forge
        .set_current_pull_request_auto_merge(
            repository.to_str().unwrap(),
            true,
            Some(PullRequestMergeMethod::Rebase),
        )
        .unwrap();
    forge
        .set_current_pull_request_auto_merge(repository.to_str().unwrap(), false, None)
        .unwrap();

    let timeline = forge
        .pull_request_timeline(repository.to_str().unwrap(), 2, "acme", "app")
        .unwrap();
    assert_eq!(timeline.items.len(), 3);
    assert!(timeline.truncated);
    assert!(matches!(
        timeline.items[0],
        PullRequestTimelineItem::Comment { .. }
    ));
    assert!(matches!(
        timeline.items[2],
        PullRequestTimelineItem::Review { .. }
    ));

    let details = forge
        .check_details(
            repository.to_str().unwrap(),
            crate::ports::forge::CheckDetailsQuery {
                repo_owner: Some("acme"),
                repo_name: Some("app"),
                check_run_id: Some(9),
                workflow_run_id: None,
                change_request_number: Some(2),
            },
        )
        .unwrap();
    assert_eq!(details.workflow_run_id, Some(44));
    assert_eq!(details.annotations.len(), 1);
    assert_eq!(details.failed_jobs.len(), 1);
    assert_eq!(details.failed_jobs[0].job_id, 70);

    let calls = fs::read_to_string(log).unwrap();
    assert!(calls.contains("pr merge 2 --squash"), "{calls}");
    assert!(calls.contains("pr merge 2 --auto --rebase"), "{calls}");
    assert!(calls.contains("pr merge 2 --disable-auto"), "{calls}");
}

#[test]
fn search_reports_no_remote_without_invoking_a_forge_cli() {
    let root = tempfile::tempdir().unwrap();
    let repository = root.path().join("repo");
    create_repository(&repository, false);
    let forge = LocalForge::with_executable(root.path().join("missing-gh"));
    let search = forge
        .search(
            repository.to_str().unwrap(),
            "",
            20,
            &[ForgeSearchKind::Issue],
        )
        .unwrap();
    assert_eq!(search.auth_state, ForgeAuthState::NoRemote);
    assert!(search.items.is_empty());
}

#[cfg(unix)]
#[test]
fn missing_cli_and_authentication_failures_have_distinct_states() {
    let root = tempfile::tempdir().unwrap();
    let repository = root.path().join("repo");
    create_repository(&repository, true);
    let missing = LocalForge::with_executable(root.path().join("missing-gh"));
    assert_eq!(
        missing
            .search(
                repository.to_str().unwrap(),
                "",
                20,
                &[ForgeSearchKind::Issue],
            )
            .unwrap()
            .auth_state,
        ForgeAuthState::CliMissing
    );

    let executable = root.path().join("auth-gh");
    fs::write(
        &executable,
        "#!/bin/sh\necho 'authentication required; run gh auth login' >&2\nexit 1\n",
    )
    .unwrap();
    fs::set_permissions(&executable, fs::Permissions::from_mode(0o700)).unwrap();
    let auth = LocalForge::with_executable(executable);
    assert_eq!(
        auth.search(
            repository.to_str().unwrap(),
            "",
            20,
            &[ForgeSearchKind::Issue],
        )
        .unwrap()
        .auth_state,
        ForgeAuthState::Unauthenticated
    );
}

#[test]
fn parses_supported_remote_spellings() {
    assert_eq!(
        parse_remote("git@github.com:acme/app.git")
            .unwrap()
            .project_path,
        "acme/app"
    );
    assert_eq!(
        parse_remote("https://github.example/acme/app.git")
            .unwrap()
            .host,
        "github.example"
    );
    assert!(parse_remote("/tmp/repo.git").is_none());
}

fn create_repository(path: &Path, remote: bool) {
    fs::create_dir_all(path).unwrap();
    run(path, &["init", "-b", "main"]);
    run(path, &["config", "user.email", "server@example.invalid"]);
    run(path, &["config", "user.name", "Server Test"]);
    fs::write(path.join("tracked.txt"), "base\n").unwrap();
    run(path, &["add", "."]);
    run(path, &["commit", "-m", "base"]);
    run(path, &["checkout", "-b", "feature"]);
    if remote {
        run(
            path,
            &["remote", "add", "origin", "git@github.com:acme/app.git"],
        );
    }
}

fn run(path: &Path, arguments: &[&str]) {
    let output = Command::new("git")
        .args(arguments)
        .current_dir(path)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "git {arguments:?}: {}",
        String::from_utf8_lossy(&output.stderr)
    );
}

#[cfg(unix)]
fn fake_gh(directory: &Path, log: &Path) -> PathBuf {
    let executable = directory.join("gh");
    let script = format!(
        r#"#!/bin/sh
echo "$*" >> '{}'
if [ "$1 $2" = "issue list" ]; then
  echo '[{{"number":1,"title":"Issue","url":"https://github.com/acme/app/issues/1","state":"OPEN","body":"body","labels":[{{"name":"bug"}}],"updatedAt":"2026-01-01T00:00:00Z"}}]'
elif [ "$1 $2" = "pr list" ]; then
  echo '[{{"number":2,"title":"PR","url":"https://github.com/acme/app/pull/2","state":"OPEN","body":null,"labels":[],"baseRefName":"main","headRefName":"feature","updatedAt":"2026-02-01T00:00:00Z"}}]'
elif [ "$1 $2" = "pr view" ]; then
  echo '{{"number":2,"url":"https://github.com/acme/app/pull/2","title":"PR","state":"OPEN","isDraft":false,"baseRefName":"main","headRefName":"feature","mergedAt":null,"reviewDecision":"CHANGES_REQUESTED","mergeable":"CONFLICTING","statusCheckRollup":[{{"__typename":"CheckRun","name":"tests","status":"COMPLETED","conclusion":"FAILURE","detailsUrl":"https://example/check","databaseId":9,"checkSuite":{{"workflowRun":{{"databaseId":44}}}}}}]}}'
elif [ "$1 $2" = "pr merge" ]; then
  exit 0
elif [ "$1 $2" = "api graphql" ]; then
  echo '{{"data":{{"repository":{{"pullRequest":{{"number":2,"reviews":{{"nodes":[{{"id":"R1","state":"APPROVED","body":"ship","url":"https://example/review","submittedAt":"2026-01-03T00:00:00Z","author":{{"login":"reviewer","url":null,"avatarUrl":null}}}}],"pageInfo":{{"hasNextPage":false}}}},"comments":{{"nodes":[{{"id":"C1","body":"general","url":"https://example/comment","createdAt":"2026-01-01T00:00:00Z","author":{{"login":"commenter","url":null,"avatarUrl":null}}}}],"pageInfo":{{"hasNextPage":true}}}},"reviewThreads":{{"nodes":[{{"id":"T1","path":"src/lib.rs","line":5,"startLine":4,"isResolved":false,"isOutdated":false,"comments":{{"nodes":[{{"id":"C2","body":"inline","url":"https://example/inline","createdAt":"2026-01-02T00:00:00Z","author":{{"login":"inline","url":null,"avatarUrl":null}},"pullRequestReview":{{"id":"R1"}}}}],"pageInfo":{{"hasNextPage":false}}}}}}],"pageInfo":{{"hasNextPage":false}}}}}}}}}}}}'
elif [ "$1" = "api" ] && [ "$2" = "repos/acme/app/check-runs/9" ]; then
  echo '{{"id":9,"name":"tests","status":"completed","conclusion":"failure","html_url":"https://example/check","details_url":"https://example/details","check_suite":{{"workflow_run":{{"id":44}}}},"output":{{"title":"failed","summary":"summary","text":"text"}}}}'
elif [ "$1" = "api" ] && [ "$2" = "repos/acme/app/check-runs/9/annotations" ]; then
  echo '[{{"path":"src/lib.rs","start_line":4,"end_line":5,"annotation_level":"failure","message":"broken","title":"lint","raw_details":"details"}}]'
elif [ "$1" = "api" ] && [ "$2" = "repos/acme/app/actions/runs/44/jobs" ]; then
  echo '{{"jobs":[{{"id":70,"name":"test","status":"completed","conclusion":"failure","html_url":"https://example/job"}}]}}'
else
  echo "unexpected: $*" >&2
  exit 2
fi
"#,
        log.display()
    );
    fs::write(&executable, script).unwrap();
    fs::set_permissions(&executable, fs::Permissions::from_mode(0o700)).unwrap();
    executable
}
