//! Behavioral cases adapted from Paseo terminal-manager and terminal-size-ownership tests.

use super::*;

fn resize(rows: u16, intent: ResizeIntent) -> Input {
    Input::Resize(Resize {
        size: Size { rows, cols: 80 },
        intent,
    })
}

#[test]
fn same_cwd_workspaces_keep_their_terminal_identity_when_a_sibling_is_archived() {
    let (mut service, registry, calls) = fixture();
    let mut sibling = registry.workspaces.lock().unwrap()[0].clone();
    sibling.workspace_id = "sibling".to_owned();
    registry.workspaces.lock().unwrap().push(sibling);
    let mut options = request();
    options.workspace_id = Some("w".to_owned());
    let first = service.create(&options).unwrap();
    options.workspace_id = Some("sibling".to_owned());
    let second = service.create(&options).unwrap();
    for terminal in [&first, &second] {
        let listed = service
            .list(&ListRequest {
                cwd: None,
                workspace_id: Some(terminal.workspace_id.clone()),
            })
            .unwrap();
        assert_eq!(listed, std::slice::from_ref(terminal));
    }
    registry.workspaces.lock().unwrap()[0].archived_at = Some("archived".to_owned());
    service.reconcile().unwrap();
    assert_eq!(service.list(&ListRequest::default()).unwrap(), [second]);
    assert_eq!(calls.lock().unwrap().killed, 1);
    service.reconcile().unwrap();
    assert_eq!(calls.lock().unwrap().killed, 1);
}

#[test]
fn removing_a_workspace_does_not_reassign_its_terminal_to_an_identical_root() {
    let (mut service, registry, calls) = fixture();
    let terminal = service.create(&request()).unwrap();
    registry.workspaces.lock().unwrap()[0].workspace_id = "replacement".to_owned();
    service.reconcile().unwrap();
    assert!(service.list(&ListRequest::default()).unwrap().is_empty());
    assert!(service.observe(&terminal.id, Some(0), None).unwrap().exited);
    assert_eq!(calls.lock().unwrap().killed, 1);
}

#[test]
fn project_archival_closes_all_its_workspaces_and_preserves_other_projects() {
    let (mut service, registry, calls) = fixture();
    service.create(&request()).unwrap();
    let mut project = registry.projects.lock().unwrap()[0].clone();
    project.project_id = "other-project".to_owned();
    project.root_path = "/other".to_owned();
    registry.projects.lock().unwrap().push(project);
    let mut workspace = registry.workspaces.lock().unwrap()[0].clone();
    workspace.project_id = "other-project".to_owned();
    workspace.workspace_id = "other-workspace".to_owned();
    workspace.cwd = "/other".to_owned();
    registry.workspaces.lock().unwrap().push(workspace);
    let mut options = request();
    options.cwd = "/other/sub".to_owned();
    let other = service.create(&options).unwrap();
    registry.projects.lock().unwrap()[0].archived_at = Some("archived".to_owned());
    service.reconcile().unwrap();
    assert_eq!(service.list(&ListRequest::default()).unwrap(), [other]);
    assert_eq!(calls.lock().unwrap().killed, 1);
}

#[test]
fn explicit_archived_workspace_is_not_replaced_with_an_active_sibling() {
    let (mut service, registry, calls) = fixture();
    let mut archived = registry.workspaces.lock().unwrap()[0].clone();
    archived.workspace_id = "archived".to_owned();
    archived.archived_at = Some("yesterday".to_owned());
    registry.workspaces.lock().unwrap().push(archived);
    let mut options = request();
    options.workspace_id = Some("archived".to_owned());
    assert_eq!(service.create(&options), Err(Error::WorkspaceNotFound));
    assert!(calls.lock().unwrap().launches.is_empty());
}

#[test]
fn sibling_path_prefix_does_not_count_as_a_workspace_descendant() {
    let (mut service, _, calls) = fixture();
    let mut options = request();
    options.cwd = "/repository".to_owned();
    assert_eq!(service.create(&options), Err(Error::WorkspaceNotFound));
    options.workspace_id = Some("w".to_owned());
    assert_eq!(service.create(&options), Err(Error::Invalid));
    assert!(calls.lock().unwrap().launches.is_empty());
}

#[test]
fn failed_workspace_reconciliation_preserves_the_running_terminal_for_retry() {
    let (mut service, registry, calls) = fixture();
    let terminal = service.create(&request()).unwrap();
    registry.workspaces.lock().unwrap().clear();
    calls.lock().unwrap().failure = true;
    assert_eq!(service.reconcile(), Err(Error::Io));
    assert!(!service.observe(&terminal.id, Some(0), None).unwrap().exited);
    calls.lock().unwrap().failure = false;
    service.reconcile().unwrap();
    assert!(service.observe(&terminal.id, Some(0), None).unwrap().exited);
    assert_eq!(calls.lock().unwrap().killed, 1);
}

#[test]
fn registry_read_failure_never_kills_processes_based_on_an_empty_snapshot() {
    let (mut service, registry, calls) = fixture();
    let terminal = service.create(&request()).unwrap();
    *registry.failure.lock().unwrap() = true;
    assert_eq!(service.reconcile(), Err(Error::Registry));
    assert_eq!(calls.lock().unwrap().killed, 0);
    *registry.failure.lock().unwrap() = false;
    assert_eq!(service.list(&ListRequest::default()).unwrap(), [terminal]);
}

#[test]
fn a_same_size_claim_transfers_ownership_and_fences_the_previous_owner() {
    let (mut service, _, calls) = fixture();
    let terminal = service.create(&request()).unwrap();
    service
        .input(&terminal.id, "a", &resize(30, ResizeIntent::Claim))
        .unwrap();
    service
        .input(&terminal.id, "b", &resize(30, ResizeIntent::Claim))
        .unwrap();
    let accepted = calls.lock().unwrap().inputs.len();
    service
        .input(&terminal.id, "a", &resize(31, ResizeIntent::Update))
        .unwrap();
    assert_eq!(calls.lock().unwrap().inputs.len(), accepted);
    service
        .input(&terminal.id, "b", &resize(32, ResizeIntent::Update))
        .unwrap();
    assert_eq!(
        calls.lock().unwrap().inputs.last(),
        Some(&format!("{:?}", resize(32, ResizeIntent::Update)))
    );
}

#[test]
fn failed_resize_claim_does_not_steal_ownership() {
    let (mut service, _, calls) = fixture();
    let terminal = service.create(&request()).unwrap();
    service
        .input(&terminal.id, "a", &resize(30, ResizeIntent::Claim))
        .unwrap();
    calls.lock().unwrap().send_failure = true;
    assert_eq!(
        service.input(&terminal.id, "b", &resize(31, ResizeIntent::Claim)),
        Err(Error::Io)
    );
    calls.lock().unwrap().send_failure = false;
    service
        .input(&terminal.id, "b", &resize(32, ResizeIntent::Update))
        .unwrap();
    assert_eq!(calls.lock().unwrap().inputs.len(), 1);
    service
        .input(&terminal.id, "a", &resize(33, ResizeIntent::Update))
        .unwrap();
    assert_eq!(calls.lock().unwrap().inputs.len(), 2);
}

#[test]
fn invalid_resize_claim_keeps_the_previous_owner_and_dimensions() {
    let (mut service, _, calls) = fixture();
    let terminal = service.create(&request()).unwrap();
    service
        .input(&terminal.id, "a", &resize(30, ResizeIntent::Claim))
        .unwrap();
    assert_eq!(
        service.input(&terminal.id, "b", &resize(0, ResizeIntent::Claim)),
        Err(Error::Invalid)
    );
    service
        .input(&terminal.id, "a", &resize(31, ResizeIntent::Update))
        .unwrap();
    assert_eq!(calls.lock().unwrap().inputs.len(), 2);
}

#[test]
fn resize_ownership_is_independent_for_each_terminal() {
    let (mut service, _, calls) = fixture();
    let first = service.create(&request()).unwrap();
    let second = service.create(&request()).unwrap();
    service
        .input(&first.id, "a", &resize(30, ResizeIntent::Claim))
        .unwrap();
    service
        .input(&second.id, "b", &resize(30, ResizeIntent::Claim))
        .unwrap();
    service
        .input(&first.id, "b", &resize(31, ResizeIntent::Update))
        .unwrap();
    service
        .input(&second.id, "a", &resize(31, ResizeIntent::Update))
        .unwrap();
    assert_eq!(calls.lock().unwrap().inputs.len(), 2);
    service
        .input(&first.id, "a", &resize(32, ResizeIntent::Update))
        .unwrap();
    service
        .input(&second.id, "b", &resize(32, ResizeIntent::Update))
        .unwrap();
    assert_eq!(calls.lock().unwrap().inputs.len(), 4);
}

#[test]
fn launch_preserves_viewport_literal_arguments_and_workspace_environment() {
    let (mut service, _, calls) = fixture();
    let mut options = request();
    options.size = Size {
        rows: 37,
        cols: 111,
    };
    options.name = Some("开发服务".to_owned());
    options.args = vec![
        "$(touch never)".to_owned(),
        "a b; c".to_owned(),
        String::new(),
    ];
    let terminal = service.create(&options).unwrap();
    let calls = calls.lock().unwrap();
    let launch = &calls.launches[0];
    assert_eq!(launch.size, options.size);
    assert_eq!(launch.args, options.args);
    assert_eq!(launch.env["PASEO_WORKSPACE_ID"], "w");
    assert_eq!(launch.env["PASEO_TERMINAL_ID"], terminal.id);
    assert_eq!(terminal.name, "开发服务");
    assert_eq!(terminal.activity, None);
}

#[test]
fn generated_names_are_numbered_per_process_directory() {
    let (mut service, _, _) = fixture();
    let mut options = request();
    assert_eq!(service.create(&options).unwrap().name, "Terminal 1");
    assert_eq!(service.create(&options).unwrap().name, "Terminal 2");
    options.cwd = "/repo/other".to_owned();
    assert_eq!(service.create(&options).unwrap().name, "Terminal 1");
}

#[test]
fn manual_title_survives_later_osc_updates_without_changing_creation_name() {
    let (mut service, _, calls) = fixture();
    let terminal = service.create(&request()).unwrap();
    calls.lock().unwrap().title = Some("automatic".to_owned());
    assert_eq!(
        service.list(&ListRequest::default()).unwrap()[0]
            .title
            .as_deref(),
        Some("automatic")
    );
    service.rename(&terminal.id, "manual").unwrap();
    calls.lock().unwrap().title = Some("later automatic".to_owned());
    let listed = service.list(&ListRequest::default()).unwrap();
    assert_eq!(listed[0].title.as_deref(), Some("manual"));
    assert_eq!(listed[0].name, terminal.name);
}

#[test]
fn rename_enforces_utf16_code_units_without_rejecting_multibyte_titles() {
    let (mut service, _, _) = fixture();
    let terminal = service.create(&request()).unwrap();
    let title = "🦀".repeat(100);
    service.rename(&terminal.id, &title).unwrap();
    assert_eq!(
        service.rename(&terminal.id, &format!("{title}a")),
        Err(Error::Invalid)
    );
    assert_eq!(
        service.list(&ListRequest::default()).unwrap()[0]
            .title
            .as_deref(),
        Some(title.as_str())
    );
    service.rename(&terminal.id, &"界".repeat(200)).unwrap();
}

#[test]
fn list_and_capture_of_unknown_terminals_are_pure_reads() {
    let (mut service, _, calls) = fixture();
    assert!(service.list(&ListRequest::default()).unwrap().is_empty());
    assert!(service.capture("unknown").unwrap().is_empty());
    service.kill("unknown").unwrap();
    assert_eq!(
        service.input("unknown", "owner", &resize(30, ResizeIntent::Claim)),
        Err(Error::NotFound)
    );
    assert!(calls.lock().unwrap().launches.is_empty());
    assert_eq!(calls.lock().unwrap().killed, 0);
}
