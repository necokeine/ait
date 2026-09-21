use std::process::Command;

use serde_json::json;

use super::transport::{connect, request};
use super::{ready, start, terminate};

#[tokio::test]
async fn binary_project_ownership_survives_catalogs_and_process_restarts() {
    let root = tempfile::tempdir().unwrap();
    let repo = root.path().join("repository");
    std::fs::create_dir(&repo).unwrap();
    for args in [
        vec!["init", "--quiet"],
        vec![
            "-c",
            "user.name=Server Test",
            "-c",
            "user.email=server@example.invalid",
            "commit",
            "--quiet",
            "--allow-empty",
            "-m",
            "initial",
        ],
    ] {
        assert!(
            Command::new("git")
                .args(args)
                .current_dir(&repo)
                .output()
                .unwrap()
                .status
                .success()
        );
    }
    let first_dir = root.path().join("first");
    let first_log = root.path().join("first.log");
    let second_dir = root.path().join("second");
    let second_log = root.path().join("second.log");
    let mut first = start(&first_dir, &first_log);
    let mut first_socket = connect(
        &ready(&mut first, &first_log).await,
        server_protocol::project_lease::CAPABILITIES,
    )
    .await;
    let params = json!({"path":repo,"idempotency_key":"open"});
    let opened = request(&mut first_socket, "project.open", params.clone()).await;
    assert_eq!(opened["type"], "response", "{opened}");
    let receipt = opened["result"].clone();
    let mut second = start(&second_dir, &second_log);
    let mut second_socket = connect(
        &ready(&mut second, &second_log).await,
        server_protocol::project_lease::CAPABILITIES,
    )
    .await;
    assert_eq!(
        request(&mut second_socket, "project.open", params.clone()).await["code"],
        "project_busy"
    );
    terminate(&mut first).await;
    assert_eq!(
        request(&mut second_socket, "project.open", params.clone()).await["result"]["project_id"],
        receipt["project_id"]
    );
    let mut restarted = start(&first_dir, &first_log);
    let mut restarted_socket = connect(
        &ready(&mut restarted, &first_log).await,
        server_protocol::project_lease::CAPABILITIES,
    )
    .await;
    assert_eq!(
        request(&mut restarted_socket, "project.open", params).await["result"],
        receipt
    );
    let summary = request(
        &mut restarted_socket,
        "project.get",
        json!({"project_id":receipt["project_id"]}),
    )
    .await;
    assert!(summary["result"]["owner_epoch"].is_null());
    terminate(&mut second).await;
    let acquired = request(
        &mut restarted_socket,
        "project.open",
        json!({"path":repo,"idempotency_key":"reacquire"}),
    )
    .await;
    assert_eq!(acquired["result"]["project_id"], receipt["project_id"]);
    terminate(&mut restarted).await;
}
