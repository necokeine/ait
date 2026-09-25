use super::*;
const TAB: &str = "11111111-1111-4111-8111-111111111111";
#[test]
fn all_twenty_two_commands_validate_and_apply_defaults() {
    for name in COMMANDS {
        let args = match *name {
            "list_tabs" | "new_tab" => json!({}),
            "navigate" => json!({"browserId":TAB,"url":"https://example.com"}),
            "click" | "hover" => json!({"browserId":TAB,"ref":"@e1"}),
            "fill" | "select" => json!({"browserId":TAB,"ref":"@e1","value":""}),
            "wait" => json!({"browserId":TAB,"text":"ready"}),
            "type" => json!({"browserId":TAB,"text":""}),
            "keypress" => json!({"browserId":TAB,"key":"Enter"}),
            "upload" => json!({"browserId":TAB,"ref":"@e1","filePaths":["/tmp/file"]}),
            "drag" => json!({"browserId":TAB,"sourceRef":"@e1","targetRef":"@e2"}),
            "evaluate" => json!({"browserId":TAB,"function":"() => 1"}),
            "scroll" => json!({"browserId":TAB,"deltaX":0,"deltaY":4}),
            "resize" => json!({"browserId":TAB,"width":800,"height":600}),
            _ => json!({"browserId":TAB}),
        };
        let value = command(json!({"command":name,"args":args})).unwrap();
        if *name == "click" {
            assert_eq!(value["args"]["button"], "left");
            assert_eq!(value["args"]["doubleClick"], false);
        }
        if *name == "logs" {
            assert_eq!(value["args"]["maxEntries"], 50);
        }
    }
    assert!(command(json!({"command":"new_tab"})).is_ok());
}
#[test]
fn rejects_invalid_targets_unknown_arguments_and_bad_values() {
    assert!(browser_id(TAB));
    assert!(browser_id("1700000000000-a1"));
    assert!(!browser_id("11111111111141118111111111111111"));
    assert!(!browser_id("0"));
    for value in [
        json!({"command":"unknown"}),
        json!({"command":"new_tab","args":{"url":"file:///tmp"}}),
        json!({"command":"click","args":{"browserId":TAB,"ref":"e1"}}),
        json!({"command":"wait","args":{"browserId":TAB,"text":"x","url":"y"}}),
        json!({"command":"wait","args":{"browserId":TAB,"text":"x","url":7}}),
        json!({"command":"snapshot","args":{"browserId":TAB,"surprise":true}}),
        json!({"command":"logs","args":{"browserId":TAB,"maxEntries":201}}),
        json!({"command":"screenshot","args":{"browserId":TAB,"fullPage":"yes"}}),
        json!({"command":"resize","args":{"browserId":TAB,"width":0,"height":1}}),
    ] {
        assert!(command(value.clone()).is_err(), "{value}");
    }
}
#[test]
fn response_checks_command_nested_logs_dialogs_and_optional_fields() {
    let good = json!({"ok":true,"result":{"command":"logs","browserId":TAB,"console":[{"level":"info","message":"done","timestamp":1}],"network":[]},"dialogs":[{"type":"confirm","message":"ok?","action":"dismissed","timestamp":1}]});
    assert!(response(&good, "logs"));
    assert!(!response(&good, "snapshot"));
    for patch in [
        json!({"console":[{}]}),
        json!({"network":[{"url":"url"}]}),
        json!({"x":"not-number"}),
        json!({"workspaceId":""}),
    ] {
        let mut bad = good.clone();
        bad["result"]
            .as_object_mut()
            .unwrap()
            .extend(patch.as_object().unwrap().clone());
        assert!(!response(&bad, "logs"));
    }
    let mut bad = good.clone();
    bad["dialogs"] = json!([{}]);
    assert!(!response(&bad, "logs"));
    let mut failure = json!({"ok":false,"error":{"code":"browser_denied","message":"denied"}});
    assert!(response(&failure, "logs"));
    defaults(&mut failure);
    assert_eq!(failure["error"]["retryable"], false);
}
