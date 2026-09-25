use std::path::Path;
use std::process::Command;

use serde_json::json;
use server_filesystem::protocol::checkout::CAPABILITIES;

use super::transport::{Socket, connect, receive, request};
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
    let mut client = connect(&address, CAPABILITIES).await;

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

#[tokio::test]
async fn binary_serves_checkout_branch_and_mutation_methods() {
    let root = tempfile::tempdir().unwrap();
    let repository = root.path().join("repository");
    create_clean_repository(&repository);
    let remote = root.path().join("remote.git");
    run(
        root.path(),
        &["init", "--bare", "-b", "main", remote.to_str().unwrap()],
    );
    run(
        &repository,
        &["remote", "add", "origin", remote.to_str().unwrap()],
    );
    run(&repository, &["push", "-u", "origin", "main"]);
    run(&repository, &["checkout", "-b", "feature"]);

    let state = root.path().join("server");
    let log = root.path().join("server.log");
    let mut process = start(&state, &log);
    let address = ready(&mut process, &log).await;
    let mut client = connect(&address, CAPABILITIES).await;

    assert_branch_methods(&mut client, &repository).await;
    assert_mutation_methods(&mut client, &repository).await;

    terminate(&mut process).await;
}

async fn assert_branch_methods(client: &mut Socket, repository: &Path) {
    let validation = request(
        client,
        "checkout.branch.validate.request",
        json!({"cwd":repository,"branchName":"main"}),
    )
    .await;
    assert_eq!(validation["result"]["exists"], true, "{validation}");
    let suggestions = request(
        client,
        "checkout.branch.suggestions.request",
        json!({"cwd":repository,"query":"ma","limit":10}),
    )
    .await;
    assert_eq!(suggestions["result"]["branches"], json!(["main"]));
    let switched = request(
        client,
        "checkout.branch.switch.request",
        json!({"cwd":repository,"branch":"main"}),
    )
    .await;
    assert_eq!(switched["result"]["source"], "local", "{switched}");
    let switched = request(
        client,
        "checkout.branch.switch.request",
        json!({"cwd":repository,"branch":"feature"}),
    )
    .await;
    assert_eq!(switched["result"]["success"], true, "{switched}");
    let renamed = request(
        client,
        "checkout.rename_branch.request",
        json!({"cwd":repository,"branch":"renamed"}),
    )
    .await;
    assert_eq!(renamed["result"]["currentBranch"], "renamed", "{renamed}");
}

async fn assert_mutation_methods(client: &mut Socket, repository: &Path) {
    std::fs::write(repository.join("committed.txt"), "committed\n").unwrap();
    let committed = request(
        client,
        "checkout.commit.request",
        json!({"cwd":repository,"message":"server mutation"}),
    )
    .await;
    assert_eq!(committed["result"]["success"], true, "{committed}");
    std::fs::write(repository.join("stash.txt"), "stash\n").unwrap();
    let saved = request(
        client,
        "checkout.stash.save.request",
        json!({"cwd":repository,"branch":"renamed"}),
    )
    .await;
    assert_eq!(saved["result"]["success"], true, "{saved}");
    let stashes = request(
        client,
        "checkout.stash.list.request",
        json!({"cwd":repository}),
    )
    .await;
    assert_eq!(stashes["result"]["entries"][0]["branch"], "renamed");
    let popped = request(
        client,
        "checkout.stash.pop.request",
        json!({"cwd":repository,"stashIndex":0}),
    )
    .await;
    assert_eq!(popped["result"]["success"], true, "{popped}");
    let discarded = request(
        client,
        "checkout.discard_changes.request",
        json!({"cwd":repository,"paths":["stash.txt"]}),
    )
    .await;
    assert_eq!(discarded["result"]["success"], true, "{discarded}");
    assert!(!repository.join("stash.txt").exists());
    let from_base = request(
        client,
        "checkout.merge_from_base.request",
        json!({"cwd":repository,"baseRef":"main"}),
    )
    .await;
    assert_eq!(from_base["result"]["success"], true, "{from_base}");
    let to_base = request(
        client,
        "checkout.merge.request",
        json!({"cwd":repository,"baseRef":"main","strategy":"merge"}),
    )
    .await;
    assert_eq!(to_base["result"]["success"], true, "{to_base}");
    let pushed = request(client, "checkout.push.request", json!({"cwd":repository})).await;
    assert_eq!(pushed["result"]["success"], true, "{pushed}");
    let pulled = request(client, "checkout.pull.request", json!({"cwd":repository})).await;
    assert_eq!(pulled["result"]["success"], true, "{pulled}");
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

fn create_clean_repository(repository: &Path) {
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
