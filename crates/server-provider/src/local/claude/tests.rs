use super::*;

mod config;
mod history;
mod permissions;
#[cfg(unix)]
mod process;
mod streaming;

fn spec(cwd: &std::path::Path) -> AgentSessionSpec {
    AgentSessionSpec {
        provider: "claude".to_owned(),
        cwd: cwd.to_str().unwrap().to_owned(),
        config: StoredAgentConfig::default(),
    }
}

#[cfg(unix)]
fn fixture() -> (tempfile::TempDir, ClaudeClient, AgentSessionSpec) {
    use std::os::unix::fs::PermissionsExt;
    let root = tempfile::tempdir().unwrap();
    let program = root.path().join("claude");
    std::fs::write(
        &program,
        include_str!("../../../tests/fixtures/claude_code.py"),
    )
    .unwrap();
    std::fs::set_permissions(&program, std::fs::Permissions::from_mode(0o700)).unwrap();
    let mut client = ClaudeClient::new(program);
    client.config_dir = Some(root.path().join("config"));
    client.deadline = Duration::from_secs(2);
    let spec = spec(root.path());
    (root, client, spec)
}
