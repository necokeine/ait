use super::Changes;

#[test]
fn subscribers_observe_committed_notifications() {
    let changes = Changes::default();
    let mut receiver = changes.subscribe();
    assert!(!receiver.has_changed().expect("sender remains available"));

    changes.notify();
    assert!(receiver.has_changed().expect("sender remains available"));
    receiver.borrow_and_update();
    assert!(!receiver.has_changed().expect("sender remains available"));
}
