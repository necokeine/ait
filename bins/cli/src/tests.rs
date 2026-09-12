use std::{collections::BTreeSet, io::Cursor};

use ait_contracts::{
    AgentConfiguration, AgentMode, AgentProvider, Command, NativeApprovalAction, ProjectExport,
    ProjectView, ProviderModel, ProviderSecret, default_settings,
};
use ait_domain::ApprovalGrantScope;
use clap::{CommandFactory, Parser};

use crate::{
    args::{Action, Arguments},
    input::StdinSource,
};

fn action(args: &[&str], stdin: &str) -> Action {
    Arguments::try_parse_from(std::iter::once("ait").chain(args.iter().copied()))
        .unwrap_or_else(|error| panic!("{error}"))
        .command
        .into_action(&mut Cursor::new(stdin), StdinSource::Redirected)
        .unwrap()
}

// Keep the complete coverage table together for review.
#[allow(clippy::too_many_lines)]
#[test]
fn every_contract_variant_has_an_explicit_cli_mapping() {
    let directory = tempfile::tempdir().unwrap();
    let archive = ProjectExport {
        format_version: 3,
        source_revision: 1,
        project: ProjectView {
            id: "p".into(),
            name: "Project".into(),
            workdir: "/old".into(),
            root_message_id: "root".into(),
            repo_url: None,
            base_commit: String::new(),
            default_agent_id: None,
            revision: 1,
        },
        agents: vec![],
        providers: vec![],
        sessions: vec![],
        messages: vec![],
    };
    let archive_path = directory.path().join("archive with spaces.json");
    std::fs::write(&archive_path, serde_json::to_vec(&archive).unwrap()).unwrap();
    let settings = default_settings();
    let settings_path = directory.path().join("settings.json");
    std::fs::write(&settings_path, serde_json::to_vec(&settings).unwrap()).unwrap();
    let config = AgentConfiguration {
        provider_id: "provider".into(),
        model: "model".into(),
        reasoning_effort: Some("high".into()),
    };
    let mut cases: Vec<(&str, Vec<&str>, Command)> = Vec::new();
    macro_rules! case {
        ($args:expr => $variant:ident $({ $($field:ident: $value:expr),* $(,)? })?) => {
            cases.push((stringify!($variant), $args.to_vec(), Command::$variant $({ $($field: $value),* })?));
        };
    }
    case!(&["project", "list"] => ListProjects);
    case!(&["project", "register", "--id", "id", "--name", "name", "--workdir", "workdir", "--repo-url", "repo_url"] => RegisterProject { id: "id".into(), name: "name".into(), workdir: Some("workdir".into()), repo_url: Some("repo_url".into()) });
    case!(&["project", "register", "--id", "named", "--name", "中文 project"] => RegisterProject { id: "named".into(), name: "中文 project".into(), workdir: None, repo_url: None });
    case!(&["project", "set-default-agent", "--project-id", "project_id", "--agent-id", "agent_id"] => SetProjectDefaultAgent { project_id: "project_id".into(), agent_id: "agent_id".into() });
    case!(&["agent", "list"] => ListAgents);
    case!(&["agent", "create", "--id", "id", "--name", "name", "--provider-id", "provider", "--model", "model", "--reasoning-effort", "high"] => RegisterAgent { id: "id".into(), name: "name".into(), config: config.clone() });
    case!(&["agent", "update", "--id", "id", "--name", "name", "--provider-id", "provider", "--model", "model", "--reasoning-effort", "high"] => UpdateAgent { id: "id".into(), name: "name".into(), config: config.clone() });
    case!(&["agent", "provider", "list"] => ListAgentProviders);
    case!(&["agent", "provider", "refresh-models", "--provider-id", "provider_id"] => RefreshProviderModels { provider_id: "provider_id".into() });
    case!(&["session", "list", "--project-id", "project_id"] => ListSessions { project_id: "project_id".into() });
    case!(&["session", "create", "--id", "id", "--project-id", "project_id", "--agent-id", "agent_id", "--at-message-id", "at_message_id"] => CreateSession { id: "id".into(), project_id: "project_id".into(), agent_id: "agent_id".into(), at_message_id: Some("at_message_id".into()) });
    case!(&["session", "set-agent", "--session-id", "session_id", "--agent-id", "agent_id"] => SetSessionAgent { session_id: "session_id".into(), agent_id: "agent_id".into() });
    case!(&["session", "set-config", "--session-id", "session_id", "--provider-id", "provider", "--model", "model", "--reasoning-effort", "high"] => SetSessionConfig { session_id: "session_id".into(), config: config.clone() });
    case!(&["session", "rename", "--session-id", "session_id", "--name", "name"] => RenameSession { session_id: "session_id".into(), name: "name".into() });
    case!(&["session", "set-title", "--session-id", "session_id", "--title", "title"] => SetSessionTitle { session_id: "session_id".into(), title: "title".into() });
    case!(&["session", "send", "--session-id", "session_id", "--text", "中文\n\\path"] => SendMessage { session_id: "session_id".into(), text: "中文\n\\path".into() });
    case!(&["session", "fork", "--id", "id", "--project-id", "project_id", "--agent-id", "agent_id", "--at-message-id", "at_message_id", "--text", "中文\n\\path"] => ForkSession { id: "id".into(), project_id: "project_id".into(), agent_id: "agent_id".into(), at_message_id: "at_message_id".into(), text: "中文\n\\path".into() });
    case!(&["session", "derive", "--id", "id", "--project-id", "project_id", "--source-session-id", "source_session_id", "--agent-id", "agent_id", "--at-message-id", "at_message_id", "--text", "中文\n\\path"] => DeriveSession { id: "id".into(), project_id: "project_id".into(), source_session_id: "source_session_id".into(), agent_id: "agent_id".into(), at_message_id: "at_message_id".into(), text: "中文\n\\path".into() });
    case!(&["message", "list", "--project-id", "project_id"] => ListMessages { project_id: "project_id".into() });
    case!(&["run", "list", "--project-id", "project_id"] => ListRuns { project_id: "project_id".into() });
    case!(&["run", "get", "--run-id", "run_id"] => GetRun { run_id: "run_id".into() });
    case!(&["run", "cancel", "--run-id", "run_id"] => CancelRun { run_id: "run_id".into() });
    case!(&["cron", "list"] => ListCrons);
    case!(&["cron", "create", "--id", "id", "--name", "name", "--project-id", "project_id", "--base-message-id", "base_message_id", "--agent-id", "agent_id", "--schedule", "schedule", "--timezone", "timezone"] => CreateCron { id: "id".into(), name: "name".into(), project_id: "project_id".into(), base_message_id: "base_message_id".into(), agent_id: "agent_id".into(), schedule: "schedule".into(), timezone: "timezone".into() });
    case!(&["cron", "enable", "--cron-id", "cron_id"] => SetCronEnabled { cron_id: "cron_id".into(), enabled: true });
    case!(&["cron", "disable", "--cron-id", "cron_id"] => SetCronEnabled { cron_id: "cron_id".into(), enabled: false });
    case!(&["cron", "trigger", "--cron-id", "cron_id", "--scheduled-at", "1788480000000"] => TriggerCron { cron_id: "cron_id".into(), scheduled_at: 1_788_480_000_000 });
    case!(&["settings", "get"] => GetSettings);
    case!(&["settings", "reset"] => ResetSettings);
    let provider = AgentProvider {
        id: "provider".into(),
        name: "Provider".into(),
        kind: AgentMode::DeepSeek,
        url: Some("https://api.deepseek.com/".into()),
        models: vec![ProviderModel {
            id: "m".into(),
            name: "Model".into(),
            reasoning_efforts: vec!["high".into()],
        }],
    };
    let models_path = directory.path().join("models with spaces.json");
    std::fs::write(&models_path, serde_json::to_vec(&provider.models).unwrap()).unwrap();
    case!(&["agent", "provider", "save", "--id", "provider", "--name", "Provider", "--kind", "deepseek", "--url", "https://api.deepseek.com", "--input", models_path.to_str().unwrap(), "--secret-stdin"] => SaveAgentProvider { provider: provider.clone(), secret: Some(ProviderSecret("fixture-secret".into())) });
    case!(&["agent", "provider", "discover-models", "--id", "provider", "--name", "Provider", "--kind", "deepseek", "--url", "https://api.deepseek.com", "--input", models_path.to_str().unwrap(), "--secret-stdin"] => DiscoverProviderModels { provider: provider.clone(), secret: Some(ProviderSecret("fixture-secret".into())) });
    case!(&["project", "export", "--project-id", "p", "--output", "archive with spaces.json"] => ExportProject { project_id: "p".into() });
    case!(&["project", "import", "--input", archive_path.to_str().unwrap(), "--workdir", "path with spaces"] => ImportProject { archive: archive, workdir: "path with spaces".into() });
    case!(&["settings", "set", "--expected-revision", "42", "--input", settings_path.to_str().unwrap()] => SaveSettings { expected_revision: 42, values: settings });
    case!(&["run", "approval", "approve", "--run-id", "r", "--approval-id", "a", "--scope", "turn"] => ResolveNativeApproval { run_id: "r".into(), approval_id: "a".into(), action: NativeApprovalAction::Approve, scope: Some(ApprovalGrantScope::Turn) });
    case!(&["run", "approval", "deny", "--run-id", "r", "--approval-id", "a"] => ResolveNativeApproval { run_id: "r".into(), approval_id: "a".into(), action: NativeApprovalAction::Deny, scope: None });
    case!(&["run", "approval", "cancel", "--run-id", "r", "--approval-id", "a"] => ResolveNativeApproval { run_id: "r".into(), approval_id: "a".into(), action: NativeApprovalAction::Cancel, scope: None });

    let documented = documented_routes();
    let mut covered = BTreeSet::new();
    for (variant, args, expected) in cases {
        covered.insert(variant.to_owned());
        let actual = match action(&args, "fixture-secret\n") {
            Action::Execute(command) => command,
            Action::Export { command, output } => {
                assert_eq!(output.to_str().unwrap(), "archive with spaces.json");
                command
            }
            Action::Events { .. } => panic!("unexpected SSE action"),
        };
        assert_eq!(actual, expected, "mapping for {variant}");
        assert!(
            documented
                .iter()
                .any(|(_, path)| path == crate::operation_path(&actual)),
            "undocumented CLI route for {variant}"
        );
    }
    // Read the actual enum syntax, rather than a second manually maintained count.
    // A new application variant must add a real invocation and expected DTO above.
    let syntax = syn::parse_file(include_str!("../../../crates/contracts/src/lib.rs")).unwrap();
    let variants = syntax
        .items
        .into_iter()
        .find_map(|item| match item {
            syn::Item::Enum(item) if item.ident == "Command" => Some(
                item.variants
                    .into_iter()
                    .map(|v| v.ident.to_string())
                    .collect::<BTreeSet<_>>(),
            ),
            _ => None,
        })
        .unwrap();
    assert_eq!(covered, variants);
}

#[test]
fn all_help_levels_are_discoverable_and_retired_entries_are_absent() {
    fn visit(command: &clap::Command, path: &[String]) {
        assert!(!command.is_hide_set());
        let mut args = path.to_owned();
        args.push("--help".into());
        let error = Arguments::try_parse_from(args).err().expect("help exits");
        assert_eq!(error.kind(), clap::error::ErrorKind::DisplayHelp);
        let help = error.to_string();
        assert!(help.contains("--host"));
        assert!(help.contains("--port"));
        assert!(!help.contains("--endpoint"));
        for child in command.get_subcommands() {
            assert!(help.contains(child.get_name()));
            let mut path = path.to_owned();
            path.push(child.get_name().into());
            visit(child, &path);
        }
    }
    let command = Arguments::command();
    command.clone().debug_assert();
    assert!(
        command
            .get_subcommands()
            .all(|command| !["command", "events", "agent-provider"].contains(&command.get_name()))
    );
    let agent = command.find_subcommand("agent").unwrap();
    let provider = agent.find_subcommand("provider").unwrap();
    assert_eq!(
        provider
            .get_subcommands()
            .map(clap::Command::get_name)
            .collect::<BTreeSet<_>>(),
        BTreeSet::from(["list", "save", "discover-models", "refresh-models"])
    );
    visit(&command, &["ait".into()]);
    for args in [
        vec!["ait", "command"],
        vec!["ait", "command", "-"],
        vec!["ait", "events"],
        vec!["ait", "agent-provider", "list"],
    ] {
        assert_eq!(
            Arguments::try_parse_from(args).err().unwrap().kind(),
            clap::error::ErrorKind::InvalidSubcommand
        );
    }
}

#[test]
fn required_parameters_and_typed_values_fail_before_io() {
    for args in [
        vec!["session", "send", "--session-id", "s"],
        vec![
            "session",
            "send",
            "--session-id",
            "s",
            "--text",
            "x",
            "--text-stdin",
        ],
        vec!["project", "register", "--id", "p"],
        vec!["run", "get", "--run-id", "  "],
        vec![
            "agent",
            "provider",
            "save",
            "--id",
            "p",
            "--name",
            "P",
            "--kind",
            "unsupported",
        ],
        vec![
            "cron",
            "trigger",
            "--cron-id",
            "c",
            "--scheduled-at",
            "yesterday",
        ],
        vec![
            "cron",
            "trigger",
            "--cron-id",
            "c",
            "--scheduled-at",
            "9223372036854775808",
        ],
        vec!["event", "list", "--after", "-1"],
        vec![
            "settings",
            "set",
            "--input",
            "-",
            "--expected-revision",
            "false",
        ],
        vec![
            "run",
            "approval",
            "approve",
            "--run-id",
            "r",
            "--approval-id",
            "a",
            "--scope",
            "all",
        ],
        vec![
            "run",
            "approval",
            "deny",
            "--run-id",
            "r",
            "--approval-id",
            "a",
            "--scope",
            "session",
        ],
    ] {
        assert!(Arguments::try_parse_from(std::iter::once("ait").chain(args)).is_err());
    }
}

#[test]
fn host_and_port_are_global_at_every_provider_command_level() {
    for index in 1..=4 {
        let mut args = vec!["ait", "agent", "provider", "list"];
        args.splice(index..index, ["--host", "localhost", "--port", "17314"]);
        let arguments = Arguments::try_parse_from(args).unwrap();
        assert_eq!(arguments.endpoint(), "http://localhost:17314");
        assert!(matches!(
            arguments
                .command
                .into_action(&mut "".as_bytes(), StdinSource::Redirected)
                .unwrap(),
            Action::Execute(Command::ListAgentProviders)
        ));
    }
    let arguments = Arguments::try_parse_from([
        "ait",
        "--host",
        "localhost",
        "agent",
        "provider",
        "--port",
        "17314",
        "list",
    ])
    .unwrap();
    assert_eq!(arguments.endpoint(), "http://localhost:17314");
}

#[test]
fn daemon_address_defaults_overrides_and_ipv6_use_http() {
    for (flags, expected) in [
        (vec![], "http://127.0.0.1:7314"),
        (vec!["--host", "localhost"], "http://localhost:7314"),
        (vec!["--port", "80"], "http://127.0.0.1:80"),
        (vec!["--port", "65535"], "http://127.0.0.1:65535"),
        (
            vec!["--host", "192.0.2.1", "--port", "1"],
            "http://192.0.2.1:1",
        ),
        (vec!["--host", "::1"], "http://[::1]:7314"),
        (
            vec!["--host", "[::1]", "--port", "17314"],
            "http://[::1]:17314",
        ),
        (vec!["--host", "2001:db8::1"], "http://[2001:db8::1]:7314"),
    ] {
        let arguments =
            Arguments::try_parse_from(["ait", "project", "list"].into_iter().chain(flags)).unwrap();
        assert_eq!(arguments.endpoint(), expected);
        let url = reqwest::Url::parse(&arguments.endpoint()).unwrap();
        assert_eq!(url.scheme(), "http");
    }
}

#[test]
fn malformed_daemon_addresses_and_endpoint_flag_are_rejected() {
    for host in [
        "",
        " ",
        "local host",
        "localhost\n",
        "http://localhost",
        "https://localhost",
        "localhost:7314",
        "user@localhost",
        "localhost/path",
        "localhost/",
        "localhost\\path",
        "localhost?query",
        "localhost#fragment",
        "%6cocalhost",
        "[::1]:7314",
        "[::1",
        "::g",
        "[localhost]",
        "256.0.0.1",
    ] {
        assert!(
            Arguments::try_parse_from(["ait", "project", "list", "--host", host]).is_err(),
            "accepted host {host:?}"
        );
    }
    for port in ["", "0", "65536", "-1", "http", "1.5"] {
        assert!(
            Arguments::try_parse_from(["ait", "project", "list", "--port", port]).is_err(),
            "accepted port {port:?}"
        );
    }
    assert!(
        Arguments::try_parse_from([
            "ait",
            "project",
            "list",
            "--endpoint",
            "http://localhost:7314"
        ])
        .is_err()
    );
}

#[test]
fn event_list_preserves_cursor_and_default() {
    assert!(matches!(
        action(&["event", "list", "--after", "12"], ""),
        Action::Events { after: 12 }
    ));
    assert!(matches!(
        action(&["event", "list"], ""),
        Action::Events { after: 0 }
    ));
}

#[test]
fn input_preserves_unicode_newlines_and_backslashes() {
    let text = "中文第一行\n第二行 C:\\path\n";
    for args in [
        vec!["session", "send", "--session-id", "s", "--text-stdin"],
        vec!["session", "send", "--session-id", "s", "--text-file", "-"],
    ] {
        assert!(
            matches!(action(&args, text), Action::Execute(Command::SendMessage { text: actual, .. }) if actual == text)
        );
    }
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("中文 text with spaces.txt");
    std::fs::write(&path, text).unwrap();
    assert!(
        matches!(action(&["session", "send", "--session-id", "s", "--text-file", path.to_str().unwrap()], ""), Action::Execute(Command::SendMessage { text: actual, .. }) if actual == text)
    );
}

#[test]
fn response_redaction_preserves_json_even_for_escaped_secrets() {
    for secret in ["sk-fixture-private", "sk-\"quoted\"\n\\secret", ":"] {
        let command = Command::SaveAgentProvider {
            provider: AgentProvider {
                id: "p".into(),
                name: "P".into(),
                kind: AgentMode::DeepSeek,
                url: None,
                models: vec![],
            },
            secret: Some(ProviderSecret(secret.into())),
        };
        let response = ait_contracts::Response::failure(ait_contracts::ApiError {
            code: ait_domain::ErrorCode::ProviderFailed,
            message: format!("upstream echoed {secret}"),
            retryable: true,
        });
        let output = crate::response_output(&response, &command);
        let parsed: ait_contracts::Response = serde_json::from_str(&output).unwrap();
        assert!(!parsed.ok);
        let error = parsed.error.unwrap();
        assert_eq!(error.message, "upstream echoed [REDACTED]");
        assert!(error.retryable);
    }
}

#[test]
fn approval_requires_a_scope_and_provider_stdin_is_not_shared() {
    assert!(
        Arguments::try_parse_from([
            "ait",
            "run",
            "approval",
            "approve",
            "--run-id",
            "r",
            "--approval-id",
            "a"
        ])
        .is_err()
    );
    let command = Arguments::try_parse_from([
        "ait",
        "agent",
        "provider",
        "save",
        "--id",
        "p",
        "--name",
        "P",
        "--kind",
        "deepseek",
        "--secret-stdin",
        "--input",
        "-",
    ])
    .unwrap()
    .command;
    let secret = "sk-fixture-private";
    let error = command
        .into_action(&mut Cursor::new(secret), StdinSource::Redirected)
        .err()
        .unwrap();
    assert!(!error.to_string().contains(secret));
    assert!(error.to_string().contains("cannot share stdin"));
}

#[test]
fn secret_stdin_rejects_terminal_before_reading() {
    for operation in ["save", "discover-models"] {
        let command = Arguments::try_parse_from([
            "ait",
            "agent",
            "provider",
            operation,
            "--id",
            "p",
            "--name",
            "P",
            "--kind",
            "deepseek",
            "--secret-stdin",
        ])
        .unwrap()
        .command;
        let mut stdin = Cursor::new("fixture-secret\n");
        let error = command
            .into_action(&mut stdin, StdinSource::Terminal)
            .err()
            .unwrap();
        assert_eq!(stdin.position(), 0, "a terminal secret must never be read");
        assert!(error.to_string().contains("requires redirected stdin"));
        assert!(!error.to_string().contains("fixture-secret"));
    }
}

#[test]
fn secret_stdin_accepts_injected_redirected_reader() {
    for operation in ["save", "discover-models"] {
        let result = action(
            &[
                "agent",
                "provider",
                operation,
                "--id",
                "p",
                "--name",
                "P",
                "--kind",
                "deepseek",
                "--secret-stdin",
            ],
            "fixture-secret\r\n",
        );
        let Action::Execute(
            Command::SaveAgentProvider { secret, .. }
            | Command::DiscoverProviderModels { secret, .. },
        ) = result
        else {
            panic!("expected Provider operation");
        };
        assert_eq!(secret, Some(ProviderSecret("fixture-secret".into())));
    }
}

fn documented_routes() -> BTreeSet<(String, String)> {
    let mut routes = BTreeSet::new();
    for line in include_str!("../../../docs/decisions/NEC-166/entity-operation-http-api.md").lines()
    {
        let cells: Vec<_> = line.split('|').map(str::trim).collect();
        if cells.len() < 4 || !matches!(cells[1], "`GET`" | "`POST`") {
            continue;
        }
        let method = cells[1].trim_matches('`').to_owned();
        let path = cells[2]
            .trim_matches('`')
            .split('?')
            .next()
            .unwrap()
            .to_owned();
        assert!(
            routes.insert((method, path)),
            "duplicate documented route: {line}"
        );
    }
    assert!(!routes.is_empty());
    routes
}

#[test]
fn documented_http_methods_and_paths_match_the_router() {
    use syn::visit::Visit;

    #[derive(Default)]
    struct Routes(BTreeSet<(String, String)>);
    impl<'ast> Visit<'ast> for Routes {
        fn visit_expr_method_call(&mut self, node: &'ast syn::ExprMethodCall) {
            if node.method == "route" {
                assert_eq!(node.args.len(), 2);
                let syn::Expr::Lit(syn::ExprLit {
                    lit: syn::Lit::Str(path),
                    ..
                }) = &node.args[0]
                else {
                    panic!(
                        "route collector requires a literal path; update it for the new router shape"
                    );
                };
                let syn::Expr::Call(call) = &node.args[1] else {
                    panic!("update route collector for chained method routers");
                };
                let syn::Expr::Path(function) = &*call.func else {
                    panic!("expected a routing function");
                };
                let method = function
                    .path
                    .get_ident()
                    .unwrap()
                    .to_string()
                    .to_uppercase();
                assert!(
                    matches!(method.as_str(), "GET" | "POST"),
                    "extend method collection: {method}"
                );
                assert!(
                    self.0.insert((method, path.value())),
                    "duplicate router path"
                );
            }
            syn::visit::visit_expr_method_call(self, node);
        }
    }
    let syntax = syn::parse_file(include_str!("../../../crates/api-http/src/lib.rs")).unwrap();
    let router = syntax
        .items
        .iter()
        .find_map(|item| match item {
            syn::Item::Fn(function) if function.sig.ident == "router_with_telemetry" => {
                Some(function)
            }
            _ => None,
        })
        .expect("the authoritative router function must exist");
    let mut collected = Routes::default();
    collected.visit_item_fn(router);
    assert_eq!(
        documented_routes(),
        collected.0,
        "update NEC-166 when HTTP routes change"
    );
}
