//! Offline subprocess counterparts of Paseo forge CLI and GitHub service tests.
use super::*;
use serde_json::json;

struct Fixture {
    root: tempfile::TempDir,
    repository: PathBuf,
    forge: LocalForge,
}

impl Fixture {
    fn new(body: &str) -> Self {
        let root = tempfile::tempdir().unwrap();
        let repository = root.path().join("repo");
        create_repository(&repository, true);
        let executable = root.path().join("gh");
        fs::write(&executable, format!("#!/bin/sh\nroot=${{0%/*}}\nprintf '%s\\n' \"$@\" >> \"$root/calls\"\nprintf '%s|%s|%s\\n' \"$GH_HOST\" \"$GH_PROMPT_DISABLED\" \"$GIT_TERMINAL_PROMPT\" >> \"$root/environment\"\n{body}\n")).unwrap();
        fs::set_permissions(&executable, fs::Permissions::from_mode(0o700)).unwrap();
        Self {
            root,
            repository,
            forge: LocalForge::with_executable(executable),
        }
    }

    fn cwd(&self) -> &str {
        self.repository.to_str().unwrap()
    }

    fn payload(&self, name: &str, value: &Value) {
        fs::write(self.root.path().join(name), value.to_string()).unwrap();
    }

    fn calls(&self) -> String {
        fs::read_to_string(self.root.path().join("calls")).unwrap()
    }
}

#[test]
fn enterprise_gh_calls_route_to_remote_host_with_interactive_prompts_disabled() {
    let fixture = Fixture::new("printf '[]'");
    run(
        &fixture.repository,
        &[
            "remote",
            "set-url",
            "origin",
            "https://github.enterprise/acme/app.git",
        ],
    );
    fixture
        .forge
        .search(fixture.cwd(), "", 1, &[ForgeSearchKind::Issue])
        .unwrap();
    assert_eq!(
        fs::read_to_string(fixture.root.path().join("environment")).unwrap(),
        "github.enterprise|1|0\n"
    );
}

#[test]
fn search_only_requests_pull_requests_when_issues_are_excluded() {
    let fixture = Fixture::new("printf '[]'");
    fixture
        .forge
        .search(fixture.cwd(), "bug", 8, &[ForgeSearchKind::ChangeRequest])
        .unwrap();
    let calls = fixture.calls();
    assert!(calls.starts_with("pr\nlist\n--search\nbug\n"), "{calls}");
    assert!(!calls.contains("issue\n"));
    assert!(calls.ends_with("--limit\n8\n"));
}

#[test]
fn search_merges_kinds_by_recency_before_applying_the_shared_result_limit() {
    let fixture = Fixture::new("cat \"$root/$1.json\"");
    fixture.payload("issue.json", &json!([
        {"number":1,"title":"older","url":"https://example/1","state":"OPEN","updatedAt":"2026-01-01"},
        {"number":3,"title":"newest","url":"https://example/3","state":"OPEN","updatedAt":"2026-03-01"}]));
    fixture.payload("pr.json", &json!([{ "number":2,"title":"middle","url":"https://example/2","state":"OPEN","updatedAt":"2026-02-01"}]));
    let result = fixture
        .forge
        .search(
            fixture.cwd(),
            "",
            2,
            &[ForgeSearchKind::Issue, ForgeSearchKind::ChangeRequest],
        )
        .unwrap();
    assert_eq!(
        result
            .items
            .iter()
            .map(|item| item.number)
            .collect::<Vec<_>>(),
        [3, 2]
    );
    assert_eq!(result.items[1].kind, ForgeSearchKind::ChangeRequest);
}

#[test]
fn partial_search_command_failure_preserves_successful_results() {
    let fixture = Fixture::new(
        "if [ \"$1\" = issue ]; then echo 'HTTP 403 forbidden' >&2; exit 1; fi\ncat \"$root/pr.json\"",
    );
    fixture.payload(
        "pr.json",
        &json!([{ "number":2,"title":"available","url":"https://example/2","state":"OPEN"}]),
    );
    let result = fixture
        .forge
        .search(
            fixture.cwd(),
            "",
            2,
            &[ForgeSearchKind::Issue, ForgeSearchKind::ChangeRequest],
        )
        .unwrap();
    assert_eq!(result.auth_state, ForgeAuthState::Authenticated);
    assert_eq!(result.items.len(), 1);
    assert_eq!(result.items[0].number, 2);
}

#[test]
fn malformed_search_json_fails_instead_of_returning_a_successful_empty_result() {
    let fixture = Fixture::new("printf '{invalid'");
    let error = fixture
        .forge
        .search(fixture.cwd(), "", 2, &[ForgeSearchKind::Issue])
        .unwrap_err();
    assert_eq!(error.kind, ForgeFailureKind::Unknown);
}

#[test]
fn fork_timeline_uses_the_explicit_parent_repository_identity() {
    let fixture =
        Fixture::new("printf '{\"data\":{\"repository\":{\"pullRequest\":{\"number\":42}}}}'");
    let result = fixture
        .forge
        .pull_request_timeline(fixture.cwd(), 42, "parent", "upstream")
        .unwrap();
    assert_eq!(result.pr_number, 42);
    let calls = fixture.calls();
    assert!(
        calls.contains("owner=parent\n-F\nname=upstream\n-F\nnumber=42\n"),
        "{calls}"
    );
    assert!(calls.contains("pullRequestReview"));
}

#[test]
fn timeline_distinguishes_missing_pull_requests_forbidden_access_and_unrelated_failures() {
    let fixture = Fixture::new("cat \"$root/error\" >&2\nexit 1");
    for (message, expected) in [
        (
            "Could not resolve to a PullRequest",
            Some(TimelineErrorKind::NotFound),
        ),
        (
            "HTTP 403 resource not accessible",
            Some(TimelineErrorKind::Forbidden),
        ),
        ("unrelated repository not found", None),
    ] {
        fs::write(fixture.root.path().join("error"), message).unwrap();
        let result = fixture
            .forge
            .pull_request_timeline(fixture.cwd(), 42, "parent", "upstream");
        if let Some(expected) = expected {
            let result = result.unwrap();
            assert!(result.items.is_empty());
            assert_eq!(result.error.unwrap().kind, expected);
        } else {
            assert_eq!(result.unwrap_err().kind, ForgeFailureKind::Unknown);
        }
    }
}

#[test]
fn status_distinguishes_an_unmatched_branch_from_authentication_failure() {
    let fixture = Fixture::new("cat \"$root/error\" >&2\nexit 1");
    for (message, expected) in [
        (
            "no pull requests found for branch",
            ForgeAuthState::Authenticated,
        ),
        (
            "authentication required; gh auth login",
            ForgeAuthState::Unauthenticated,
        ),
    ] {
        fs::write(fixture.root.path().join("error"), message).unwrap();
        let result = fixture
            .forge
            .current_pull_request_status(fixture.cwd())
            .unwrap();
        assert!(result.status.is_none());
        assert_eq!(result.auth_state, expected);
    }
}

#[test]
fn bounded_command_kills_a_hung_process_at_the_requested_deadline() {
    let mut command = Command::new("/bin/sh");
    command.args(["-c", "while :; do :; done"]);
    let started = Instant::now();
    let error = run_command(command, Duration::from_millis(30), CommandFamily::Forge).unwrap_err();
    assert_eq!(error.kind, ForgeFailureKind::Unknown);
    assert!(error.message.contains("timed out"));
    assert!(started.elapsed() < Duration::from_secs(5));
}

#[test]
fn bounded_command_accepts_the_output_limit_and_rejects_one_extra_byte() {
    for (size, succeeds) in [(OUTPUT_LIMIT, true), (OUTPUT_LIMIT + 1, false)] {
        let mut command = Command::new("/bin/sh");
        command.args([
            "-c",
            "head -c \"$1\" /dev/zero",
            "fixture",
            &size.to_string(),
        ]);
        let result = run_command(command, Duration::from_secs(5), CommandFamily::Forge);
        if succeeds {
            assert_eq!(result.unwrap().stdout.len() as u64, OUTPUT_LIMIT);
        } else {
            assert!(result.unwrap_err().message.contains("output exceeded"));
        }
    }
}
