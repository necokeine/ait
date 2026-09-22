use std::ffi::OsString;
use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::Path;
use std::process::Command;

use serde_json::json;

use super::transport::{connect, request};
use super::{ready, start_with_path, terminate};

#[tokio::test]
async fn binary_serves_all_forge_and_pull_request_methods() {
    let root = tempfile::tempdir().unwrap();
    let repository = root.path().join("repository");
    let remote = root.path().join("remote.git");
    create_repository(&repository, &remote);
    let bin = root.path().join("bin");
    fs::create_dir(&bin).unwrap();
    fake_gh(&bin.join("gh"));
    let path = prepend_path(&bin);
    let state = root.path().join("server");
    let log = root.path().join("server.log");
    let mut process = start_with_path(&state, &log, Some(&path));
    let address = ready(&mut process, &log).await;
    let mut client = connect(&address, server_protocol::forge::CAPABILITIES).await;

    let search = request(
        &mut client,
        "forge.search.request",
        json!({"cwd":repository,"query":"fix"}),
    )
    .await;
    assert_eq!(search["result"]["items"][1]["kind"], "issue", "{search}");
    assert_eq!(search["result"]["authState"], "authenticated");
    let legacy = request(
        &mut client,
        "github.search.request",
        json!({"cwd":repository,"query":"fix","kinds":["pr"]}),
    )
    .await;
    assert_eq!(legacy["result"]["items"][0]["kind"], "pr");
    assert_eq!(legacy["result"]["githubFeaturesEnabled"], true);

    let created = request(
        &mut client,
        "checkout.pr.create.request",
        json!({"cwd":repository,"title":"Fix","body":"Details","baseRef":"main"}),
    )
    .await;
    assert_eq!(created["result"]["number"], 3, "{created}");
    let status = request(
        &mut client,
        "checkout.pr.status.request",
        json!({"cwd":repository}),
    )
    .await;
    assert_eq!(status["result"]["status"]["number"], 3, "{status}");
    assert_eq!(status["result"]["status"]["checksStatus"], "success");
    let merged = request(
        &mut client,
        "checkout.pr.merge.request",
        json!({"cwd":repository,"mergeMethod":"squash"}),
    )
    .await;
    assert_eq!(merged["result"]["success"], true, "{merged}");

    let enabled = request(
        &mut client,
        "checkout.forge.set_auto_merge.request",
        json!({"cwd":repository,"enabled":true,"mergeMethod":"merge"}),
    )
    .await;
    assert_eq!(enabled["result"]["success"], true, "{enabled}");
    let disabled = request(
        &mut client,
        "checkout.github.set_auto_merge.request",
        json!({"cwd":repository,"enabled":false}),
    )
    .await;
    assert_eq!(disabled["result"]["enabled"], false, "{disabled}");

    let timeline = request(
        &mut client,
        "checkout.pr.timeline.request",
        json!({"cwd":repository,"prNumber":3,"repoOwner":"acme","repoName":"app"}),
    )
    .await;
    assert_eq!(
        timeline["result"]["items"][0]["kind"], "review",
        "{timeline}"
    );
    for method in [
        "checkout.forge.get_check_details.request",
        "checkout.github.get_check_details.request",
    ] {
        let details = request(
            &mut client,
            method,
            json!({
                "cwd":repository,
                "repoOwner":"acme",
                "repoName":"app",
                "checkRunId":9,
                "workflowRunId":44
            }),
        )
        .await;
        assert_eq!(details["result"]["success"], true, "{details}");
        assert_eq!(
            details["result"]["details"]["annotations"][0]["path"],
            "src/lib.rs"
        );
    }

    terminate(&mut process).await;
}

fn create_repository(repository: &Path, remote: &Path) {
    fs::create_dir_all(repository).unwrap();
    run(repository, &["init", "-b", "main"]);
    run(
        repository,
        &["config", "user.email", "server@example.invalid"],
    );
    run(repository, &["config", "user.name", "Server Test"]);
    fs::write(repository.join("tracked.txt"), "base\n").unwrap();
    run(repository, &["add", "."]);
    run(repository, &["commit", "-m", "base"]);
    run(
        repository,
        &["init", "--bare", "-b", "main", remote.to_str().unwrap()],
    );
    run(
        repository,
        &["remote", "add", "origin", "https://github.com/acme/app.git"],
    );
    run(
        repository,
        &[
            "config",
            &format!("url.{}.insteadOf", remote.display()),
            "https://github.com/acme/app.git",
        ],
    );
    run(repository, &["push", "-u", "origin", "main"]);
    run(repository, &["checkout", "-b", "feature"]);
    fs::write(repository.join("feature.txt"), "feature\n").unwrap();
    run(repository, &["add", "."]);
    run(repository, &["commit", "-m", "feature"]);
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

fn prepend_path(bin: &Path) -> OsString {
    let mut paths = vec![bin.to_path_buf()];
    paths.extend(std::env::split_paths(
        &std::env::var_os("PATH").unwrap_or_default(),
    ));
    std::env::join_paths(paths).unwrap()
}

fn fake_gh(path: &Path) {
    let script = r#"#!/bin/sh
if [ "$1 $2" = "issue list" ]; then
  echo '[{"number":1,"title":"Issue","url":"https://github.com/acme/app/issues/1","state":"OPEN","body":"body","labels":[{"name":"bug"}],"updatedAt":"2026-01-01T00:00:00Z"}]'
elif [ "$1 $2" = "pr list" ]; then
  echo '[{"number":3,"title":"PR","url":"https://github.com/acme/app/pull/3","state":"OPEN","body":null,"labels":[],"baseRefName":"main","headRefName":"feature","updatedAt":"2026-02-01T00:00:00Z"}]'
elif [ "$1 $2" = "pr view" ]; then
  echo '{"number":3,"url":"https://github.com/acme/app/pull/3","title":"PR","state":"OPEN","isDraft":false,"baseRefName":"main","headRefName":"feature","mergedAt":null,"reviewDecision":"APPROVED","mergeable":"MERGEABLE","statusCheckRollup":[{"__typename":"CheckRun","name":"tests","status":"COMPLETED","conclusion":"SUCCESS","detailsUrl":"https://example/check","databaseId":9,"checkSuite":{"workflowRun":{"databaseId":44}}}]}'
elif [ "$1 $2" = "pr merge" ]; then
  exit 0
elif [ "$1" = "api" ] && [ "$2" = "-X" ]; then
  echo '{"number":3,"html_url":"https://github.com/acme/app/pull/3"}'
elif [ "$1 $2" = "api graphql" ]; then
  echo '{"data":{"repository":{"pullRequest":{"number":3,"reviews":{"nodes":[{"id":"R1","state":"APPROVED","body":"ship","url":"https://example/review","submittedAt":"2026-01-01T00:00:00Z","author":{"login":"octo","url":null,"avatarUrl":null}}],"pageInfo":{"hasNextPage":false}},"comments":{"nodes":[],"pageInfo":{"hasNextPage":false}},"reviewThreads":{"nodes":[],"pageInfo":{"hasNextPage":false}}}}}}'
elif [ "$1" = "api" ] && [ "$2" = "repos/acme/app/check-runs/9" ]; then
  echo '{"id":9,"name":"tests","status":"completed","conclusion":"success","html_url":"https://example/check","details_url":"https://example/details","check_suite":{"workflow_run":{"id":44}},"output":null}'
elif [ "$1" = "api" ] && [ "$2" = "repos/acme/app/check-runs/9/annotations" ]; then
  echo '[{"path":"src/lib.rs","start_line":1,"end_line":1,"annotation_level":"notice","message":"ok"}]'
elif [ "$1" = "api" ] && [ "$2" = "repos/acme/app/actions/runs/44/jobs" ]; then
  echo '{"jobs":[]}'
else
  echo "unexpected: $*" >&2
  exit 2
fi
"#;
    fs::write(path, script).unwrap();
    fs::set_permissions(path, fs::Permissions::from_mode(0o700)).unwrap();
}
