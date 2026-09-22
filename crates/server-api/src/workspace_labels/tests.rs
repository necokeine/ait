use server_application::workspace_labels::{
    SequencedWorkspaceLabelChange, WorkspaceLabelChange, WorkspaceLabelError,
};
use server_domain::workspace_labels::{WorkspaceLabelColor, WorkspaceLabelDefinition};
use server_protocol::ErrorCode;

use crate::outbound::Outbound;

use super::{Delivery, live_update, map_error};

fn change(name: &str, seq: u64) -> SequencedWorkspaceLabelChange {
    SequencedWorkspaceLabelChange {
        generation: "generation-one".to_owned(),
        seq,
        change: WorkspaceLabelChange::Upsert {
            label: WorkspaceLabelDefinition {
                name: name.to_owned(),
                color: WorkspaceLabelColor::Red,
            },
            previous_name: None,
        },
    }
}

#[test]
fn label_business_errors_keep_paseo_machine_codes() {
    assert_eq!(
        map_error(WorkspaceLabelError::NameEmpty),
        ErrorCode::LabelNameEmpty
    );
    assert_eq!(
        map_error(WorkspaceLabelError::LabelNotFound),
        ErrorCode::LabelNotFound
    );
    assert_eq!(
        map_error(WorkspaceLabelError::NameTaken),
        ErrorCode::LabelNameTaken
    );
    assert_eq!(
        map_error(WorkspaceLabelError::StorageUncertain),
        ErrorCode::WorkspaceLabelStorageUncertain
    );
}

#[test]
fn live_update_uses_the_connection_subscription_and_sequence() {
    let update = live_update(
        "subscription-one",
        &SequencedWorkspaceLabelChange {
            generation: "generation-one".to_owned(),
            seq: 3,
            change: WorkspaceLabelChange::Upsert {
                label: WorkspaceLabelDefinition {
                    name: "Urgent".to_owned(),
                    color: WorkspaceLabelColor::Red,
                },
                previous_name: Some("Blocked".to_owned()),
            },
        },
    );
    let json = serde_json::to_value(update).unwrap();
    assert_eq!(json["subscriptionId"], "subscription-one");
    assert_eq!(json["generation"], "generation-one");
    assert_eq!(json["previousName"], "Blocked");
    assert_eq!(json["seq"], 3);

    let (outbound, _receiver) = Outbound::new();
    let mut delivery = Delivery {
        ready: false,
        pending: Vec::new(),
        outbound,
        subscription_id: "subscription-one".to_owned(),
    };
    delivery.accept(change("A", 1));
    delivery.accept(change("B", 2));
    delivery.accept(change("A", 3));
    let buffered = delivery
        .pending
        .iter()
        .map(|(key, change)| (key.as_str(), change.seq))
        .collect::<Vec<_>>();
    assert_eq!(buffered, vec![("a", 3), ("b", 2)]);
}
