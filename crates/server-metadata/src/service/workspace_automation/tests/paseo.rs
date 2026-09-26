//! Paseo automation gate and workspace-scripts admission invariants.

use super::*;

#[test]
fn untrusted_workspace_does_not_start_automatic_setup() {
    let (service, _, runtime) = service();
    assert!(!service.start_created_setup("blocked").unwrap());
    assert!(runtime.setups.lock().unwrap().is_empty());
    assert!(matches!(
        service.setup_status("blocked").unwrap(),
        SetupStatus::Blocked { .. }
    ));
}

#[test]
fn trusted_workspace_starts_created_setup_with_saved_placement() {
    let (service, workspaces, runtime) = service();
    workspaces
        .update("trusted", &|record| {
            let mut next = record.clone();
            next.cwd = "/checkout/packages/server".to_owned();
            next.worktree_root = Some("/checkout".to_owned());
            next.main_repo_root = Some("/repository".to_owned());
            next.branch = Some("feature/setup".to_owned());
            next
        })
        .unwrap();
    assert!(service.start_created_setup("trusted").unwrap());
    let setups = runtime.setups.lock().unwrap();
    assert_eq!(setups[0].cwd, "/checkout/packages/server");
    assert_eq!(setups[0].worktree_path, "/checkout");
    assert_eq!(setups[0].repo_root, "/repository");
    assert_eq!(setups[0].branch_name, "feature/setup");
}

#[test]
fn archived_workspace_cannot_be_approved_or_start_created_setup() {
    let (service, _, runtime) = service();
    assert!(
        service
            .approve_and_start_setup("archived", "later")
            .is_err()
    );
    assert!(service.start_created_setup("archived").is_err());
    assert!(runtime.setups.lock().unwrap().is_empty());
}

#[test]
fn unknown_workspace_script_operations_do_not_reach_the_runtime() {
    let (service, _, runtime) = service();
    assert!(service.list_scripts("missing").is_err());
    assert!(service.start_script("missing", "web").is_err());
    assert!(service.stop_script("missing", "web").is_err());
    assert!(runtime.scripts.lock().unwrap().is_empty());
}

#[test]
fn untrusted_workspace_can_inspect_and_stop_but_cannot_start_scripts() {
    let (service, _, runtime) = service();
    assert_eq!(service.list_scripts("blocked").unwrap().len(), 1);
    assert!(!service.stop_script("blocked", "web").unwrap().running);
    assert!(service.start_script("blocked", "web").is_err());
    assert!(runtime.scripts.lock().unwrap().is_empty());
}

#[test]
fn approval_preserves_workspace_title_labels_and_pin() {
    let (service, workspaces, runtime) = service();
    workspaces
        .update("blocked", &|record| {
            let mut next = record.clone();
            next.title = Some("Review".to_owned());
            next.labels = Some(vec!["Urgent".to_owned()]);
            next.pinned_at = Some("pinned".to_owned());
            next
        })
        .unwrap();
    service
        .approve_and_start_setup("blocked", "approved")
        .unwrap();
    let record = workspaces.get("blocked").unwrap().unwrap();
    assert_eq!(record.title.as_deref(), Some("Review"));
    assert_eq!(record.labels, Some(vec!["Urgent".to_owned()]));
    assert_eq!(record.pinned_at.as_deref(), Some("pinned"));
    assert!(record.untrusted_source.is_none());
    assert_eq!(runtime.setups.lock().unwrap().len(), 1);
}
