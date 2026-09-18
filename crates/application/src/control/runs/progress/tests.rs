use super::*;

#[test]
fn live_projection_limits_text_on_a_utf8_boundary() {
    let oversized = "界".repeat(MAX_LIVE_MESSAGE_BYTES / 3 + 2);
    let bounded = bounded(oversized);

    assert!(bounded.len() <= MAX_LIVE_MESSAGE_BYTES);
    assert!(bounded.is_char_boundary(bounded.len()));
    assert_eq!(bounded_to("界".into(), 2), "");
}

#[test]
fn live_projection_evicts_the_oldest_item() {
    let mut projection = ProgressProjection::default();
    for index in 0..=MAX_PROJECTED_ITEMS {
        let id = format!("item-{index}");
        projection.remember(&id);
        projection.items.insert(
            id,
            ProjectedItem::Message {
                phase: None,
                text: String::new(),
            },
        );
    }

    assert_eq!(projection.order.len(), MAX_PROJECTED_ITEMS);
    assert!(!projection.seen.contains("item-0"));
    assert!(!projection.items.contains_key("item-0"));
    assert_eq!(projection.order.first().map(String::as_str), Some("item-1"));
}
