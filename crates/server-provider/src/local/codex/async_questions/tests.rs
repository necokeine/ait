use super::*;

fn item() -> Value {
    json!({"type":"agentMessage","id":"question","delivery":"async","questions":[
        {"title":"Which runtime?","options":["Rust","Python"]},{"title":"Any constraints?","options":null}]})
}

#[test]
fn pending_survives_restart_and_answers_validate_before_completion() {
    let mut questions = Questions::default();
    let request = questions.receive(&item()).unwrap().unwrap();
    assert_eq!(request["input"]["questions"][0]["header"], "Question 1");
    assert!(questions.receive(&item()).unwrap().is_none());
    let mut restored = Questions::restore(questions.saved().as_ref()).unwrap();
    assert_eq!(restored.pending(), questions.pending());
    let id = request["id"].as_str().unwrap();
    assert!(
        restored
            .prepare(
                id,
                &json!({"behavior":"allow","updatedInput":{"answers":{"Question 1":"Rust"}}})
            )
            .is_err()
    );
    let response = json!({"behavior":"allow","updatedInput":{"answers":{"Question 1":" Rust ","Question 2":"No unsafe"}}});
    let prompt = restored.prepare(id, &response).unwrap().unwrap();
    assert_eq!(
        prompt.text,
        "Answers to your questions:\n\nWhich runtime?\nRust\n\nAny constraints?\nNo unsafe"
    );
    assert_eq!(
        prompt.client_message_id.as_deref(),
        Some(format!("async-answer:{:x}", Sha256::digest(id)).as_str())
    );
    let entry = restored.resolve(id, &response).unwrap();
    assert!(
        entry.item["detail"]["text"]
            .as_str()
            .unwrap()
            .contains("No unsafe")
    );
    assert!(restored.pending().is_empty());
    assert!(restored.prepare(id, &response).is_err());
    let restored = Questions::restore(restored.saved().as_ref()).unwrap();
    let mut history = vec![NativeItem {
        key: "native:turn:question".into(),
        turn_id: Some("turn".into()),
        timestamp: super::super::discovery::timestamp(),
        item: timeline(&item()).unwrap(),
    }];
    restored.history(&mut history).unwrap();
    assert_eq!(history[1].item, entry.item);
}

#[test]
fn dismissals_malformed_native_data_and_metadata_bounds_are_explicit() {
    let mut questions = Questions::default();
    questions.receive(&item()).unwrap();
    assert!(
        questions
            .prepare("permission-question", &json!({"behavior":"deny"}))
            .unwrap()
            .is_none()
    );
    questions
        .resolve("permission-question", &json!({"behavior":"deny"}))
        .unwrap();
    assert!(
        Questions::restore(questions.saved().as_ref())
            .unwrap()
            .pending()
            .is_empty()
    );
    let mut conflicting = item();
    conflicting["questions"][0]["title"] = json!("changed");
    assert!(questions.receive(&conflicting).is_err());
    for bad in [json!({}), json!([{"item":item(),"resolution":[1,2]}])] {
        assert!(Questions::restore(Some(&bad)).is_err());
    }
    let mut questions = Questions::default();
    for index in 0..32 {
        let mut item = item();
        item["id"] = json!(index.to_string());
        questions.receive(&item).unwrap();
    }
    assert!(questions.receive(&item()).is_err());
}
