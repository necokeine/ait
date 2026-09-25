use super::*;

fn pending(kind: Kind) -> Pending {
    Pending {
        native_id: json!("native"),
        kind,
        request: json!({"input":{"questions":[{"id":"choice"}]}}),
    }
}

#[test]
fn approvals_reject_policy_amendments_mismatched_actions_and_unrequested_answers() {
    let command = pending(Kind::Command);
    assert_eq!(
        resolution(&command, &json!({"behavior":"allow"})).unwrap(),
        json!({"decision":"accept"})
    );
    assert_eq!(
        resolution(&command, &json!({"behavior":"deny","interrupt":true})).unwrap(),
        json!({"decision":"decline"})
    );
    for response in [
        json!({"behavior":"allow","updatedPermissions":[]}),
        json!({"behavior":"allow","updatedInput":{}}),
        json!({"behavior":"allow","selectedActionId":"deny"}),
        json!({"behavior":"deny","interrupt":"yes"}),
        json!({"behavior":"other"}),
    ] {
        assert!(resolution(&command, &response).is_err());
    }
    let mut file = pending(Kind::File);
    file.request["input"]["grantRoot"] = json!("/tmp");
    assert!(resolution(&file, &json!({"behavior":"allow"})).is_err());
    assert!(resolution(&file, &json!({"behavior":"deny"})).is_ok());
    let question = pending(Kind::Question);
    assert_eq!(
        resolution(
            &question,
            &json!({"behavior":"allow","updatedInput":{"answers":{"choice":"first"}}})
        )
        .unwrap(),
        json!({"answers":{"choice":{"answers":["first"]}}})
    );
    assert_eq!(
        resolution(&question, &json!({"behavior":"deny"})).unwrap(),
        json!({"answers":{}})
    );
    for answers in [
        json!({}),
        json!({"unknown":"first"}),
        json!({"choice":1}),
        json!({"choice":[]}),
        json!({"choice":[false]}),
    ] {
        assert!(
            resolution(
                &question,
                &json!({"behavior":"allow","updatedInput":{"answers":answers}})
            )
            .is_err()
        );
    }
    assert!(question_ids(&json!([{"id":"same"},{"id":"same"}])).is_err());
    assert!(!supported("item/permissions/requestApproval"));
}
