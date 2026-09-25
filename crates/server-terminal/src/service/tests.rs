use super::*;
use crate::protocol::{Resize, Size};
use crate::test_support::{fixture, request};

#[test]
fn placement_filter_rename_capture_kill_and_shutdown_have_real_effects() {
    let (mut service, registry, calls) = fixture();
    let terminal = service.create(&request()).unwrap();
    assert_eq!(terminal.workspace_id, "w");
    assert_eq!(
        calls.lock().unwrap().launches[0].env["PASEO_TERMINAL_ID"],
        terminal.id
    );
    assert_eq!(service.list(&ListRequest::default()).unwrap().len(), 1);
    assert_eq!(
        service
            .list(&ListRequest {
                cwd: Some("/repo".to_owned()),
                workspace_id: None
            })
            .unwrap()
            .len(),
        1
    );
    assert!(
        service
            .list(&ListRequest {
                cwd: Some("/elsewhere".to_owned()),
                workspace_id: None
            })
            .unwrap()
            .is_empty()
    );
    assert_eq!(
        service
            .list(&ListRequest {
                cwd: Some("/elsewhere".to_owned()),
                workspace_id: Some("w".to_owned())
            })
            .unwrap()
            .len(),
        1
    );
    service.rename(&terminal.id, " build ").unwrap();
    assert_eq!(
        service.list(&ListRequest::default()).unwrap()[0]
            .title
            .as_deref(),
        Some("build")
    );
    for title in ["", "   ", "bad\nname", &"x".repeat(201)] {
        assert_eq!(service.rename(&terminal.id, title), Err(Error::Invalid));
    }
    assert_eq!(service.capture(&terminal.id).unwrap().len(), 3);
    assert!(service.capture("missing").unwrap().is_empty());
    assert!(service.observe(&terminal.id, None, None).is_ok());
    assert!(matches!(
        service.observe("missing", None, None),
        Err(Error::NotFound)
    ));
    service.kill(&terminal.id).unwrap();
    service.kill(&terminal.id).unwrap();
    assert_eq!(calls.lock().unwrap().killed, 1);
    let _ = service.create(&request()).unwrap();
    registry.workspaces.lock().unwrap()[0].archived_at = Some("now".to_owned());
    service.reconcile().unwrap();
    assert_eq!(calls.lock().unwrap().killed, 2);
    service.shutdown().unwrap();
}

#[test]
fn resize_owner_claim_update_and_non_owner_input_are_distinct() {
    let (mut service, _, calls) = fixture();
    let terminal = service.create(&request()).unwrap();
    let update = Input::Resize(Resize {
        size: Size::default(),
        intent: ResizeIntent::Update,
    });
    service.input(&terminal.id, "a", &update).unwrap();
    assert!(calls.lock().unwrap().inputs.is_empty());
    service
        .input(
            &terminal.id,
            "a",
            &Input::Resize(Resize {
                size: Size::default(),
                intent: ResizeIntent::Claim,
            }),
        )
        .unwrap();
    service.input(&terminal.id, "b", &update).unwrap();
    service.input(&terminal.id, "a", &update).unwrap();
    service
        .input(
            &terminal.id,
            "b",
            &Input::Input {
                data: "text".to_owned(),
            },
        )
        .unwrap();
    assert_eq!(calls.lock().unwrap().inputs.len(), 3);
    calls.lock().unwrap().exited = true;
    assert_eq!(
        service.input(&terminal.id, "a", &update),
        Err(Error::NotFound)
    );
    assert_eq!(service.rename(&terminal.id, "name"), Err(Error::NotFound));
    assert!(service.list(&ListRequest::default()).unwrap().is_empty());
}

#[test]
fn deepest_workspace_legacy_placement_and_explicit_scope_are_checked() {
    let (mut service, registry, _) = fixture();
    let mut child = registry.workspaces.lock().unwrap()[0].clone();
    child.workspace_id = "nested".to_owned();
    child.cwd = "/repo/sub".to_owned();
    registry.workspaces.lock().unwrap().push(child);
    assert_eq!(service.create(&request()).unwrap().workspace_id, "nested");
    assert!(
        service
            .list(&ListRequest {
                cwd: Some("/repo".to_owned()),
                workspace_id: None
            })
            .unwrap()
            .is_empty()
    );
    let mut request = request();
    request.workspace_id = Some("w".to_owned());
    assert_eq!(service.create(&request).unwrap().workspace_id, "w");
    request.cwd = "/outside".to_owned();
    assert_eq!(service.create(&request), Err(Error::Invalid));
    request.workspace_id = None;
    assert_eq!(service.create(&request), Err(Error::WorkspaceNotFound));
    registry.projects.lock().unwrap()[0].archived_at = Some("now".to_owned());
    assert_eq!(
        service.create(&crate::test_support::request()),
        Err(Error::WorkspaceNotFound)
    );
}

#[test]
fn limits_and_failures_do_not_report_false_success_or_lose_retryable_entries() {
    let (mut service, registry, calls) = fixture();
    let mut request = request();
    request.agent_id = Some("agent".to_owned());
    assert_eq!(service.create(&request), Err(Error::Invalid));
    request.agent_id = None;
    calls.lock().unwrap().failure = true;
    assert_eq!(service.create(&request), Err(Error::Io));
    calls.lock().unwrap().failure = false;
    for _ in 0..MAX_TERMINALS {
        service.create(&request).unwrap();
    }
    assert_eq!(service.create(&request), Err(Error::Exhausted));
    calls.lock().unwrap().failure = true;
    assert_eq!(service.shutdown(), Err(Error::Io));
    assert_eq!(service.entries.len(), MAX_TERMINALS);
    calls.lock().unwrap().failure = false;
    calls.lock().unwrap().exited = true;
    service.create(&request).unwrap();
    assert_eq!(service.entries.len(), 1);
    *registry.failure.lock().unwrap() = true;
    assert_eq!(service.reconcile(), Err(Error::Registry));
    *registry.failure.lock().unwrap() = false;
    service.shutdown().unwrap();
}

#[test]
fn closed_terminal_keeps_tail_only_for_existing_observers() {
    let (mut service, _, _) = fixture();
    let terminal = service.create(&request()).unwrap();
    service.kill(&terminal.id).unwrap();
    assert!(service.capture(&terminal.id).unwrap().is_empty());
    assert!(matches!(
        service.observe(&terminal.id, None, None),
        Err(Error::NotFound)
    ));
    assert!(service.observe(&terminal.id, Some(0), None).unwrap().exited);
    assert!(service.list(&ListRequest::default()).unwrap().is_empty());
    assert_eq!(service.rename(&terminal.id, "closed"), Err(Error::NotFound));
    service.shutdown().unwrap();
    assert!(service.entries.is_empty());
}
