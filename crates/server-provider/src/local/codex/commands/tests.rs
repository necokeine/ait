use super::*;

#[test]
fn command_parsing_never_interprets_unrelated_text_as_a_native_mutation() {
    assert!(out_of_band(" /goal pause "));
    assert!(!out_of_band("discuss /goal pause"));
    assert!(!out_of_band("/folder/goal pause"));
    assert_eq!(
        goal("thread", "PAUSE").1,
        json!({"threadId":"thread","status":"paused"})
    );
    assert_eq!(goal("thread", "resume").1["status"], "active");
    assert_eq!(goal("thread", "clear").0, Some("thread/goal/clear"));
    assert_eq!(
        goal("thread", "ship feature").1["objective"],
        "ship feature"
    );
    assert!(goal("thread", "").0.is_none());
}
