use super::*;
use crate::test_support::fixture;

#[test]
fn rpc_projects_success_failure_and_inclusive_negative_capture_ranges() {
    let (mut service, _, _) = fixture();
    let created = execute(
        &mut service,
        "terminal.create.request",
        json!({"cwd":"/repo"}),
    )
    .unwrap();
    let id = created["terminal"]["id"].clone();
    assert!(created["error"].is_null());
    let listed = execute(
        &mut service,
        "terminal.list.request",
        json!({"cwd":"/repo"}),
    )
    .unwrap();
    assert_eq!(listed["terminals"][0]["id"], id);
    let renamed = execute(
        &mut service,
        "terminal.rename.request",
        json!({"terminalId":id,"title":"new"}),
    )
    .unwrap();
    assert_eq!(renamed["success"], true);
    let invalid = execute(
        &mut service,
        "terminal.rename.request",
        json!({"terminalId":id,"title":""}),
    )
    .unwrap();
    assert_eq!(invalid["success"], false);
    let tail = execute(
        &mut service,
        "terminal.capture.request",
        json!({"terminalId":id,"start":-2,"end":-1,"stripAnsi":false}),
    )
    .unwrap();
    assert_eq!(tail["lines"], json!(["second", "last"]));
    assert_eq!(tail["totalLines"], 3);
    let reversed = execute(
        &mut service,
        "terminal.capture.request",
        json!({"terminalId":id,"start":2,"end":0}),
    )
    .unwrap();
    assert_eq!(reversed["lines"], json!([]));
    let killed = execute(
        &mut service,
        "terminal.kill.request",
        json!({"terminalId":id}),
    )
    .unwrap();
    assert_eq!(killed["success"], true);
    let missing = execute(
        &mut service,
        "terminal.capture.request",
        json!({"terminalId":id}),
    )
    .unwrap();
    assert_eq!(missing["totalLines"], 0);
    assert_eq!(
        execute(&mut service, "unknown", json!({})),
        Err(Error::MethodNotFound)
    );
    assert_eq!(
        execute(&mut service, "terminal.create.request", json!({})),
        Err(Error::Invalid)
    );
    assert!(
        execute(
            &mut service,
            "terminal.create.request",
            json!({"cwd":"relative"})
        )
        .unwrap()["terminal"]
            .is_null()
    );
    assert_eq!(index(Some(i64::MIN), 3, 0), 0);
    assert_eq!(index(Some(i64::MAX), 3, 0), 2);
}
