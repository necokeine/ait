//! NEC-272: replay permission changes and repository inspection in a real worker.
use super::*;
use ait_domain::{SandboxAccess, ToolExecutionStatus};

#[tokio::test]
async fn changed_permission_reaches_worker_and_repository_inspection() {
    verify_permission_change(env!("CARGO_BIN_EXE_ait-worker").into()).await;
}

async fn verify_permission_change(worker: std::path::PathBuf) {
    for kind in [ProviderKind::DeepSeek, ProviderKind::OpenAI] {
        let shell = json!({
            "command":"pwd && ls && wc -l source.rs",
            "description":"Inspect repository after changing permissions",
            "sandbox_permissions":"workspace-write"
        });
        let f = Fixture::new_with_worker(
            kind,
            vec![
                response(kind, &[("before", "bash", shell.clone())]),
                response(kind, &[]),
                response(kind, &[
                    ("after", "bash", shell),
                    ("count", "bash", json!({"command":"wc -l source.rs","description":"Count source lines"})),
                    ("search", "grep", json!({"pattern":"^pub (struct|enum|trait) [A-Z]"})),
                    ("tests", "grep", json!({"pattern":r"^#\[cfg\(test\)\]"})),
                    ("write", "write", json!({"file_path":"permission.txt","content":"workspace write applied\n","sandbox_permissions":"workspace-write"})),
                ]),
                response(kind, &[]),
            ],
            "read_only",
            worker.clone(),
        ).await;
        // The file fits the legacy read bound, but its matches exceed the output bound.
        std::fs::write(
            f.workdir.join("source.rs"),
            format!("{}#[cfg(test)]\n", "pub struct Example;\n".repeat(1000)),
        )
        .unwrap();
        for args in [
            vec!["add", "source.rs"],
            vec![
                "-c",
                "user.name=Fixture",
                "-c",
                "user.email=fixture@example.invalid",
                "commit",
                "-m",
                "Seed repository inspection",
            ],
        ] {
            let output = std::process::Command::new("git")
                .args(args)
                .current_dir(&f.workdir)
                .output()
                .unwrap();
            assert!(output.status.success(), "{output:?}");
        }

        let before = f.run().await;
        assert_eq!(before.permission_profile.sandbox, SandboxAccess::ReadOnly);
        assert_eq!(
            before.execution.as_ref().unwrap().tools[0].status,
            ToolExecutionStatus::Denied
        );

        let mut settings = default_settings();
        settings
            .0
            .insert("permissions.sandbox".into(), json!("workspace_write"));
        ok(
            &f.service,
            Command::SaveSettings {
                expected_revision: 2,
                values: settings,
            },
        )
        .await;
        let after = f.run().await;
        assert_eq!(after.status, "completed");
        assert_eq!(
            after.permission_profile.sandbox,
            SandboxAccess::WorkspaceWrite
        );
        let tools = &after.execution.as_ref().unwrap().tools;
        assert_eq!(tools.len(), 5);
        let verdicts = tools
            .iter()
            .map(|tool| (&tool.call_id, tool.status, &tool.error))
            .collect::<Vec<_>>();
        assert!(
            tools
                .iter()
                .all(|tool| tool.status == ToolExecutionStatus::Succeeded),
            "{kind:?}: {verdicts:?}"
        );
        for id in ["after", "count"] {
            let output = tools
                .iter()
                .find(|tool| tool.call_id == id)
                .unwrap()
                .result
                .as_ref()
                .unwrap();
            assert_eq!(output["exit_status"], 0, "{output}");
            assert!(
                output["stdout"]
                    .as_str()
                    .unwrap()
                    .contains("1001 source.rs")
            );
        }
        let search = tools
            .iter()
            .find(|tool| tool.call_id == "search")
            .unwrap()
            .result
            .as_ref()
            .unwrap();
        assert_eq!(search["total_count"], 1000);
        assert_eq!(search["count_complete"], true);
        assert_eq!(search["truncated"], true);
        assert_eq!(search["matches"].as_array().unwrap().len(), 200);
        assert_eq!(search["next_offset"], 200);
        let tests = tools
            .iter()
            .find(|tool| tool.call_id == "tests")
            .unwrap()
            .result
            .as_ref()
            .unwrap();
        assert_eq!(tests["matches"][0]["line"], 1001);
        assert_eq!(
            std::fs::read_to_string(f.workdir.join("permission.txt")).unwrap(),
            "workspace write applied\n"
        );
        assert!(!f.project.path().join("permission.txt").exists());

        // Settings changes must leave historical Run and ToolResult records intact.
        let persisted = support::persisted_run(f.store.as_ref(), &before.id).await;
        assert_eq!(persisted, before);
        let requests = f.requests.lock().unwrap().clone();
        assert_eq!(requests.len(), 4);
        // Readonly now advertises write/edit as approvable capabilities. The first
        // invocation above still proves that advertising cannot grant authority.
        for request in [&requests[0], &requests[2]] {
            let has_write = request["tools"].as_array().unwrap().iter().any(|tool| {
                let definition = if kind == ProviderKind::OpenAI {
                    tool
                } else {
                    &tool["function"]
                };
                definition["name"] == "write"
            });
            assert!(has_write);
        }
        let wire = requests[3].to_string();
        assert!(wire.contains("total_count"));
        f.finish().await;
    }
}

#[tokio::test]
#[ignore = "requires AIT_TEST_WORKER_EXECUTABLE pointing to a separately built worker"]
async fn replay_permission_change_with_external_worker() {
    let worker = std::env::var_os("AIT_TEST_WORKER_EXECUTABLE")
        .expect("set AIT_TEST_WORKER_EXECUTABLE to the absolute worker executable path");
    let worker = std::path::PathBuf::from(worker);
    assert!(
        worker.is_absolute(),
        "worker executable path must be absolute"
    );
    verify_permission_change(worker).await;
}
