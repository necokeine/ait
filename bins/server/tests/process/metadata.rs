use serde_json::json;

use super::transport::{connect, request};
use super::{ready, start, terminate};

#[tokio::test]
async fn projects_use_only_the_metadata_registry_and_survive_restart() {
    let root = tempfile::tempdir().unwrap();
    let state = root.path().join("server");
    let project = root.path().join("project");
    let retired = project.join(".ait-server");
    std::fs::create_dir_all(&retired).unwrap();
    let historical = retired.join("project.sqlite3");
    std::fs::write(&historical, "untouched historical project data").unwrap();
    let log = root.path().join("server.log");
    let mut process = start(&state, &log);
    let address = ready(&mut process, &log).await;
    let mut socket = connect(&address, server_metadata::protocol::directory::CAPABILITIES).await;

    for method in [
        "project.open",
        "project.list",
        "project.get",
        "project.close",
    ] {
        assert_eq!(
            request(&mut socket, method, json!({"path":project})).await["code"],
            "method_not_found"
        );
    }
    let added = request(&mut socket, "project.add.request", json!({"cwd":project})).await;
    let project_id = added["result"]["project"]["projectId"]
        .as_str()
        .unwrap()
        .to_owned();
    assert!(project_id.starts_with("prj_"));
    let created = request(
        &mut socket,
        "workspace.create.request",
        json!({
            "source":{"kind":"directory","path":project,"projectId":project_id},
            "title":"Persistent workspace"
        }),
    )
    .await;
    let workspace_id = created["result"]["workspace"]["id"]
        .as_str()
        .unwrap()
        .to_owned();
    assert!(workspace_id.starts_with("wks_"));
    assert_eq!(
        request(
            &mut socket,
            "project.rename.request",
            json!({"projectId":project_id,"customName":"Renamed"})
        )
        .await["result"]["accepted"],
        true
    );
    drop(socket);
    terminate(&mut process).await;

    let mut restarted = start(&state, &log);
    let address = ready(&mut restarted, &log).await;
    let mut socket = connect(&address, server_metadata::protocol::directory::CAPABILITIES).await;
    let projects = request(&mut socket, "project.list.request", json!({})).await;
    assert_eq!(projects["result"]["projects"].as_array().unwrap().len(), 1);
    assert_eq!(projects["result"]["projects"][0]["projectId"], project_id);
    assert_eq!(
        projects["result"]["projects"][0]["projectDisplayName"],
        "Renamed"
    );
    let workspaces = request(&mut socket, "workspace.list.request", json!({})).await;
    assert_eq!(workspaces["result"]["entries"][0]["id"], workspace_id);
    assert_eq!(
        workspaces["result"]["entries"][0]["title"],
        "Persistent workspace"
    );
    assert_eq!(
        std::fs::read_to_string(&historical).unwrap(),
        "untouched historical project data"
    );
    assert!(!retired.join("project.lock").exists());
    assert!(!root.path().join(".ait-server-project-locks").exists());
    assert!(state.join("projects/projects.json").is_file());
    assert!(state.join("projects/workspaces.json").is_file());
    drop(socket);
    terminate(&mut restarted).await;
}
