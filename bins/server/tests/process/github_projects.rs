use std::os::unix::fs::PermissionsExt;
use std::path::Path;
use std::process::Command;

use serde_json::json;

use super::transport::{connect, request};
use super::{ready, start_with_path, terminate};

#[tokio::test]
async fn binary_searches_and_clones_github_project_with_local_git_transport() {
    let root = tempfile::tempdir().expect("root");
    let path = prepare_local_github(root.path());
    let state = root.path().join("server");
    let log = root.path().join("server.log");
    let mut process = start_with_path(&state, &log, Some(std::ffi::OsStr::new(&path)));
    let address = ready(&mut process, &log).await;
    let mut socket = connect(
        &address,
        server_filesystem::protocol::github_projects::CAPABILITIES,
    )
    .await;

    let search = request(
        &mut socket,
        "workspace.github.search_repositories.request",
        json!({"query":"repo","limit":5}),
    )
    .await;
    assert_eq!(search["result"]["status"], "success");
    assert_eq!(
        search["result"]["repositories"][0]["nameWithOwner"],
        "owner/repo"
    );
    let unauthenticated = request(
        &mut socket,
        "workspace.github.search_repositories.request",
        json!({"query":"auth"}),
    )
    .await;
    assert_eq!(unauthenticated["result"]["status"], "unauthenticated");
    assert_eq!(unauthenticated["result"]["available"], false);
    let target = root.path().join("checkouts");
    let cloned = request(
        &mut socket,
        "project.github.clone.request",
        json!({"repo":"owner/repo","cloneProtocol":"https","targetDirectory":target}),
    )
    .await;
    assert_eq!(cloned["result"]["repo"], "owner/repo");
    assert_eq!(cloned["result"]["project"]["projectKind"], "git");
    assert!(cloned["result"]["error"].is_null());
    assert!(target.join("repo/.git").is_dir());
    let collision = request(
        &mut socket,
        "project.github.clone.request",
        json!({"repo":"owner/repo","cloneProtocol":"https","targetDirectory":target}),
    )
    .await;
    assert!(collision["result"]["project"].is_null());
    assert_eq!(collision["result"]["error"], "Checkout path already exists");
    assert_eq!(
        collision["result"]["checkoutPath"],
        cloned["result"]["checkoutPath"]
    );
    assert_eq!(
        request(
            &mut socket,
            "project.github.clone.request",
            json!({"repo":"owner/repo","cloneProtocol":"ftp","targetDirectory":target})
        )
        .await["code"],
        "invalid_message"
    );
    terminate(&mut process).await;
    assert!(
        Path::new(
            &cloned["result"]["checkoutPath"]
                .as_str()
                .expect("checkout path")
        )
        .exists()
    );
}

fn prepare_local_github(root: &Path) -> String {
    let source = root.join("source");
    std::fs::create_dir(&source).expect("source");
    let bare = source.join("repo.git");
    assert!(
        Command::new("git")
            .args(["init", "--bare", "-q"])
            .arg(&bare)
            .status()
            .expect("init bare")
            .success()
    );
    std::fs::write(
        root.join(".gitconfig"),
        format!(
            "[url \"file://{}/\"]\n\tinsteadOf = https://github.com/owner/\n",
            source.display()
        ),
    )
    .expect("git config");
    let bin = root.join("bin");
    std::fs::create_dir(&bin).expect("bin");
    let gh = bin.join("gh");
    std::fs::write(
        &gh,
        r#"#!/bin/sh
case "$1" in
  search) if [ "$3" = "auth" ]; then printf 'gh auth login\n' >&2; exit 1; fi
          printf '[{"id":"R_1","name":"repo","fullName":"owner/repo","description":"test","isPrivate":false,"updatedAt":"2026-09-23T00:00:00Z","url":"https://github.com/owner/repo"}]' ;;
  config) printf 'https\n' ;;
esac
"#,
    )
    .expect("gh script");
    let mut permissions = std::fs::metadata(&gh).expect("metadata").permissions();
    permissions.set_mode(0o700);
    std::fs::set_permissions(&gh, permissions).expect("permissions");
    format!(
        "{}:{}",
        bin.display(),
        std::env::var("PATH").expect("test PATH")
    )
}
