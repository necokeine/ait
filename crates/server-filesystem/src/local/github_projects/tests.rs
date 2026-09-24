use std::process::Command;

use serde_json::json;

use super::*;

#[test]
fn parses_repository_search_and_list_with_paseo_clone_urls() {
    let listed = json!([{
        "id":" R_recent ", "name":" paseo ", "nameWithOwner":" getpaseo/paseo ",
        "description":null, "isPrivate":false, "updatedAt":"2026-07-15T12:00:00Z",
        "sshUrl":"git@github.com:getpaseo/paseo.git",
        "url":"https://github.com/getpaseo/paseo"
    }]);
    let searched = json!([{
        "id":42, "name":"private-repo", "fullName":"octo/private-repo",
        "description":"Private project", "isPrivate":true,
        "updatedAt":"2026-07-14T08:00:00Z", "url":"https://github.com/octo/private-repo"
    }]);

    let listed = parse_repositories(&listed.to_string(), true, "ssh").expect("list");
    let searched = parse_repositories(&searched.to_string(), false, "https").expect("search");

    assert_eq!(listed[0].id, "R_recent");
    assert_eq!(listed[0].clone_url, "git@github.com:getpaseo/paseo.git");
    assert_eq!(searched[0].id, "42");
    assert_eq!(searched[0].visibility, GithubRepositoryVisibility::Private);
    assert_eq!(
        searched[0].clone_url,
        "https://github.com/octo/private-repo"
    );
}

#[test]
fn malformed_repository_rows_fail_closed() {
    let rows = json!([{
        "id":"R_1", "name":"repo", "fullName":"owner/../../repo",
        "isPrivate":false, "updatedAt":"2026-09-23", "url":"https://github.com/owner/repo"
    }]);
    assert_eq!(
        parse_repositories(&rows.to_string(), false, "https"),
        Err(GithubProjectsError::SearchFailed)
    );
}

#[test]
fn missing_github_cli_has_distinct_unavailable_error() {
    let home = tempfile::tempdir().expect("home");
    let runtime = LocalGithubProjects::with_executables(
        home.path().join("missing-gh"),
        PathBuf::from("git"),
        home.path().to_path_buf(),
    );

    assert_eq!(
        runtime.search_repositories("repo", 5),
        Err(GithubProjectsError::CliMissing)
    );
}

#[cfg(unix)]
#[test]
fn cli_search_uses_recent_list_for_blank_query_and_typed_search_otherwise() {
    use std::os::unix::fs::PermissionsExt;

    let home = tempfile::tempdir().expect("home");
    let executable = home.path().join("gh");
    std::fs::write(&executable, r#"#!/bin/sh
case "$1" in
  repo) printf '[{"id":"R_1","name":"repo","nameWithOwner":"owner/repo","description":null,"isPrivate":false,"updatedAt":"2026-09-23","sshUrl":"git@github.com:owner/repo.git","url":"https://github.com/owner/repo"}]' ;;
  search) printf '[{"id":7,"name":"other","fullName":"owner/other","description":"x","isPrivate":true,"updatedAt":"2026-09-23","url":"https://github.com/owner/other"}]' ;;
  config) printf 'ssh\n' ;;
esac
"#).expect("script");
    let mut permissions = std::fs::metadata(&executable)
        .expect("metadata")
        .permissions();
    permissions.set_mode(0o700);
    std::fs::set_permissions(&executable, permissions).expect("permissions");
    let runtime = LocalGithubProjects::with_executables(
        executable,
        PathBuf::from("git"),
        home.path().to_path_buf(),
    );

    let listed = runtime.search_repositories("  ", 8).expect("list");
    let searched = runtime
        .search_repositories(" private project ", 5)
        .expect("search");

    assert_eq!(listed[0].clone_url, "git@github.com:owner/repo.git");
    assert_eq!(searched[0].clone_url, "git@github.com:owner/other.git");
    assert_eq!(searched[0].visibility, GithubRepositoryVisibility::Private);
}

#[test]
fn clone_stages_local_git_repository_and_never_replaces_target() {
    let root = tempfile::tempdir().expect("root");
    let source = root.path().join("source");
    std::fs::create_dir(&source).expect("source");
    assert!(
        Command::new("git")
            .args(["init", "-q"])
            .current_dir(&source)
            .status()
            .expect("init")
            .success()
    );
    assert!(
        Command::new("git")
            .args([
                "-c",
                "user.name=Test",
                "-c",
                "user.email=test@example.invalid",
                "commit",
                "-q",
                "--allow-empty",
                "-m",
                "initial"
            ])
            .current_dir(&source)
            .status()
            .expect("commit")
            .success()
    );
    let parent = root.path().join("checkouts");
    let runtime = LocalGithubProjects::with_executables(
        PathBuf::from("gh"),
        PathBuf::from("git"),
        root.path().to_path_buf(),
    );

    let checkout = runtime
        .clone_repository(
            source.to_str().expect("source text"),
            parent.to_str().expect("parent text"),
            "copied",
        )
        .expect("clone");

    assert!(PathBuf::from(&checkout).join(".git").is_dir());
    assert_eq!(
        runtime.clone_repository(
            source.to_str().expect("source text"),
            parent.to_str().expect("parent text"),
            "copied"
        ),
        Err(GithubProjectsError::TargetExists)
    );
    assert_eq!(std::fs::read_dir(&parent).expect("entries").count(), 1);
}

#[test]
fn failed_clone_cleans_staging_directory() {
    let root = tempfile::tempdir().expect("root");
    let parent = root.path().join("checkouts");
    let runtime = LocalGithubProjects::with_executables(
        PathBuf::from("gh"),
        PathBuf::from("git"),
        root.path().to_path_buf(),
    );

    let result = runtime.clone_repository(
        "/definitely/missing/repository",
        parent.to_str().expect("parent text"),
        "uncloned",
    );

    assert_eq!(result, Err(GithubProjectsError::CloneFailed));
    assert_eq!(std::fs::read_dir(parent).expect("entries").count(), 0);
}
