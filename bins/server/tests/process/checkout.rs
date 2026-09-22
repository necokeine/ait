use std::path::Path;
use std::process::Command;

use serde_json::json;

use super::transport::{connect, receive, request};
use super::{ready, start, terminate};

#[tokio::test]
async fn binary_serves_checkout_reads_and_connection_owned_diff_updates() {
    let root = tempfile::tempdir().unwrap();
    let repository = root.path().join("repository");
    create_repository(&repository);
    let state = root.path().join("server");
    let log = root.path().join("server.log");
    let mut process = start(&state, &log);
    let address = ready(&mut process, &log).await;
    let mut client = connect(&address, server_protocol::checkout::CAPABILITIES).await;

    let status = request(
        &mut client,
        "checkout.status.get.request",
        json!({"cwd":repository}),
    )
    .await;
    assert_eq!(status["result"]["isGit"], true, "{status}");
    assert_eq!(status["result"]["currentBranch"], "feature");
    assert_eq!(status["result"]["baseRef"], "main");
    assert_eq!(status["result"]["isDirty"], true);

    let diff = request(
        &mut client,
        "checkout.diff.get.request",
        json!({"cwd":repository,"compare":{"mode":"uncommitted"}}),
    )
    .await;
    assert_eq!(
        diff["result"]["files"]
            .as_array()
            .unwrap()
            .iter()
            .map(|file| file["path"].as_str().unwrap())
            .collect::<Vec<_>>(),
        ["new.txt", "tracked.txt"]
    );

    let commits = request(
        &mut client,
        "checkout.commits.list.request",
        json!({"cwd":repository}),
    )
    .await;
    assert_eq!(commits["result"]["baseRef"], "main", "{commits}");
    assert_eq!(commits["result"]["commits"][0]["subject"], "feature");
    assert_eq!(commits["result"]["commits"][0]["isOnBase"], false);
    let sha = commits["result"]["commits"][0]["sha"]
        .as_str()
        .unwrap()
        .to_owned();
    let file_diff = request(
        &mut client,
        "checkout.commits.file_diff.request",
        json!({"cwd":repository,"sha":sha,"path":"tracked.txt"}),
    )
    .await;
    assert_eq!(file_diff["result"]["file"]["path"], "tracked.txt");
    assert_eq!(file_diff["result"]["file"]["additions"], 1);
    assert_eq!(file_diff["result"]["file"]["deletions"], 1);

    let subscription = request(
        &mut client,
        "checkout.diff.subscribe.request",
        json!({
            "subscriptionId":"diff-live",
            "cwd":repository,
            "compare":{"mode":"uncommitted"}
        }),
    )
    .await;
    assert_eq!(subscription["result"]["subscriptionId"], "diff-live");
    let replacement = request(
        &mut client,
        "checkout.diff.subscribe.request",
        json!({
            "subscriptionId":"diff-live",
            "cwd":repository,
            "compare":{"mode":"uncommitted"}
        }),
    )
    .await;
    assert_eq!(replacement["result"]["subscriptionId"], "diff-live");
    std::fs::write(repository.join("new.txt"), "new\nchanged\n").unwrap();
    let update = receive(&mut client).await;
    assert_eq!(update["type"], "event", "{update}");
    assert_eq!(update["method"], "checkout.diff.update");
    assert_eq!(update["params"]["subscriptionId"], "diff-live");
    assert_eq!(update["params"]["files"][0]["additions"], 2);

    let unsubscribed = request(
        &mut client,
        "checkout.diff.unsubscribe.request",
        json!({"subscriptionId":"diff-live"}),
    )
    .await;
    assert_eq!(unsubscribed["result"]["subscriptionId"], "diff-live");
    let refreshed = request(
        &mut client,
        "checkout.refresh.request",
        json!({"cwd":repository}),
    )
    .await;
    assert_eq!(refreshed["result"]["success"], true);

    terminate(&mut process).await;
}

fn create_repository(repository: &Path) {
    std::fs::create_dir_all(repository).unwrap();
    run(repository, &["init", "--quiet", "--initial-branch=main"]);
    run(repository, &["config", "user.name", "Server Test"]);
    run(
        repository,
        &["config", "user.email", "server@example.invalid"],
    );
    std::fs::write(repository.join("tracked.txt"), "base\n").unwrap();
    run(repository, &["add", "tracked.txt"]);
    run(repository, &["commit", "--quiet", "-m", "base"]);
    run(repository, &["checkout", "--quiet", "-b", "feature"]);
    std::fs::write(repository.join("tracked.txt"), "feature\n").unwrap();
    run(repository, &["add", "tracked.txt"]);
    run(repository, &["commit", "--quiet", "-m", "feature"]);
    std::fs::write(repository.join("tracked.txt"), "feature\nworking\n").unwrap();
    std::fs::write(repository.join("new.txt"), "new\n").unwrap();
}

fn run(repository: &Path, arguments: &[&str]) {
    let output = Command::new("git")
        .args(arguments)
        .current_dir(repository)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "git {:?}: {}",
        arguments,
        String::from_utf8_lossy(&output.stderr)
    );
}
