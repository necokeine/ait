//! Default catalog coverage, schema validity, and profile selection contracts.

use ait_tools::{DEFAULT_SYSTEM_PROMPT, ToolDefinition, ToolSet, ToolSetError, ToolSetRegistry};
use serde_json::{Value, json};

fn definition(name: &str) -> ToolDefinition {
    ToolDefinition {
        name: name.to_owned(),
        description: "A fixture tool".to_owned(),
        parameters: json!({"type": "object", "properties": {}}),
    }
}

#[test]
fn catalog_covers_standard_plus_minimal_editor_with_valid_schemas() {
    let catalog = ToolSet::default();
    // Audited against the pinned Standard composition, including dynamically
    // registered list_subagent_models, plus Minimal's str_replace_editor.
    let mut expected = vec![
        "ask_user_question",
        "create_goal",
        "edit",
        "exit_plan_mode",
        "get_goal",
        "glob",
        "grep",
        "interrupt_agent",
        "job_kill",
        "job_list",
        "job_output",
        "list_agents",
        "list_subagent_models",
        "ralph",
        "read",
        "read_image",
        "send_message",
        "skill",
        "str_replace_editor",
        "subagent",
        "subagent_fork",
        "todo_write",
        "update_goal",
        "web_fetch",
        "web_search",
        "workflow",
        "write",
    ];
    expected.push(if cfg!(windows) { "pwsh" } else { "bash" });
    expected.sort_unstable();
    assert_eq!(
        catalog
            .tools()
            .iter()
            .map(|tool| tool.name.as_str())
            .collect::<Vec<_>>(),
        expected
    );
    assert_eq!(catalog.system_prompt(), DEFAULT_SYSTEM_PROMPT);

    // Also validate the shell variant not selected on this test host.
    let all: Vec<ToolDefinition> =
        serde_json::from_str(include_str!("../catalog/default.json")).unwrap();
    for tool in all {
        jsonschema::validator_for(&tool.parameters).unwrap_or_else(|error| {
            panic!("invalid schema for {}: {error}", tool.name);
        });
        assert_eq!(
            serde_json::from_value::<ToolDefinition>(serde_json::to_value(&tool).unwrap()).unwrap(),
            tool
        );
        assert!(!tool.description.contains("DSH"));
    }
}

#[test]
fn schemas_accept_real_arguments_and_reject_bad_calls() {
    let catalog = ToolSet::default();
    for (name, valid, invalid) in [
        (
            "read",
            json!({"file_path":"src/main.rs","offset":1,"limit":20}),
            json!({"file_path":"src/main.rs","offset":0}),
        ),
        (
            "edit",
            json!({"file_path":"a.rs","old_string":"before","new_string":"after"}),
            json!({"file_path":"a.rs","new_string":"after"}),
        ),
        (
            "str_replace_editor",
            json!({"command":"view","path":"/repo/a.rs","view_range":[1,10]}),
            json!({"command":"delete","path":"/repo/a.rs"}),
        ),
        (
            "web_search",
            json!({"queries":["Rust async"]}),
            json!({"queries":[]}),
        ),
        (
            "todo_write",
            json!({"todos":[{"content":"Inspect","status":"in_progress"}]}),
            json!({"todos":[{"content":"Inspect","status":"done"}]}),
        ),
        (
            "update_goal",
            json!({"goal_id":"goal-1","revision":1,"action":"complete"}),
            json!({"goal_id":"goal-1","revision":1.5,"action":"complete"}),
        ),
        (
            "subagent",
            json!({"description":"Inspect parser errors","prompt":"Review the parser","provider":"deepseek","model":"fixture-model"}),
            json!({"prompt":"Review the parser"}),
        ),
    ] {
        let validator = jsonschema::validator_for(&catalog.get(name).unwrap().parameters).unwrap();
        assert!(validator.is_valid(&valid), "{name} rejected a valid call");
        assert!(
            !validator.is_valid(&invalid),
            "{name} accepted invalid arguments"
        );
    }
    let search = jsonschema::validator_for(&catalog.get("web_search").unwrap().parameters).unwrap();
    assert!(!search.is_valid(&json!({"queries":["a","b","c","d","e"]})));
}

#[test]
fn overrides_are_exact_and_do_not_change_the_shared_default() {
    let mut registry = ToolSetRegistry::default();
    let custom = ToolSet::new("Custom system instructions", vec![definition("lookup")]).unwrap();
    registry.insert("deepseek", "custom-model", custom.clone());
    assert_eq!(registry.resolve("deepseek", "custom-model"), &custom);
    for (provider, model) in [
        ("deepseek", "unknown"),
        ("openai", "custom-model"),
        ("claude", "custom-model"),
    ] {
        assert_eq!(registry.resolve(provider, model), &ToolSet::default());
    }
    assert_eq!(
        ToolSetRegistry::new(custom.clone()).resolve("openai", "any-model"),
        &custom
    );
}

#[test]
fn catalogs_reject_ambiguous_configuration_and_have_stable_order() {
    assert_eq!(ToolSet::new(" ", vec![]), Err(ToolSetError::EmptyPrompt));
    assert_eq!(
        ToolSet::new("system", vec![definition("same"), definition("same")]),
        Err(ToolSetError::InvalidName)
    );
    for name in ["", "bad name", "非ASCII", &"a".repeat(65)] {
        assert_eq!(
            ToolSet::new("system", vec![definition(name)]),
            Err(ToolSetError::InvalidName)
        );
    }
    let mut bad = definition("tool");
    bad.parameters = Value::Null;
    assert_eq!(
        ToolSet::new("system", vec![bad]),
        Err(ToolSetError::InvalidDefinition)
    );
    let a = ToolSet::new("system", vec![definition("z"), definition("a")]).unwrap();
    let b = ToolSet::new("system", vec![definition("a"), definition("z")]).unwrap();
    assert_eq!(a, b);
    assert!(a.get("missing").is_none());
    assert!(
        ToolSet::new("Text only", vec![])
            .unwrap()
            .tools()
            .is_empty()
    );
}
