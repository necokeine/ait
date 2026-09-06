//! WF-11: real `DeepSeek` through the existing Codex harness, with independent Python checks.

#![cfg(unix)]

use std::{
    env,
    fs::{self, File},
    os::unix::process::CommandExt,
    path::{Path, PathBuf},
    process::{Child, Command as ProcessCommand, Stdio},
    time::Duration,
};

use serde_json::{Value, json};
use tokio::{process::Command, time::timeout};

const MODEL: &str = "deepseek-v4-flash";
const VERIFY_LOGIC: &str = include_str!("fixtures/verify_hello.py");
const PROMPT: &str = "Create exactly one file, hello.py, in the repository root. \
    Define a zero-argument main() that only calls print with the literal Hello, world! \
    and implicitly returns None. Call main() only under if __name__ == \"__main__\". \
    No imports, dependencies, other files, or extra behavior. Running python3 -I -B hello.py \
    must print exactly Hello, world! followed by a newline. Verify it. \
    Do not create a Git commit; AIT will commit your changes.";

// Deliberately not Debug: neither command diagnostics nor dotenv parse errors may expose it.
struct Credential(String);

impl Credential {
    fn load(path: &Path) -> Result<Self, &'static str> {
        let metadata =
            fs::metadata(path).map_err(|_| "cannot read .env; set AIT_DEEPSEEK_ENV_FILE")?;
        if metadata.len() > 1_048_576 {
            return Err(".env exceeds the 1 MiB workflow limit");
        }
        let text = fs::read_to_string(path).map_err(|_| ".env must be readable UTF-8")?;
        Self::parse(&text)
    }

    fn parse(text: &str) -> Result<Self, &'static str> {
        let mut key = None;
        for line in text.trim_start_matches('\u{feff}').lines() {
            let line = line.trim().strip_prefix("export ").unwrap_or(line.trim());
            let Some((name, value)) = line.split_once('=') else {
                continue;
            };
            if name.trim() != "DEEPSEEK_API_KEY" {
                continue;
            }
            if key.is_some() {
                return Err("duplicate DEEPSEEK_API_KEY in .env");
            }
            let value = value.trim();
            let value = if value.starts_with(['\'', '"']) {
                let quote = value.chars().next().unwrap();
                value
                    .strip_prefix(quote)
                    .and_then(|value| value.strip_suffix(quote))
                    .ok_or("unbalanced quotes in DEEPSEEK_API_KEY")?
            } else {
                value
            };
            if value.is_empty()
                || !value
                    .bytes()
                    .all(|byte| byte.is_ascii_alphanumeric() || b"-_".contains(&byte))
            {
                return Err(
                    "DEEPSEEK_API_KEY must be a nonempty literal token (letters, digits, - or _)",
                );
            }
            key = Some(Self(value.to_owned()));
        }
        key.ok_or("DEEPSEEK_API_KEY is missing from .env")
    }

    fn assert_absent(&self, bytes: &[u8]) {
        assert!(
            !bytes
                .windows(self.0.len())
                .any(|window| window == self.0.as_bytes()),
            "credential leaked into workflow output; content suppressed"
        );
    }
}

struct Daemon(Child);

impl Drop for Daemon {
    fn drop(&mut self) {
        // Only the new daemon's process group, including any remaining app-server child.
        let _ = ProcessCommand::new("kill")
            .args(["-KILL", "--", &format!("-{}", self.0.id())])
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status();
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

struct Workflow {
    root: PathBuf,
    endpoint: String,
    credential: Credential,
    daemon: Option<Daemon>,
}

fn config(model: &str) -> String {
    format!(
        r#"model = {model:?}
model_provider = "deepseek"
model_reasoning_effort = "low"
model_reasoning_summary = "none"
model_supports_reasoning_summaries = false
web_search = "disabled"

[model_providers.deepseek]
name = "DeepSeek"
base_url = "https://api.deepseek.com"
wire_api = "responses"
env_key = "DEEPSEEK_API_KEY"
requires_openai_auth = false
request_max_retries = 0
stream_max_retries = 0
stream_idle_timeout_ms = 120000

[shell_environment_policy]
include_only = ["PATH", "HOME", "TMPDIR", "USER", "LOGNAME"]

[history]
persistence = "none"
"#
    )
}

impl Workflow {
    async fn start(credential: Credential, model: &str) -> Self {
        let binary = env::var_os("AIT_WORKFLOW_DAEMON_BIN")
            .map_or_else(
                || Path::new(env!("CARGO_BIN_EXE_ait-cli")).with_file_name("ait-daemon"),
                PathBuf::from,
            )
            .canonicalize()
            .expect("build ait-daemon first; see ./test_with_deepseek.sh");
        let root = tempfile::Builder::new()
            .prefix("ait-wf11-")
            .tempdir()
            .unwrap()
            .keep()
            .canonicalize()
            .unwrap();
        assert_eq!(fs::read_dir(&root).unwrap().count(), 0);
        eprintln!(
            "WF-11 artifacts (retained on success/failure): {}",
            root.display()
        );
        let codex_config_dir = root.join("codex");
        fs::create_dir(&codex_config_dir).unwrap();
        fs::write(codex_config_dir.join("config.toml"), config(model)).unwrap();
        let log_path = root.join("daemon.log");
        let log = File::create(&log_path).unwrap();
        let mut command = ProcessCommand::new(binary);
        command.env_clear();
        // Preserve only executable discovery, OS home/temp conventions and network proxy settings.
        for name in [
            "PATH",
            "HOME",
            "TMPDIR",
            "USER",
            "LOGNAME",
            "HTTPS_PROXY",
            "https_proxy",
            "HTTP_PROXY",
            "http_proxy",
            "ALL_PROXY",
            "all_proxy",
            "NO_PROXY",
            "no_proxy",
        ] {
            if let Some(value) = env::var_os(name) {
                command.env(name, value);
            }
        }
        let mut daemon = Daemon(
            command
                .args(["--database", "ait.sqlite3", "--listen", "127.0.0.1:0"])
                .current_dir(&root)
                .env("CODEX_HOME", &codex_config_dir)
                .env("DEEPSEEK_API_KEY", &credential.0)
                .process_group(0)
                .stdin(Stdio::null())
                .stdout(Stdio::from(log.try_clone().unwrap()))
                .stderr(Stdio::from(log))
                .spawn()
                .expect("start isolated daemon"),
        );
        let endpoint = timeout(Duration::from_secs(10), async {
            loop {
                assert!(
                    daemon.0.try_wait().unwrap().is_none(),
                    "daemon exited; see daemon.log"
                );
                let log = fs::read_to_string(&log_path).unwrap();
                credential.assert_absent(log.as_bytes());
                if let Some(endpoint) = log
                    .lines()
                    .find_map(|line| line.strip_prefix("AIT daemon listening on "))
                {
                    break endpoint.to_owned();
                }
                tokio::time::sleep(Duration::from_millis(25)).await;
            }
        })
        .await
        .expect("daemon startup exceeded 10s");
        Self {
            root,
            endpoint,
            credential,
            daemon: Some(daemon),
        }
    }

    async fn output(&self, command: &mut Command, seconds: u64) -> String {
        // Never format Command: it could contain a credential-bearing environment.
        command.env_remove("DEEPSEEK_API_KEY");
        let output = timeout(
            Duration::from_secs(seconds),
            command.kill_on_drop(true).output(),
        )
        .await
        .expect("workflow command timed out")
        .expect("could not start workflow command");
        self.credential.assert_absent(&output.stdout);
        self.credential.assert_absent(&output.stderr);
        assert!(
            output.status.success(),
            "command failed: {}\nstdout: {}\nstderr: {}",
            output.status,
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        assert!(
            output.stderr.is_empty(),
            "unexpected stderr: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        String::from_utf8(output.stdout).unwrap()
    }

    async fn cli(&self, name: &str, args: &[&str], seconds: u64) -> Value {
        let output = self
            .output(
                Command::new(env!("CARGO_BIN_EXE_ait-cli"))
                    .args(["--endpoint", &self.endpoint])
                    .args(args)
                    .current_dir(&self.root)
                    .env("NO_PROXY", "*")
                    .env("no_proxy", "*"),
                seconds,
            )
            .await;
        fs::write(self.root.join(format!("{name}.json")), &output).unwrap();
        let response: Value = serde_json::from_str(&output).expect("CLI JSON envelope");
        assert_eq!(response["api_version"], 1);
        assert_eq!(response["ok"], true, "{response}");
        response["result"]["value"].clone()
    }

    async fn command(&self, name: &str, body: Value, seconds: u64) -> Value {
        self.cli(name, &["command", &body.to_string()], seconds)
            .await
    }

    async fn git(&self, args: &[&str]) -> String {
        self.output(
            Command::new("git")
                .arg("-C")
                .arg(self.root.join("example-project"))
                .args(args),
            20,
        )
        .await
    }
}

fn entity<'a>(snapshot: &'a Value, collection: &str, id: &Value) -> &'a Value {
    snapshot[collection]
        .as_array()
        .unwrap()
        .iter()
        .find(|value| value["id"] == *id)
        .expect("missing workflow entity")
}

#[tokio::test]
#[ignore = "uses real DeepSeek API credits, Codex and Python; run ./test_with_deepseek.sh"]
async fn wf11_real_deepseek_python_hello_world() {
    let env_path = env::var_os("AIT_DEEPSEEK_ENV_FILE").map_or_else(
        || Path::new(env!("CARGO_MANIFEST_DIR")).join("../../.env"),
        PathBuf::from,
    );
    let credential = Credential::load(&env_path).unwrap_or_else(|message| panic!("{message}"));
    let model = env::var("AIT_DEEPSEEK_MODEL").unwrap_or_else(|_| MODEL.into());
    assert!(
        model.starts_with("deepseek-")
            && model
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b == b'-'),
        "AIT_DEEPSEEK_MODEL must name a DeepSeek model"
    );
    let mut workflow = Workflow::start(credential, &model).await;
    let initial = workflow.cli("initial-snapshot", &["snapshot"], 20).await;
    for collection in ["projects", "agents", "sessions", "messages", "runs"] {
        assert_eq!(initial[collection], json!([]));
    }
    let directory = workflow.root.join("example-project");
    fs::create_dir(&directory).unwrap();
    let project = workflow.command("project", json!({
        "type": "register_project", "id": "example-project", "name": "example-project", "workdir": directory,
    }), 20).await;
    workflow.git(&["config", "user.name", "AIT Workflow"]).await;
    workflow
        .git(&["config", "user.email", "workflow@localhost"])
        .await;
    workflow.git(&["config", "commit.gpgsign", "false"]).await;
    assert!(
        workflow
            .git(&["ls-tree", "--name-only", "HEAD"])
            .await
            .is_empty()
    );
    let agent = workflow.command("agent", json!({
        "type": "register_agent", "id": "deepseek", "name": "DeepSeek", "model": model, "mode": "codex",
    }), 20).await;
    assert_eq!(agent["model"], model);
    assert_eq!(agent["mode"], "codex");
    let default = workflow.command("default-agent", json!({
        "type": "set_project_default_agent", "project_id": project["id"], "agent_id": agent["id"],
    }), 20).await;
    assert_eq!(default["default_agent_id"], agent["id"]);
    // The current contract requires an explicit Session Agent; select the saved Project default.
    let session = workflow
        .command(
            "session",
            json!({
                "type": "create_session", "id": "hello-world", "project_id": project["id"],
                "agent_id": default["default_agent_id"],
            }),
            20,
        )
        .await;
    // Naming the Session suppresses the unrelated, hardcoded OpenAI title-model request.
    let session = workflow.command("named-session", json!({
        "type": "rename_session", "session_id": session["id"], "name": "DeepSeek Hello World",
    }), 20).await;
    assert_eq!(session["agent_id"], agent["id"]);
    assert_eq!(session["current_message_id"], project["root_message_id"]);
    eprintln!("WF-11: asking real DeepSeek ({model}) via Codex; deadline 600s");
    let run = workflow
        .command(
            "run",
            json!({
                "type": "send_message", "session_id": session["id"],
                "expected_version": session["version"], "text": PROMPT,
            }),
            600,
        )
        .await;
    verify_run(&mut workflow, &project, &agent, &session, &run, &model).await;
}

async fn verify_run(
    workflow: &mut Workflow,
    project: &Value,
    agent: &Value,
    session: &Value,
    run: &Value,
    model: &str,
) {
    let directory = workflow.root.join("example-project");
    assert_eq!(run["status"], "completed", "{run}");
    assert!(run["error"].is_null(), "{run}");
    assert_eq!(run["agent_id"], agent["id"]);
    assert_eq!(run["agent_revision"], agent["revision"]);
    let snapshot = workflow.cli("final-snapshot", &["snapshot"], 20).await;
    assert_eq!(
        entity(&snapshot, "projects", &project["id"])["default_agent_id"],
        agent["id"]
    );
    assert_eq!(entity(&snapshot, "runs", &run["id"]), run);
    let assistant = entity(&snapshot, "messages", &run["last_message_id"]);
    assert_eq!(assistant["role"], "assistant");
    assert!(!assistant["text"].as_str().unwrap().trim().is_empty());
    assert_eq!(
        entity(&snapshot, "messages", &run["base_message_id"])["git_commit"],
        project["base_commit"]
    );
    let final_session = entity(&snapshot, "sessions", &session["id"]);
    assert!(final_session["active_run_id"].is_null());
    assert_eq!(final_session["current_message_id"], assistant["id"]);
    assert_eq!(
        workflow
            .git(&["ls-tree", "-r", "--name-only", "HEAD"])
            .await,
        "hello.py\n"
    );
    assert_eq!(
        workflow.git(&["rev-list", "--count", "HEAD"]).await.trim(),
        "2"
    );
    assert_eq!(
        workflow.git(&["rev-parse", "HEAD^"]).await.trim(),
        project["base_commit"].as_str().unwrap()
    );
    let head = workflow.git(&["rev-parse", "HEAD"]).await.trim().to_owned();
    assert_eq!(assistant["data"]["codex"]["commit_id"], head);
    assert!(workflow.git(&["status", "--porcelain=v1"]).await.is_empty());
    let files = fs::read_dir(&directory)
        .unwrap()
        .map(|entry| entry.unwrap().file_name())
        .filter(|name| name != ".git")
        .collect::<Vec<_>>();
    assert_eq!(files, vec!["hello.py"]);
    assert!(
        fs::symlink_metadata(directory.join("hello.py"))
            .unwrap()
            .file_type()
            .is_file()
    );
    let logic = workflow
        .output(
            Command::new("python3")
                .args(["-I", "-B", "-c", VERIFY_LOGIC])
                .arg(directory.join("hello.py"))
                .current_dir(&workflow.root),
            20,
        )
        .await;
    assert_eq!(logic, "logic verified\n");
    let stdout = workflow
        .output(
            Command::new("python3")
                .args(["-I", "-B", "hello.py"])
                .current_dir(&directory),
            20,
        )
        .await;
    assert_eq!(stdout, "Hello, world!\n");
    assert!(workflow.git(&["status", "--porcelain=v1"]).await.is_empty());
    drop(workflow.daemon.take());
    // Scan durable AIT state and output only after stopping the writer.
    for entry in fs::read_dir(&workflow.root).unwrap() {
        let path = entry.unwrap().path();
        if path.is_file() {
            workflow.credential.assert_absent(&fs::read(path).unwrap());
        }
    }
    let report = json!({
        "workflow": "WF-11", "result": "passed", "provider": "deepseek", "harness": "codex",
        "model": model, "project_id": project["id"], "default_agent_id": agent["id"],
        "run_id": run["id"], "run_status": run["status"], "commit": head,
        "files": ["hello.py"], "stdout": stdout, "logic_verified": true, "worktree_clean": true,
    });
    fs::write(
        workflow.root.join("verification.json"),
        serde_json::to_vec_pretty(&report).unwrap(),
    )
    .unwrap();
    eprintln!("{report}");
}

#[test]
fn dotenv_reads_only_a_literal_key_and_reports_no_secret() {
    for text in [
        "DEEPSEEK_API_KEY=sk-test_123",
        "export DEEPSEEK_API_KEY = 'sk-test_123'\r\n",
        "\u{feff}# comment\nIGNORED=$(false)\nDEEPSEEK_API_KEY=\"sk-test_123\"\n",
    ] {
        assert_eq!(Credential::parse(text).unwrap().0, "sk-test_123");
    }
    for text in [
        "",
        "API_KEY=sk-private",
        "DEEPSEEK_API_KEY=",
        "DEEPSEEK_API_KEY='sk-private",
        "DEEPSEEK_API_KEY=$(sk-private)",
        "DEEPSEEK_API_KEY=sk-private # comment",
        "DEEPSEEK_API_KEY=sk-private\nDEEPSEEK_API_KEY=sk-other",
    ] {
        let error = Credential::parse(text)
            .err()
            .expect("invalid dotenv must fail");
        assert!(!error.contains("sk-private"));
    }
    assert!(Credential::load(Path::new("/nonexistent/ait-wf11.env")).is_err());
}

#[test]
fn python_verifier_accepts_the_contract_and_rejects_wrong_or_extra_logic() {
    let root = tempfile::tempdir().unwrap();
    let program = root.path().join("hello.py");
    let good =
        "def main():\n    print('Hello, world!')\n\nif __name__ == '__main__':\n    main()\n";
    for (source, expected) in [
        (good.to_owned(), true),
        (
            format!(
                "\"\"\"Module docstring.\"\"\"\n{}",
                good.replace("main():", "main() -> None:")
            ),
            true,
        ),
        (good.replace("world!", "WORLD!"), false),
        (
            good.replace("if __name__ == '__main__':", "if True:"),
            false,
        ),
        (format!("import os\n{good}"), false),
        (good.replace("print('Hello, world!')", "pass"), false),
        (format!("{good}\nmain()\n"), false),
    ] {
        fs::write(&program, source).unwrap();
        let output = ProcessCommand::new("python3")
            .args(["-I", "-B", "-c", VERIFY_LOGIC])
            .arg(&program)
            .output()
            .expect("Python 3 is required for WF-11 logic checks");
        assert_eq!(
            output.status.success(),
            expected,
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
    }
}
