//! Codex skill-catalog cases from Paseo's app-server provider suite.

use super::*;

fn skill(name: &str, enabled: bool, path: &str) -> Value {
    json!({"name":name,"enabled":enabled,"path":path,"description":"Offline skill"})
}

#[test]
fn disabled_skills_do_not_reappear_in_slash_command_catalog() {
    let root = tempfile::tempdir().unwrap();
    let cwd = root.path().to_str().unwrap();
    let response = json!({"data":[{"cwd":cwd,"errors":[],"skills":[skill("disabled", false, "/disabled/SKILL.md"),skill("enabled", true, "/enabled/SKILL.md")]}]});
    let commands = skills(&response, cwd).unwrap();
    assert_eq!(commands.len(), 1);
    assert_eq!(commands[0].name, "enabled");
}

#[test]
fn duplicate_skill_names_from_multiple_roots_keep_first_native_precedence() {
    let root = tempfile::tempdir().unwrap();
    let cwd = root.path().to_str().unwrap();
    let response = json!({"data":[
        {"cwd":cwd,"errors":[],"skills":[skill("review", true, "/project/review/SKILL.md")]},
        {"cwd":cwd,"errors":[],"skills":[skill("review", true, "/user/review/SKILL.md")]}
    ]});
    let commands = skills(&response, cwd).unwrap();
    assert_eq!(commands.len(), 1);
    assert_eq!(commands[0].path, "/project/review/SKILL.md");
}

#[test]
fn disabled_duplicate_does_not_hide_an_enabled_skill_from_another_root() {
    let root = tempfile::tempdir().unwrap();
    let cwd = root.path().to_str().unwrap();
    let response = json!({"data":[{"cwd":cwd,"errors":[],"skills":[skill("review", false, "/disabled/SKILL.md"),skill("review", true, "/enabled/SKILL.md")]}]});
    let commands = skills(&response, cwd).unwrap();
    assert_eq!(commands.len(), 1);
    assert_eq!(commands[0].path, "/enabled/SKILL.md");
}

#[test]
fn foreign_workspace_skill_catalog_and_errors_do_not_leak_into_current_workspace() {
    let root = tempfile::tempdir().unwrap();
    let foreign = tempfile::tempdir().unwrap();
    let cwd = root.path().to_str().unwrap();
    let response = json!({"data":[
        {"cwd":foreign.path(),"errors":["foreign error"],"skills":[skill("foreign", true, "/foreign/SKILL.md")]},
        {"cwd":cwd,"errors":[],"skills":[skill("local", true, "/local/SKILL.md")]}
    ]});
    let commands = skills(&response, cwd).unwrap();
    assert_eq!(commands.len(), 1);
    assert_eq!(commands[0].name, "local");
}

#[test]
fn unresolved_workspace_paths_cannot_match_each_other_and_expose_skills() {
    let root = tempfile::tempdir().unwrap();
    let missing = root.path().join("missing");
    let response = json!({"data":[{"cwd":root.path().join("different-missing"),"errors":[],"skills":[skill("foreign", true, "/foreign/SKILL.md")]}]});
    assert!(skills(&response, missing.to_str().unwrap()).is_err());
}

#[cfg(unix)]
#[test]
fn canonical_workspace_aliases_share_the_same_native_skill_catalog() {
    let root = tempfile::tempdir().unwrap();
    let actual = root.path().join("actual");
    let alias = root.path().join("alias");
    std::fs::create_dir(&actual).unwrap();
    std::os::unix::fs::symlink(&actual, &alias).unwrap();
    let response = json!({"data":[{"cwd":alias,"errors":[],"skills":[skill("review", true, "/review/SKILL.md")]}]});
    let commands = skills(&response, actual.to_str().unwrap()).unwrap();
    assert_eq!(commands.len(), 1);
    assert_eq!(commands[0].name, "review");
}

#[test]
fn malformed_enabled_skill_cannot_create_a_partial_command_catalog() {
    let root = tempfile::tempdir().unwrap();
    let cwd = root.path().to_str().unwrap();
    let response = json!({"data":[{"cwd":cwd,"errors":[],"skills":[skill("valid", true, "/valid/SKILL.md"),{"name":"missing-description","path":"/bad/SKILL.md","enabled":true}]}]});
    assert!(skills(&response, cwd).is_err());
}
