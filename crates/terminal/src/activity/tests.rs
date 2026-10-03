use super::*;

#[test]
fn terminal_transitions_wake_workspace_subscribers() {
    let changes = model::changes::Changes::default();
    let mut receiver = changes.subscribe();
    let activities = Activities::default().with_changes(changes);
    activities.register("terminal".to_owned(), "workspace".to_owned());
    assert!(!receiver.has_changed().expect("sender remains available"));

    activities.report("terminal", ReportState::Running);
    assert!(receiver.has_changed().expect("sender remains available"));
    receiver.borrow_and_update();

    activities.report("terminal", ReportState::Running);
    assert!(!receiver.has_changed().expect("sender remains available"));

    activities.report("terminal", ReportState::Idle);
    assert!(receiver.has_changed().expect("sender remains available"));
}

#[test]
fn upstream_activity_transitions_preserve_finished_and_ignore_repeated_reports() {
    let working = transition(None, ReportState::Running, 10);
    assert_eq!(working.as_ref().unwrap().state, State::Working);
    assert_eq!(
        transition(working.clone(), ReportState::Running, 20),
        working
    );
    let finished = transition(working, ReportState::Idle, 30);
    assert_eq!(
        finished.as_ref().unwrap().attention_reason,
        Some(AttentionReason::Finished)
    );
    assert_eq!(
        transition(finished.clone(), ReportState::Idle, 40),
        finished
    );
    let needs_input = transition(finished, ReportState::NeedsInput, 50);
    assert_eq!(needs_input.as_ref().unwrap().state, State::Idle);
    assert_eq!(
        needs_input.as_ref().unwrap().attention_reason,
        Some(AttentionReason::NeedsInput)
    );
    let idle = transition(needs_input, ReportState::Idle, 60).unwrap();
    assert_eq!(idle.attention_reason, None);
    assert_eq!(idle.changed_at, 60);
}

#[test]
fn unknown_interrupt_and_attention_clearing_follow_upstream_tracker() {
    let activities = Activities::default();
    activities.register("one".into(), "workspace".into());
    assert!(activities.get("one").is_none());
    assert!(!activities.clear_attention("one"));
    activities.report("one", ReportState::Running);
    assert!(!activities.clear_attention("one"));
    activities.interrupt("one");
    assert!(activities.get("one").is_none());
    activities.report("one", ReportState::NeedsInput);
    let previous = activities.get("one");
    activities.interrupt("one");
    assert_eq!(activities.get("one"), previous);
    assert!(activities.clear_attention("one"));
    assert_eq!(activities.get("one").unwrap().attention_reason, None);
    assert!(!activities.clear_attention("one"));
    assert!(!activities.clear_attention("unknown"));
}

#[test]
fn workspace_projection_tracks_explicit_ownership_and_excludes_unknown_idle_removed() {
    let activities = Activities::default();
    activities.register("one".into(), "workspace-one".into());
    activities.register("two".into(), "workspace-two".into());
    assert!(activities.snapshot().unwrap().is_empty());
    for (report, expected) in [
        (ReportState::Running, WorkspaceStateBucket::Running),
        (ReportState::Idle, WorkspaceStateBucket::Attention),
        (ReportState::NeedsInput, WorkspaceStateBucket::NeedsInput),
    ] {
        activities.report("one", report);
        let snapshot = activities.snapshot().unwrap();
        assert_eq!(snapshot.len(), 1);
        assert_eq!(snapshot[0].workspace_id, "workspace-one");
        assert_eq!(snapshot[0].bucket, expected);
        assert!(snapshot[0].changed_at.as_ref().unwrap().ends_with('Z'));
    }
    activities.clear_attention("one");
    assert!(activities.snapshot().unwrap().is_empty());
    activities.report("two", ReportState::Running);
    activities.remove("two");
    assert!(activities.snapshot().unwrap().is_empty());
}
