//! Opt-in real Codex tool round trip, with assertions between two turns.

use std::{collections::HashSet, fs, path::Path, process::Command, time::Duration};

use ait_agent_adapters::{
    AgentAdapter, AgentEvent, AgentRunRequest, AgentRunStatus, ApprovalPolicy, SandboxMode,
    codex::{CodexAppServerAdapter, CodexAppServerConfig},
};
use serde_json::{Value, json};
use tokio_stream::StreamExt;
use tokio_util::sync::CancellationToken;

async fn turn(
    adapter: &CodexAppServerAdapter,
    cwd: &Path,
    thread: Option<String>,
    prompt: &str,
    sequence: u8,
) -> Vec<AgentEvent> {
    let cancellation = CancellationToken::new();
    let mut stream = adapter.run(AgentRunRequest {
        request_id: format!("python-smoke-{sequence}"),
        model: Some(std::env::var("AIT_CODEX_SMOKE_MODEL").unwrap_or_else(|_| "gpt-5.6-sol".into())),
        reasoning_effort: Some("low".into()),
        project_instructions: Some("Use Python 3, no third-party dependencies. Do not access the network or delegate. Only work on hello.py in the project directory.".into()),
        prompt: prompt.into(),
        cwd: cwd.to_path_buf(),
        resume_thread_id: thread,
        sandbox: SandboxMode::WorkspaceWrite,
        approval_policy: ApprovalPolicy::Never,
        output_schema: None,
        cancellation: cancellation.clone(),
    }).await.unwrap();
    let result = tokio::time::timeout(Duration::from_mins(4), async {
        let mut events = Vec::new();
        while let Some(event) = stream.next().await {
            events.push(event.expect("Codex protocol event"));
        }
        events
    })
    .await;
    if result.is_err() {
        cancellation.cancel();
        // Collect child cleanup before the test returns, including startup failures.
        let _ = tokio::time::timeout(Duration::from_secs(10), async {
            while stream.next().await.is_some() {}
        })
        .await;
    }
    let events = result.expect("Codex turn exceeded 240 seconds");
    assert!(
        matches!(
            events.last(),
            Some(AgentEvent::Completed {
                status: AgentRunStatus::Completed,
                error: None,
                ..
            })
        ),
        "turn must finish successfully: {:?}",
        events.last()
    );
    events
}

fn thread_id(events: &[AgentEvent]) -> String {
    events
        .iter()
        .find_map(|event| match event {
            AgentEvent::ThreadStarted { thread_id } => Some(thread_id.clone()),
            _ => None,
        })
        .expect("thread id")
}

async fn verify_python(cwd: &Path) {
    assert!(
        cwd.join("hello.py").is_file(),
        "native edit must create hello.py"
    );
    let output = tokio::time::timeout(
        Duration::from_secs(10),
        tokio::process::Command::new("python3")
            .arg("hello.py")
            .current_dir(cwd)
            .kill_on_drop(true)
            .output(),
    )
    .await
    .expect("Python execution exceeded 10 seconds")
    .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(output.stdout, b"Hello, world!\n");
    assert!(output.stderr.is_empty());
}

fn tool_evidence(events: &[AgentEvent]) -> Vec<Value> {
    let mut started = HashSet::new();
    let mut evidence = Vec::new();
    for event in events {
        match event {
            AgentEvent::ItemStarted { item } => {
                started.insert(item["id"].as_str().unwrap().to_owned());
            }
            AgentEvent::ItemCompleted { item }
                if matches!(
                    item["type"].as_str(),
                    Some("fileChange" | "commandExecution")
                ) =>
            {
                assert!(
                    started.contains(item["id"].as_str().unwrap()),
                    "tool completion must follow its start"
                );
                evidence.push(json!({
                    "id": item["id"], "type": item["type"], "status": item["status"],
                    "exit_code": item["exitCode"],
                    "hello_output": item["aggregatedOutput"].as_str().is_some_and(|text| text.lines().any(|line| line == "Hello, world!")),
                    "runs_python": item["command"].as_str().is_some_and(|command| command.contains("python3") && command.contains("hello.py")),
                    "edits_hello_py": item["changes"].as_array().is_some_and(|changes| changes.iter().any(|change| {
                        change["path"].as_str().is_some_and(|path| Path::new(path).file_name().is_some_and(|name| name == "hello.py"))
                    })),
                }));
            }
            _ => {}
        }
    }
    evidence
}

#[tokio::test]
#[ignore = "requires logged-in Codex and Python 3; performs two real model turns"]
async fn codex_native_tools_create_and_verify_python_hello_world() {
    let cwd = tempfile::Builder::new()
        .prefix("ait-codex-python-")
        .tempdir()
        .unwrap()
        .keep()
        .canonicalize()
        .unwrap();
    eprintln!("Codex Python smoke artifacts: {}", cwd.display());
    let version = Command::new("codex")
        .arg("--version")
        .output()
        .expect("install Codex first");
    assert!(version.status.success());
    assert!(
        Command::new("git")
            .args(["init", "--quiet"])
            .current_dir(&cwd)
            .status()
            .unwrap()
            .success()
    );
    let adapter = CodexAppServerAdapter::new(CodexAppServerConfig::default()).unwrap();

    let create = turn(&adapter, &cwd, None,
        "Use the native apply_patch tool to create hello.py containing print(\"Hello, world!\"). Do not write files through shell redirection. Then run python3 hello.py with the native command tool and check its output. Report the result.", 1).await;
    verify_python(&cwd).await;
    let first = tool_evidence(&create);
    fs::write(
        cwd.join("stage-1.json"),
        serde_json::to_vec_pretty(&first).unwrap(),
    )
    .unwrap();
    assert!(
        first.iter().any(|event| event["type"] == "fileChange"
            && event["status"] == "completed"
            && event["edits_hello_py"] == true),
        "must observe a successful native patch: {first:?}"
    );
    assert!(
        first.iter().any(|event| event["type"] == "commandExecution"
            && event["status"] == "completed"
            && event["exit_code"] == 0
            && event["runs_python"] == true
            && event["hello_output"] == true),
        "Codex must execute and inspect Python: {first:?}"
    );

    // Resume the actual core thread, with fresh instructions and permissions.
    let thread = thread_id(&create);
    let verify = turn(&adapter, &cwd, Some(thread.clone()),
        "Inspect hello.py with the native command tool. Use apply_patch to refactor it to a main() function plus an if __name__ == \"__main__\" guard. Preserve the exact Hello, world! output. Run python3 hello.py and check success. Report the result.", 2).await;
    assert_eq!(thread_id(&verify), thread);
    verify_python(&cwd).await;
    let source = fs::read_to_string(cwd.join("hello.py")).unwrap();
    assert!(source.contains("def main("));
    assert!(source.contains("__name__"));
    let second = tool_evidence(&verify);
    assert!(
        second.iter().any(|event| event["type"] == "fileChange"
            && event["status"] == "completed"
            && event["edits_hello_py"] == true),
        "resume must use a native patch: {second:?}"
    );
    assert!(
        second
            .iter()
            .any(|event| event["type"] == "commandExecution"
                && event["status"] == "completed"
                && event["exit_code"] == 0
                && event["runs_python"] == true
                && event["hello_output"] == true),
        "resume must execute Python: {second:?}"
    );
    let report = json!({
        "result": "passed", "codex_version": String::from_utf8_lossy(&version.stdout).trim(),
        "tool_set": ait_tools::codex::CODEX_TOOL_SET_REVISION,
        "provider": "codex", "thread_resumed": true,
        "stdout": "Hello, world!\n", "source": source,
        "create": first, "refactor_and_verify": second,
    });
    fs::write(
        cwd.join("verification.json"),
        serde_json::to_vec_pretty(&report).unwrap(),
    )
    .unwrap();
    eprintln!("{report}");
}
