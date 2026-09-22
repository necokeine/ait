use serde_json::{Value, json};

use super::transport::{connect, receive, request};
use super::{ready, start, terminate};

const LABEL_CAPABILITIES: &[&str] = &[
    "workspace.label.list.request",
    "workspace.label.assignment.set.request",
    "workspace.label.update.request",
    "workspace.label.delete.inspect.request",
    "workspace.label.delete.request",
    "subscription.release.request",
];

#[tokio::test]
async fn workspace_labels_persist_and_publish_canonical_subscriptions() {
    let root = tempfile::tempdir().unwrap();
    let state = root.path().join("server");
    seed_workspace(&state);
    let log = root.path().join("server.log");
    let mut process = start(&state, &log);
    let address = ready(&mut process, &log).await;
    let mut subscriber = connect(
        &address,
        &[
            "workspace.label.list.request",
            "subscription.release.request",
        ],
    )
    .await;
    let mut writer = connect(&address, LABEL_CAPABILITIES).await;

    let rejected = request(
        &mut subscriber,
        "workspace.label.list.request",
        json!({"subscribe":{"subscriptionId":"client-chosen"}}),
    )
    .await;
    assert_eq!(rejected["code"], "invalid_message");
    let initial = request(
        &mut subscriber,
        "workspace.label.list.request",
        json!({"subscribe":{}}),
    )
    .await;
    assert_eq!(initial["result"]["labels"], json!([]));
    assert_eq!(initial["result"]["sync"]["mode"], "snapshot");
    let subscription_id = initial["result"]["subscriptionId"]
        .as_str()
        .unwrap()
        .to_owned();
    assert!(!subscription_id.is_empty());

    let assigned = request(
        &mut writer,
        "workspace.label.assignment.set.request",
        json!({
            "workspaceId":"wks_one",
            "label":{"name":"  Needs   review ","color":"sky"},
            "assigned":true
        }),
    )
    .await;
    assert_eq!(assigned["result"]["label"]["name"], "Needs review");
    assert_eq!(
        assigned["result"]["workspaceLabels"],
        json!(["Needs review"])
    );
    let created = receive(&mut subscriber).await;
    assert_eq!(created["type"], "event");
    assert_eq!(created["method"], "workspace.label.update");
    assert_eq!(created["params"]["subscriptionId"], subscription_id);
    assert_eq!(created["params"]["kind"], "upsert");
    assert_eq!(created["params"]["seq"], 1);

    let edited = request(
        &mut writer,
        "workspace.label.update.request",
        json!({"name":"needs REVIEW","newName":"Priority","color":"amber"}),
    )
    .await;
    assert_eq!(
        edited["result"]["label"],
        json!({"name":"Priority","color":"amber"})
    );
    assert_eq!(edited["result"]["affectedWorkspaceCount"], 1);
    let changed = receive(&mut subscriber).await;
    assert_eq!(changed["params"]["previousName"], "Needs review");
    assert_eq!(changed["params"]["seq"], 2);
    assert_eq!(
        request(
            &mut writer,
            "workspace.label.delete.inspect.request",
            json!({"name":"priority"})
        )
        .await["result"]["affectedWorkspaceCount"],
        1
    );

    terminate(&mut process).await;
    let mut process = start(&state, &log);
    let address = ready(&mut process, &log).await;
    let mut client = connect(&address, LABEL_CAPABILITIES).await;
    let persisted = request(&mut client, "workspace.label.list.request", json!({})).await;
    assert_eq!(
        persisted["result"]["labels"],
        json!([{"name":"Priority","color":"amber"}])
    );
    assert_eq!(persisted["result"]["sync"]["mode"], "snapshot");
    terminate(&mut process).await;
}

#[tokio::test]
async fn workspace_label_subscription_release_stops_updates_and_delete_rewrites_disk() {
    let root = tempfile::tempdir().unwrap();
    let state = root.path().join("server");
    seed_workspace(&state);
    let log = root.path().join("server.log");
    let mut process = start(&state, &log);
    let address = ready(&mut process, &log).await;
    let mut client = connect(&address, LABEL_CAPABILITIES).await;
    request(
        &mut client,
        "workspace.label.assignment.set.request",
        json!({
            "workspaceId":"wks_one",
            "label":{"name":"Priority","color":"amber"},
            "assigned":true
        }),
    )
    .await;
    let mut subscriber = connect(
        &address,
        &[
            "workspace.label.list.request",
            "subscription.release.request",
        ],
    )
    .await;
    let first_response = request(
        &mut subscriber,
        "workspace.label.list.request",
        json!({"subscribe":{}}),
    )
    .await;
    let first_id = first_response["result"]["subscriptionId"]
        .as_str()
        .unwrap()
        .to_owned();
    let second_response = request(
        &mut subscriber,
        "workspace.label.list.request",
        json!({"subscribe":{}}),
    )
    .await;
    let second_id = second_response["result"]["subscriptionId"]
        .as_str()
        .unwrap()
        .to_owned();
    assert_ne!(first_id, second_id);
    let released = request(
        &mut subscriber,
        "subscription.release.request",
        json!({"subscriptionId":first_id}),
    )
    .await;
    assert_eq!(released["result"]["subscriptionId"], first_id);
    let deleted = request(
        &mut client,
        "workspace.label.delete.request",
        json!({"name":"PRIORITY"}),
    )
    .await;
    assert_eq!(deleted["result"]["affectedWorkspaceCount"], 1);
    let update = receive(&mut subscriber).await;
    assert_eq!(update["params"]["subscriptionId"], second_id);
    assert_eq!(update["params"]["kind"], "remove");
    request(
        &mut subscriber,
        "subscription.release.request",
        json!({"subscriptionId":second_id}),
    )
    .await;
    let after_release = request(&mut subscriber, "workspace.label.list.request", json!({})).await;
    assert_eq!(after_release["type"], "response");
    assert_eq!(after_release["result"]["labels"], json!([]));

    let catalog: Value = serde_json::from_slice(
        &std::fs::read(state.join("projects/workspace-labels.json")).unwrap(),
    )
    .unwrap();
    let workspaces: Value =
        serde_json::from_slice(&std::fs::read(state.join("projects/workspaces.json")).unwrap())
            .unwrap();
    assert_eq!(catalog, json!([]));
    assert!(workspaces[0].get("labels").is_none());
    assert!(
        !state
            .join("projects/workspace-labels.transaction.json")
            .exists()
    );
    terminate(&mut process).await;
}

fn seed_workspace(state: &std::path::Path) {
    let projects = state.join("projects");
    std::fs::create_dir_all(&projects).unwrap();
    std::fs::write(
        projects.join("projects.json"),
        serde_json::to_vec_pretty(&json!([{
            "projectId":"prj_one",
            "rootPath":"/repo",
            "kind":"non_git",
            "displayName":"repo",
            "projectKey":null,
            "customName":null,
            "customIconRevision":null,
            "createdAt":"2026-08-14T00:00:00.000Z",
            "updatedAt":"2026-08-14T00:00:00.000Z",
            "archivedAt":null
        }]))
        .unwrap(),
    )
    .unwrap();
    std::fs::write(
        projects.join("workspaces.json"),
        serde_json::to_vec_pretty(&json!([{
            "workspaceId":"wks_one",
            "projectId":"prj_one",
            "cwd":"/repo",
            "kind":"directory",
            "displayName":"repo",
            "createdAt":"2026-08-14T00:00:00.000Z",
            "updatedAt":"2026-08-14T00:00:00.000Z",
            "archivedAt":null
        }]))
        .unwrap(),
    )
    .unwrap();
}
