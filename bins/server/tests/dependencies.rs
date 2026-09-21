//! Architectural regression guard, including optional, target-specific, dev and build edges.

use std::collections::BTreeSet;
use std::process::Command;

use serde_json::{Value, json};

fn violations(packages: &[Value], members: &BTreeSet<String>) -> Vec<String> {
    let workspace_names: BTreeSet<_> = packages
        .iter()
        .filter(|p| members.contains(p["id"].as_str().unwrap()))
        .map(|p| p["name"].as_str().unwrap())
        .collect();
    let mut violations = Vec::new();
    for package in packages
        .iter()
        .filter(|p| p["name"].as_str().unwrap().starts_with("server-"))
    {
        let name = package["name"].as_str().unwrap();
        let allowed: &[&str] = match name {
            "server-bin" => &[
                "server-api",
                "server-protocol",
                "server-application",
                "server-domain",
                "server-ports",
                "server-storage",
                "server-workspace",
                "server-execution",
                "server-providers",
            ],
            "server-api" => &["server-application", "server-domain", "server-protocol"],
            "server-application" => &["server-domain", "server-ports"],
            "server-ports" => &["server-domain"],
            "server-storage" | "server-workspace" | "server-providers" => {
                &["server-domain", "server-ports"]
            }
            "server-execution" => &["server-domain", "server-ports", "server-protocol"],
            "server-domain" | "server-protocol" => &[],
            _ => {
                violations.push(format!("unregistered server package: {name}"));
                continue;
            }
        };
        for dependency in package["dependencies"].as_array().unwrap() {
            let target = dependency["name"].as_str().unwrap();
            if (workspace_names.contains(target) || dependency["path"].is_string())
                && !allowed.contains(&target)
            {
                violations.push(format!("{name} -> {target}"));
            }
            if matches!(name, "server-domain" | "server-protocol")
                && [
                    "tokio", "sqlx", "axum", "hyper", "reqwest", "tonic", "tauri", "rig", "codex",
                ]
                .iter()
                .any(|prefix| target == *prefix || target.starts_with(&format!("{prefix}-")))
            {
                violations.push(format!("impure {name} -> {target}"));
            }
        }
    }
    violations
}

#[test]
fn all_server_dependency_edges_follow_adr_022() {
    let output = Command::new(env!("CARGO"))
        .args(["metadata", "--format-version", "1", "--no-deps", "--locked"])
        .current_dir(env!("CARGO_MANIFEST_DIR"))
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let metadata: Value = serde_json::from_slice(&output.stdout).unwrap();
    let members = metadata["workspace_members"]
        .as_array()
        .unwrap()
        .iter()
        .map(|id| id.as_str().unwrap().to_owned())
        .collect();
    let violations = violations(metadata["packages"].as_array().unwrap(), &members);
    assert!(
        violations.is_empty(),
        "server boundary violations: {violations:?}"
    );
}

#[test]
fn guard_catches_indirect_renamed_optional_target_and_development_edges() {
    let members = ["server-bin", "server-api", "ait-domain"]
        .map(str::to_owned)
        .into_iter()
        .collect();
    for kind in [Value::Null, json!("dev"), json!("build")] {
        let packages = vec![
            json!({"id":"server-bin", "name":"server-bin", "dependencies":[{"name":"server-api", "path":"../server-api"}]}),
            json!({"id":"server-api", "name":"server-api", "dependencies":[{"name":"ait-domain", "rename":"innocent", "kind":kind, "optional":true, "target":"cfg(windows)"}]}),
            json!({"id":"ait-domain", "name":"ait-domain", "dependencies":[]}),
        ];
        assert_eq!(
            violations(&packages, &members),
            ["server-api -> ait-domain"]
        );
    }
    assert_eq!(
        violations(
            &[
                json!({"id":"server-domain", "name":"server-domain", "dependencies":[{"name":"tokio"}]})
            ],
            &BTreeSet::new()
        ),
        ["impure server-domain -> tokio"]
    );
}
