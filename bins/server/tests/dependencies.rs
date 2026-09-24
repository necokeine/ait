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
                "server-metadata",
                "server-filesystem",
                "server-provider",
                "server-api",
                "server-terminal",
                "server-protocol",
                "server-domain",
            ],
            "server-api" => &[
                "server-terminal",
                "server-provider",
                "server-protocol",
                "server-metadata",
                "server-filesystem",
            ],
            "server-provider" => &["server-domain", "server-metadata"],
            "server-protocol" => &[
                "server-metadata",
                "server-filesystem",
                "server-provider",
                "server-terminal",
            ],
            "server-filesystem" | "server-terminal" => &["server-metadata"],
            "server-domain" | "server-metadata" => &[],
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
            if matches!(
                name,
                "server-domain"
                    | "server-protocol"
                    | "server-metadata"
                    | "server-filesystem"
                    | "server-terminal"
            ) && [
                "tokio", "sqlx", "rusqlite", "axum", "hyper", "reqwest", "tonic", "tauri", "rig",
                "codex",
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
fn all_server_dependency_edges_follow_adr_031() {
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

#[test]
fn metadata_cannot_depend_on_host_crates_or_runtime_adapters() {
    for dependency in [
        "server-protocol",
        "server-provider",
        "server-domain",
        "server-ports",
        "server-application",
        "ait-domain",
    ] {
        let packages = [
            json!({"id":"server-metadata", "name":"server-metadata", "dependencies":[{"name":dependency, "path":"../dependency"}]}),
        ];
        assert_eq!(
            violations(&packages, &BTreeSet::new()),
            [format!("server-metadata -> {dependency}")]
        );
    }
    for dependency in ["tokio", "rusqlite", "axum", "reqwest"] {
        let packages = [
            json!({"id":"server-metadata", "name":"server-metadata", "dependencies":[{"name":dependency}]}),
        ];
        assert_eq!(
            violations(&packages, &BTreeSet::new()),
            [format!("impure server-metadata -> {dependency}")]
        );
    }
}

#[test]
fn filesystem_cannot_depend_on_host_agent_or_transport_crates() {
    for dependency in [
        "server-api",
        "server-protocol",
        "server-application",
        "server-ports",
        "server-domain",
        "server-workspace",
        "server-provider",
        "ait-domain",
    ] {
        let packages = [
            json!({"id":"server-filesystem", "name":"server-filesystem", "dependencies":[{"name":dependency, "path":"../dependency"}]}),
        ];
        assert_eq!(
            violations(&packages, &BTreeSet::new()),
            [format!("server-filesystem -> {dependency}")]
        );
    }
    for dependency in ["tokio", "axum", "reqwest", "rusqlite"] {
        let packages = [
            json!({"id":"server-filesystem", "name":"server-filesystem", "dependencies":[{"name":dependency}]}),
        ];
        assert_eq!(
            violations(&packages, &BTreeSet::new()),
            [format!("impure server-filesystem -> {dependency}")]
        );
    }
}

#[test]
fn provider_depends_inward_and_retired_packages_cannot_return() {
    for dependency in [
        "server-api",
        "server-protocol",
        "server-filesystem",
        "ait-domain",
    ] {
        let packages = [
            json!({"id":"server-provider", "name":"server-provider", "dependencies":[{"name":dependency, "path":"../dependency"}]}),
        ];
        assert_eq!(
            violations(&packages, &BTreeSet::new()),
            [format!("server-provider -> {dependency}")]
        );
    }
    for retired in [
        "server-application",
        "server-ports",
        "server-storage",
        "server-workspace",
        "server-providers",
    ] {
        let packages = [json!({"id":retired, "name":retired, "dependencies":[]})];
        assert_eq!(
            violations(&packages, &BTreeSet::new()),
            [format!("unregistered server package: {retired}")]
        );
    }
}

#[test]
fn terminal_cannot_depend_on_provider_transport_or_old_workspace_packages() {
    for dependency in [
        "server-api",
        "server-protocol",
        "server-provider",
        "server-domain",
        "server-filesystem",
        "ait-domain",
    ] {
        let packages = [
            json!({"id":"server-terminal", "name":"server-terminal", "dependencies":[{"name":dependency,"path":"../dependency"}]}),
        ];
        assert_eq!(
            violations(&packages, &BTreeSet::new()),
            [format!("server-terminal -> {dependency}")]
        );
    }
    for dependency in ["tokio", "axum", "rusqlite", "reqwest"] {
        let packages = [
            json!({"id":"server-terminal", "name":"server-terminal", "dependencies":[{"name":dependency}]}),
        ];
        assert_eq!(
            violations(&packages, &BTreeSet::new()),
            [format!("impure server-terminal -> {dependency}")]
        );
    }
}
