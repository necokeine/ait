//! WF-10: opt-in acceptance test using the real daemon, CLI, Codex, Cargo and Git.

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

const PROMPT: &str = "Create a minimal Rust binary package named example-project in this repository root. \
    It must have Cargo.toml, Cargo.lock, src/main.rs, and a .gitignore that ignores /target/. \
    Use no external dependencies. Running cargo run --offline --quiet must print exactly Hello, world! \
    followed by a newline. Verify the program. Do not create a Git commit: AIT will commit your changes.";

struct Daemon {
    child: Child,
}

impl Drop for Daemon {
    fn drop(&mut self) {
        // The daemon owns a fresh process group, including any in-flight Codex process.
        // Kill only that group on success, assertion failure, or a command deadline.
        let _ = ProcessCommand::new("kill")
            .args(["-KILL", "--", &format!("-{}", self.child.id())])
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status();
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

struct Workflow {
    root: PathBuf,
    endpoint: String,
    _daemon: Daemon,
}

impl Workflow {
    async fn start() -> Self {
        let binary = env::var_os("AIT_WORKFLOW_DAEMON_BIN").map_or_else(
            || Path::new(env!("CARGO_BIN_EXE_ait-cli")).with_file_name("ait-daemon"),
            PathBuf::from,
        );
        let binary = binary.canonicalize().expect(
            "build ait-daemon first: cargo build -p ait-daemon; or set AIT_WORKFLOW_DAEMON_BIN",
        );
        let root = tempfile::Builder::new()
            .prefix("ait-wf10-")
            .tempdir()
            .unwrap()
            .keep()
            .canonicalize()
            .unwrap();
        assert_eq!(fs::read_dir(&root).unwrap().count(), 0);
        eprintln!(
            "WF-10 artifacts (retained on success and failure): {}",
            root.display()
        );
        let log_path = root.join("daemon.log");
        let log = File::create(&log_path).unwrap();
        let mut daemon = Daemon {
            child: ProcessCommand::new(binary)
                .args(["--database", "ait.sqlite3", "--listen", "127.0.0.1:0"])
                .current_dir(&root)
                .process_group(0)
                .stdin(Stdio::null())
                .stdout(Stdio::from(log.try_clone().unwrap()))
                .stderr(Stdio::from(log))
                .spawn()
                .expect("start ait-daemon"),
        };
        let endpoint = timeout(Duration::from_secs(10), async {
            loop {
                assert!(
                    daemon.child.try_wait().unwrap().is_none(),
                    "daemon exited; see daemon.log"
                );
                let log = fs::read_to_string(&log_path).unwrap();
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
        .expect("daemon startup exceeded 10 seconds; see daemon.log");
        Self {
            root,
            endpoint,
            _daemon: daemon,
        }
    }

    async fn cli(&self, name: &str, arguments: &[&str], seconds: u64) -> Value {
        let mut command = Command::new(env!("CARGO_BIN_EXE_ait-cli"));
        command
            .args(["--endpoint", &self.endpoint])
            .args(arguments)
            .current_dir(&self.root)
            .env("NO_PROXY", "*")
            .env("no_proxy", "*");
        let output = checked_output(&mut command, seconds).await;
        fs::write(self.root.join(format!("{name}.json")), &output).unwrap();
        let response: Value = serde_json::from_str(&output).expect("CLI JSON envelope");
        assert_eq!(response["api_version"], 1);
        assert_eq!(response["ok"], true, "{response}");
        response["result"]["value"].clone()
    }

    async fn command(&self, name: &str, value: Value, seconds: u64) -> Value {
        self.cli(name, &["command", &value.to_string()], seconds)
            .await
    }
}

async fn checked_output(command: &mut Command, seconds: u64) -> String {
    let description = format!("{command:?}");
    let output = timeout(
        Duration::from_secs(seconds),
        command.kill_on_drop(true).output(),
    )
    .await
    .unwrap_or_else(|_| panic!("command exceeded {seconds}s: {description}"))
    .unwrap_or_else(|error| panic!("cannot start {description}: {error}"));
    assert!(
        output.status.success(),
        "{description}: {}\nstdout: {}\nstderr: {}",
        output.status,
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr),
    );
    String::from_utf8(output.stdout).expect("UTF-8 command output")
}

async fn git(project: &Path, args: &[&str]) -> String {
    checked_output(Command::new("git").arg("-C").arg(project).args(args), 20).await
}

fn entity<'a>(snapshot: &'a Value, collection: &str, id: &Value) -> &'a Value {
    snapshot[collection]
        .as_array()
        .unwrap()
        .iter()
        .find(|value| value["id"] == *id)
        .unwrap_or_else(|| panic!("missing {collection} {id}"))
}

async fn initialize_project(directory: &Path) -> String {
    fs::create_dir(directory).unwrap();
    git(directory, &["init", "--quiet"]).await;
    git(directory, &["config", "user.name", "AIT Workflow"]).await;
    git(directory, &["config", "user.email", "workflow@localhost"]).await;
    git(directory, &["config", "commit.gpgsign", "false"]).await;
    git(
        directory,
        &[
            "commit",
            "--allow-empty",
            "--no-gpg-sign",
            "-m",
            "Initial empty commit",
        ],
    )
    .await;
    let initial_commit = git(directory, &["rev-parse", "HEAD"])
        .await
        .trim()
        .to_owned();
    assert_eq!(
        git(directory, &["rev-list", "--count", "HEAD"])
            .await
            .trim(),
        "1"
    );
    assert!(
        git(directory, &["ls-tree", "--name-only", "HEAD"])
            .await
            .is_empty()
    );

    initial_commit
}

#[tokio::test]
#[ignore = "requires real Codex credentials/model access and a built ait-daemon; see WF-10"]
async fn wf10_create_project_with_real_codex_and_commit() {
    let workflow = Workflow::start().await;
    let empty = workflow.cli("initial-snapshot", &["snapshot"], 20).await;
    for collection in ["projects", "agents", "sessions", "messages", "runs"] {
        assert_eq!(empty[collection], json!([]));
    }

    // The project does not exist until after the isolated daemon has started.
    let directory = workflow.root.join("example-project");
    let initial_commit = initialize_project(&directory).await;
    let project = workflow
        .command(
            "project",
            json!({
                "type": "register_project", "id": "example-project", "name": "example-project",
                "workdir": directory,
            }),
            20,
        )
        .await;
    assert_eq!(project["workdir"], directory.to_str().unwrap());
    assert_eq!(project["base_commit"], initial_commit);
    assert_eq!(
        git(&directory, &["rev-parse", "HEAD"]).await.trim(),
        initial_commit
    );
    let model = env::var("AIT_WORKFLOW_MODEL").unwrap_or_else(|_| "gpt-5.6-sol".into());
    let agent = workflow
        .command(
            "agent",
            json!({
                "type": "register_agent", "id": "codex", "name": "Codex",
                "model": model, "mode": "codex",
            }),
            20,
        )
        .await;
    assert_eq!(agent["mode"], "codex");
    let session = workflow
        .command(
            "session",
            json!({
                "type": "create_session", "id": "hello-world", "project_id": project["id"],
                "agent_id": agent["id"],
            }),
            20,
        )
        .await;
    assert_eq!(session["current_message_id"], project["root_message_id"]);

    eprintln!("WF-10: asking real Codex ({model}) to create Rust Hello World; deadline 600s");
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
    assert_eq!(run["status"], "completed", "{run}");
    assert!(run["error"].is_null(), "{run}");
    let snapshot = workflow.cli("final-snapshot", &["snapshot"], 20).await;
    assert_eq!(entity(&snapshot, "runs", &run["id"]), &run);
    let assistant = entity(&snapshot, "messages", &run["last_message_id"]);
    assert_eq!(assistant["role"], "assistant");
    assert!(!assistant["text"].as_str().unwrap().trim().is_empty());
    let user = entity(&snapshot, "messages", &run["base_message_id"]);
    assert_eq!(user["git_commit"], initial_commit);
    let final_session = entity(&snapshot, "sessions", &session["id"]);
    assert_eq!(final_session["current_message_id"], assistant["id"]);
    assert!(final_session["active_run_id"].is_null());

    let (head, output) = verify_program_and_commit(&directory, &initial_commit).await;
    assert_eq!(assistant["data"]["codex"]["commit_id"], head);
    let report = json!({
        "workflow": "WF-10", "result": "passed", "model": model,
        "project_id": project["id"], "run_id": run["id"], "run_status": run["status"],
        "initial_commit": initial_commit, "commit": head, "commit_count": 2,
        "stdout": output, "worktree_clean": true,
    });
    fs::write(
        workflow.root.join("verification.json"),
        serde_json::to_vec_pretty(&report).unwrap(),
    )
    .unwrap();
    eprintln!("{report}");
}

async fn verify_program_and_commit(directory: &Path, initial_commit: &str) -> (String, String) {
    let head = git(directory, &["rev-parse", "HEAD"])
        .await
        .trim()
        .to_owned();
    assert_ne!(head, initial_commit);
    assert_eq!(
        git(directory, &["rev-list", "--count", "HEAD"])
            .await
            .trim(),
        "2"
    );
    assert_eq!(
        git(directory, &["rev-parse", "HEAD^"]).await.trim(),
        initial_commit
    );
    assert!(
        git(directory, &["log", "-1", "--pretty=%s"])
            .await
            .starts_with("ait: ")
    );
    for path in ["Cargo.toml", "Cargo.lock", "src/main.rs", ".gitignore"] {
        git(directory, &["cat-file", "-e", &format!("HEAD:{path}")]).await;
    }
    assert!(
        git(
            directory,
            &["ls-tree", "-r", "--name-only", "HEAD", "target"]
        )
        .await
        .is_empty()
    );
    assert!(
        git(directory, &["status", "--porcelain=v1"])
            .await
            .is_empty()
    );
    let output = checked_output(
        Command::new("cargo")
            .args(["run", "--offline", "--locked", "--quiet"])
            .current_dir(directory)
            .env(
                "CARGO_TARGET_DIR",
                directory.parent().unwrap().join("cargo-target"),
            ),
        120,
    )
    .await;
    assert_eq!(output, "Hello, world!\n");
    assert!(
        git(directory, &["status", "--porcelain=v1"])
            .await
            .is_empty()
    );
    (head, output)
}
