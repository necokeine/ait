use std::os::unix::fs::PermissionsExt;
use std::path::PathBuf;

use server_domain::agent_runtime::StoredAgentConfig;

use crate::local::codex::CodexClient;
use crate::ports::agent_session::AgentSessionSpec;

pub(crate) struct Fixture {
    pub(crate) root: tempfile::TempDir,
    pub(crate) program: PathBuf,
    pub(crate) cwd: PathBuf,
}

impl Fixture {
    pub(crate) fn new() -> Self {
        let root = tempfile::tempdir().unwrap();
        let program = root.path().join("codex");
        std::fs::write(
            &program,
            include_str!("../tests/fixtures/codex_app_server.py"),
        )
        .unwrap();
        std::fs::set_permissions(&program, std::fs::Permissions::from_mode(0o700)).unwrap();
        let cwd = root.path().join("work");
        std::fs::create_dir(&cwd).unwrap();
        Self { root, program, cwd }
    }

    pub(crate) fn spec(&self) -> AgentSessionSpec {
        AgentSessionSpec {
            provider: "codex".to_owned(),
            cwd: self.cwd.to_str().unwrap().to_owned(),
            config: StoredAgentConfig::default(),
        }
    }

    pub(crate) fn client(&self) -> CodexClient {
        CodexClient::new(self.program.clone())
    }

    pub(crate) fn mode(&self, mode: &str) {
        std::fs::write(self.cwd.join("behavior"), mode).unwrap();
    }

    pub(crate) fn requests(&self) -> Vec<serde_json::Value> {
        std::fs::read_to_string(self.cwd.join("native-requests.jsonl"))
            .unwrap()
            .lines()
            .map(|line| serde_json::from_str(line).unwrap())
            .collect()
    }
}
