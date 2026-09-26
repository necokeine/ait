//! Host payload variants used by Paseo browser-tools/tools.test.ts.
use super::*;

fn succeeds(name: &str, fields: Value) -> Value {
    let mut result = json!({"command":name,"browserId":TAB});
    let Value::Object(fields) = fields else {
        panic!("result fields must be an object");
    };
    result.as_object_mut().unwrap().extend(fields);
    json!({"ok":true,"result":result})
}

#[test]
fn snapshot_preserves_yaml_stats_and_rejects_incomplete_stats() {
    let value = succeeds(
        "snapshot",
        json!({"url":"https://example.com","title":"Example",
        "format":"aria-yaml","snapshot":"- button Save [ref=@e1]","truncated":false,
        "stats":{"nodeCount":1,"refCount":1,"textLength":28,"iframeCount":0,"maxDepth":2}}),
    );
    assert!(response(&value, "snapshot"));
    for key in ["nodeCount", "refCount", "textLength"] {
        let mut missing = value.clone();
        missing["result"]["stats"]
            .as_object_mut()
            .unwrap()
            .remove(key);
        assert!(!response(&missing, "snapshot"));
    }
    for patch in [
        json!({"format":"html"}),
        json!({"truncated":"false"}),
        json!({"stats":{"nodeCount":-1,"refCount":0,"textLength":0}}),
        json!({"stats":{"nodeCount":1,"refCount":0,"textLength":0,"arbitrary":1}}),
    ] {
        let mut invalid = value.clone();
        invalid["result"]
            .as_object_mut()
            .unwrap()
            .extend(patch.as_object().unwrap().clone());
        assert!(!response(&invalid, "snapshot"));
    }
}

#[test]
fn screenshots_require_png_data_and_unsigned_dimensions() {
    let good = succeeds(
        "screenshot",
        json!({"mimeType":"image/png","dataBase64":"aGVsbG8=",
        "width":800,"height":600}),
    );
    assert!(response(&good, "screenshot"));
    for (key, value) in [
        ("mimeType", json!("image/jpeg")),
        ("dataBase64", json!("")),
        ("width", json!(-1)),
        ("height", json!(1.5)),
    ] {
        let mut bad = good.clone();
        bad["result"][key] = value;
        assert!(!response(&bad, "screenshot"));
    }
}

#[test]
fn drag_requires_both_references_and_accepts_numeric_coordinates() {
    let mut value = succeeds(
        "drag",
        json!({"sourceRef":"@e1","targetRef":"@e2",
        "sourceX":0.5,"sourceY":-1,"targetX":400,"targetY":300}),
    );
    assert!(response(&value, "drag"));
    value["result"]["targetRef"] = json!("e2");
    assert!(!response(&value, "drag"));
    value["result"]["targetRef"] = json!("@e2");
    value["result"]["sourceX"] = json!("0.5");
    assert!(!response(&value, "drag"));
}

#[test]
fn upload_requires_at_least_one_nonempty_path_and_an_element_reference() {
    let good = succeeds(
        "upload",
        json!({"ref":"@e1","filePaths":["/tmp/a", "/tmp/b"]}),
    );
    assert!(response(&good, "upload"));
    for paths in [
        json!([]),
        json!([""]),
        json!(["/tmp/a", 7]),
        json!("/tmp/a"),
    ] {
        let mut bad = good.clone();
        bad["result"]["filePaths"] = paths;
        assert!(!response(&bad, "upload"));
    }
}

#[test]
fn interaction_results_validate_their_command_specific_payloads() {
    for (name, fields) in [
        ("click", json!({"ref":"@e1"})),
        ("fill", json!({"ref":"@e1"})),
        ("hover", json!({"ref":"@e1"})),
        ("select", json!({"ref":"@e1","value":""})),
        ("wait", json!({"matched":"url"})),
        ("keypress", json!({"key":"Enter"})),
        ("navigate", json!({"url":"https://example.com"})),
        ("evaluate", json!({"resultJson":"null","truncated":false})),
        ("scroll", json!({"deltaX":-0.5,"deltaY":1})),
        ("resize", json!({"width":800,"height":600})),
    ] {
        assert!(response(&succeeds(name, fields), name), "{name}");
        assert!(!response(&succeeds(name, json!({})), name), "{name}");
    }
}

#[test]
fn logs_accept_optional_network_and_console_metadata_but_reject_wrong_types() {
    let good = succeeds(
        "logs",
        json!({"console":[{"level":"warning","message":"notice",
        "timestamp":1.5,"source":"page.js","line":-1}],
        "network":[{"url":"https://example.com","startTime":1,"duration":0.5,
        "method":"GET","type":"fetch","status":200,"transferSize":123}]}),
    );
    assert!(response(&good, "logs"));
    for (section, key, invalid) in [
        ("console", "source", json!(false)),
        ("console", "line", json!(1.5)),
        ("network", "method", json!(7)),
        ("network", "type", json!(null)),
        ("network", "status", json!("200")),
        ("network", "transferSize", json!("123")),
    ] {
        let mut bad = good.clone();
        bad["result"][section][0][key] = invalid;
        assert!(!response(&bad, "logs"), "{section}.{key}");
    }
}

#[test]
fn dialogs_validate_actions_types_and_optional_prompt_fields() {
    let mut value = succeeds("reload", json!({}));
    for kind in ["alert", "confirm", "prompt", "beforeunload"] {
        for action in ["accepted", "dismissed"] {
            value["dialogs"] = json!([{"type":kind,"message":"","action":action,
                "timestamp":1.25,"defaultValue":"default","promptText":"text"}]);
            assert!(response(&value, "reload"));
        }
    }
    for (key, invalid) in [
        ("type", json!("dialog")),
        ("action", json!("ignored")),
        ("timestamp", json!("now")),
        ("defaultValue", json!(false)),
        ("promptText", json!(null)),
    ] {
        let mut bad = value.clone();
        bad["dialogs"][0][key] = invalid;
        assert!(!response(&bad, "reload"));
    }
}

#[test]
fn typed_failures_reject_unknown_codes_and_apply_retryable_default() {
    for code in [
        "browser_disabled",
        "browser_no_host",
        "browser_tab_not_found",
        "browser_tab_closed",
        "browser_timeout",
        "screenshot_no_frame",
        "browser_denied",
        "browser_unsupported",
        "browser_stale_ref",
        "browser_unknown_error",
    ] {
        let mut value = json!({"ok":false,"error":{"code":code,"message":"Failure"}});
        assert!(response(&value, "snapshot"));
        defaults(&mut value);
        assert_eq!(value["error"]["retryable"], false);
    }
    for error in [
        json!({"code":"arbitrary","message":"Failure"}),
        json!({"code":"browser_denied","message":""}),
        json!({"code":"browser_denied","message":"Denied","retryable":1}),
    ] {
        assert!(!response(&json!({"ok":false,"error":error}), "snapshot"));
    }
}

#[test]
fn tab_defaults_preserve_explicit_state_and_allow_legacy_ids() {
    let mut value = json!({"ok":true,"result":{"command":"list_tabs","tabs":[
        {"browserId":TAB,"url":"about:blank","title":"","isActive":true,"isLoading":true},
        {"browserId":"1700000000000-abc","url":"","title":""}]}});
    assert!(response(&value, "list_tabs"));
    defaults(&mut value);
    assert_eq!(value["result"]["tabs"][0]["isActive"], true);
    assert_eq!(value["result"]["tabs"][0]["isLoading"], true);
    assert_eq!(value["result"]["tabs"][1]["isActive"], false);
    assert_eq!(value["result"]["tabs"][1]["isLoading"], false);
}
