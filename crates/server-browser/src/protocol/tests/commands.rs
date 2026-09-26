//! Input variants from Paseo browser-tools/tools.test.ts, at the Rust protocol boundary.
use super::*;

#[test]
fn click_options_preserve_explicit_button_modifiers_and_double_click() {
    let args = json!({"browserId":TAB,"ref":"@e2","button":"right",
        "doubleClick":true,"modifiers":["Control","Shift"]});
    let value = command(json!({"command":"click","args":args})).unwrap();
    assert_eq!(value["args"], args);
    for (key, invalid) in [
        ("button", json!("primary")),
        ("doubleClick", json!(1)),
        ("modifiers", json!(["ctrl"])),
        ("modifiers", json!("Alt")),
    ] {
        let mut bad = value.clone();
        bad["args"][key] = invalid;
        assert!(command(bad).is_err());
    }
}

#[test]
fn wait_requires_exactly_one_condition_and_a_bounded_positive_timeout() {
    for condition in [json!({"text":"ready"}), json!({"url":"**/done"})] {
        let mut args = condition;
        args["browserId"] = json!(TAB);
        args["timeoutMs"] = json!(30_000);
        assert!(command(json!({"command":"wait","args":args})).is_ok());
        for timeout in [json!(0), json!(-1), json!(30_001), json!(1.5), json!("100")] {
            args["timeoutMs"] = timeout;
            assert!(command(json!({"command":"wait","args":args})).is_err());
        }
    }
    for args in [
        json!({"browserId":TAB}),
        json!({"browserId":TAB,"text":""}),
        json!({"browserId":TAB,"text":"ready","url":"**/done"}),
    ] {
        assert!(command(json!({"command":"wait","args":args})).is_err());
    }
}

#[test]
fn navigation_rejects_non_web_schemes_but_accepts_local_http_hosts() {
    for name in ["new_tab", "navigate"] {
        for url in [
            "https://example.com/path?q=1",
            "http://localhost:3000",
            "http://127.0.0.1",
        ] {
            let mut args = json!({"url":url});
            if name == "navigate" {
                args["browserId"] = json!(TAB);
            }
            assert!(command(json!({"command":name,"args":args})).is_ok());
        }
        for url in [
            "file:///tmp/page",
            "javascript:alert(1)",
            "data:text/html,hi",
            "about:blank",
            "/relative",
        ] {
            let mut args = json!({"url":url});
            if name == "navigate" {
                args["browserId"] = json!(TAB);
            }
            assert!(command(json!({"command":name,"args":args})).is_err());
        }
    }
}

#[test]
fn optional_element_reference_is_validated_for_keyboard_evaluation_and_scroll() {
    for (name, fields) in [
        ("type", json!({"text":""})),
        ("keypress", json!({"key":"Enter"})),
        ("evaluate", json!({"function":"() => 1"})),
        ("scroll", json!({"deltaX":-1,"deltaY":0})),
    ] {
        let mut args = fields;
        args["browserId"] = json!(TAB);
        assert!(command(json!({"command":name,"args":args})).is_ok());
        args["ref"] = json!("@e123");
        assert!(command(json!({"command":name,"args":args})).is_ok());
        for invalid in [json!("e123"), json!("@e"), json!("@e-1"), json!(123)] {
            args["ref"] = invalid;
            assert!(command(json!({"command":name,"args":args})).is_err());
        }
    }
}

#[test]
fn browser_ids_reject_wrong_uuid_versions_variants_and_malformed_legacy_ids() {
    for invalid in [
        "11111111-1111-1111-8111-111111111111",
        "11111111-1111-4111-1111-111111111111",
        "170000000000-abc",
        "1700000000000-",
        "1700000000000-xyz",
        "1700000000000-ab-cd",
        "not-a-tab",
    ] {
        assert!(!browser_id(invalid), "{invalid}");
    }
    assert!(browser_id("11111111-1111-4111-A111-111111111111"));
    assert!(browser_id("1700000000000-aBcD"));
}
