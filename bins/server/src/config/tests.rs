use super::*;

const TOKEN: &str = "offline-config-test-token-longer-than-32";

fn environment(name: &str) -> Option<OsString> {
    match name {
        "AIT_SERVER_TOKEN" => Some(TOKEN.into()),
        "HOME" => Some("/unused-home-for-config-test".into()),
        _ => None,
    }
}

#[test]
fn defaults_are_isolated_and_secrets_are_redacted() {
    let config = Config::load(Cli::parse_from(["server"]), environment).unwrap();
    assert_eq!(
        config.data_dir,
        PathBuf::from("/unused-home-for-config-test/.ait-server")
    );
    assert_eq!(config.listen, "127.0.0.1:7316".parse().unwrap());
    assert!(!format!("{config:?}").contains(TOKEN));
    assert!(Config::load(Cli::parse_from(["server"]), |_| None).is_err());
}

#[test]
fn cli_overrides_environment_which_overrides_file() {
    let directory = tempfile::tempdir().unwrap();
    std::fs::write(
        directory.path().join("config.toml"),
        "listen = '127.0.0.1:7001'\nlog_level = 'warn'\n",
    )
    .unwrap();
    let args = ["server", "--data-dir", directory.path().to_str().unwrap()];
    let file = Config::load(Cli::parse_from(args), environment).unwrap();
    assert_eq!(file.listen.port(), 7001);
    assert_eq!(file.log_level, tracing::level_filters::LevelFilter::WARN);
    let env = |name: &str| match name {
        "AIT_SERVER_LISTEN" => Some("127.0.0.1:7002".into()),
        "AIT_SERVER_LOG_LEVEL" => Some("debug".into()),
        "AIT_SERVER_DATA_DIR" => Some(directory.path().as_os_str().to_owned()),
        _ => environment(name),
    };
    let config = Config::load(Cli::parse_from(["server"]), env).unwrap();
    assert_eq!(config.listen.port(), 7002);
    assert_eq!(config.log_level, tracing::level_filters::LevelFilter::DEBUG);
    let args = [
        "server",
        "--data-dir",
        directory.path().to_str().unwrap(),
        "--listen",
        "[::1]:0",
        "--log-level",
        "error",
    ];
    let cli = Config::load(Cli::parse_from(args), env).unwrap();
    assert_eq!(cli.listen, "[::1]:0".parse().unwrap());
    assert_eq!(cli.log_level, tracing::level_filters::LevelFilter::ERROR);
}

#[test]
fn invalid_configuration_fails_without_creating_state() {
    let directory = tempfile::tempdir().unwrap();
    let missing = directory.path().join("not-created");
    for args in [
        vec![
            "server",
            "--data-dir",
            missing.to_str().unwrap(),
            "--listen",
            "0.0.0.0:7316",
        ],
        vec!["server", "--config", missing.to_str().unwrap()],
        vec!["server", "--log-level", "invalid"],
    ] {
        assert!(Config::load(Cli::parse_from(args), environment).is_err());
    }
    assert!(Cli::try_parse_from(["server", "--data-dir", ""]).is_err());
    assert!(
        Config::load(Cli::parse_from(["server"]), |name| {
            if name == "AIT_SERVER_DATA_DIR" {
                Some(OsString::new())
            } else {
                environment(name)
            }
        })
        .is_err()
    );
    assert!(!missing.exists());
    for contents in [
        "bad toml",
        "token = 'never-supported'",
        "listen = '0.0.0.0:7316'",
    ] {
        std::fs::write(directory.path().join("config.toml"), contents).unwrap();
        assert!(
            Config::load(
                Cli::parse_from(["server", "--data-dir", directory.path().to_str().unwrap()]),
                environment
            )
            .is_err()
        );
    }
    std::fs::write(
        directory.path().join("config.toml"),
        format!("token = '{TOKEN}'"),
    )
    .unwrap();
    let error = Config::load(
        Cli::parse_from(["server", "--data-dir", directory.path().to_str().unwrap()]),
        environment,
    )
    .unwrap_err();
    assert!(!format!("{error:#}").contains(TOKEN));
    assert!(
        Config::load(Cli::parse_from(["server"]), |name| {
            if name == "AIT_SERVER_TOKEN" {
                Some("short".into())
            } else {
                environment(name)
            }
        })
        .is_err()
    );
    assert!(
        Config::load(Cli::parse_from(["server"]), |name| {
            if name == "HOME" {
                None
            } else {
                environment(name)
            }
        })
        .is_err()
    );
    assert!(
        Config::load(Cli::parse_from(["server"]), |name| {
            if name == "AIT_SERVER_LISTEN" {
                Some("invalid".into())
            } else {
                environment(name)
            }
        })
        .is_err()
    );
}
