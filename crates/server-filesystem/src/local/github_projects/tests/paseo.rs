//! GitHub repository discovery contracts and bounded Rust adapter edge cases.
use super::*;
use std::os::unix::fs::PermissionsExt;

struct Fixture {
    root: tempfile::TempDir,
    runtime: LocalGithubProjects,
}

impl Fixture {
    fn new(config: &str) -> Self {
        let root = tempfile::tempdir().unwrap();
        let executable = root.path().join("gh");
        std::fs::write(&executable,format!("#!/bin/sh\nroot=${{0%/*}}\nprintf '%s\\n' \"$@\" >> \"$root/calls\"\nif [ \"$1\" = config ]; then\n{config}\nelse\ncat \"$root/repositories\"\nfi\n")).unwrap();
        std::fs::set_permissions(&executable, std::fs::Permissions::from_mode(0o700)).unwrap();
        std::fs::write(
            root.path().join("repositories"),
            json!([repository()]).to_string(),
        )
        .unwrap();
        let runtime = LocalGithubProjects::with_executables(
            executable,
            PathBuf::from("git"),
            root.path().to_path_buf(),
        );
        Self { root, runtime }
    }
}

fn repository() -> Value {
    json!({"id":42,"name":"repo","nameWithOwner":"owner/repo","fullName":"owner/repo",
        "isPrivate":false,"updatedAt":"2026-01-01T00:00:00Z","url":"https://github.com/owner/repo",
        "sshUrl":"git@github.com:owner/repo.git","description":null})
}

#[test]
fn typed_repository_query_is_trimmed_but_remains_one_literal_cli_argument() {
    let fixture = Fixture::new("printf https");
    let query = "private project; $(touch SHOULD_NOT_EXIST)";
    let result = fixture
        .runtime
        .search_repositories(&format!("  {query}  "), 5)
        .unwrap();
    assert_eq!(result.len(), 1);
    let calls = std::fs::read_to_string(fixture.root.path().join("calls")).unwrap();
    assert!(
        calls.starts_with(&format!("search\nrepos\n{query}\n--json\n")),
        "{calls}"
    );
    assert!(calls.contains("--sort\nupdated\n--order\ndesc\n--limit\n5\n"));
    assert!(!fixture.root.path().join("SHOULD_NOT_EXIST").exists());
}

#[test]
fn unavailable_git_protocol_configuration_falls_back_to_https_clone_identity() {
    let fixture = Fixture::new("echo 'config key is unset' >&2\nexit 1");
    let result = fixture.runtime.search_repositories("repo", 5).unwrap();
    assert_eq!(result[0].clone_url, "https://github.com/owner/repo");
}

#[test]
fn ssh_protocol_is_case_insensitive_and_search_builds_the_owner_scoped_clone_url() {
    let fixture = Fixture::new("printf ' SSH \\n'");
    let result = fixture.runtime.search_repositories("repo", 5).unwrap();
    assert_eq!(result[0].clone_url, "git@github.com:owner/repo.git");
    assert_eq!(result[0].id, "42");
    assert_eq!(result[0].visibility, GithubRepositoryVisibility::Public);
}

#[test]
fn authentication_failure_during_protocol_lookup_is_not_hidden_by_https_fallback() {
    let fixture = Fixture::new("echo 'HTTP 401 bad credentials' >&2\nexit 1");
    assert_eq!(
        fixture.runtime.search_repositories("repo", 5),
        Err(GithubProjectsError::Unauthenticated)
    );
}

#[test]
fn invalid_repository_limits_are_rejected_before_launching_the_cli() {
    let fixture = Fixture::new("printf https");
    for limit in [0, 51, usize::MAX] {
        assert_eq!(
            fixture.runtime.search_repositories("repo", limit),
            Err(GithubProjectsError::SearchFailed)
        );
    }
    assert!(!fixture.root.path().join("calls").exists());
}

#[test]
fn listed_repository_ssh_identity_must_match_the_normalized_owner_and_name() {
    let mut row = repository();
    row["sshUrl"] = json!("git@github.com:other/repo.git");
    assert_eq!(
        parse_repositories(&json!([row]).to_string(), true, "ssh"),
        Err(GithubProjectsError::SearchFailed)
    );
}

#[test]
fn one_malformed_repository_aborts_the_whole_result_instead_of_returning_a_prefix() {
    let mut invalid = repository();
    invalid["url"] = json!("https://unrelated.example/owner/repo");
    assert_eq!(
        parse_repositories(&json!([repository(), invalid]).to_string(), false, "https"),
        Err(GithubProjectsError::SearchFailed)
    );
}

#[test]
fn repository_identity_requires_nonempty_names_ids_dates_and_boolean_visibility() {
    for (field, value) in [
        ("id", json!(" ")),
        ("name", Value::Null),
        ("updatedAt", json!(" ")),
        ("isPrivate", json!("false")),
        ("fullName", json!("owner/other")),
    ] {
        let mut row = repository();
        row[field] = value;
        assert_eq!(
            parse_repositories(&json!([row]).to_string(), false, "https"),
            Err(GithubProjectsError::SearchFailed),
            "{field}"
        );
    }
}
