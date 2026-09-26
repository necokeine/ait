use std::collections::BTreeMap;

use serde_json::Value;

use crate::ports::agent_session::AgentSessionError;

enum Rule {
    Boolean,
    Text,
    Strings,
    Port,
    Choice(&'static [&'static str]),
    Object(&'static [(&'static str, Rule)]),
    Map(&'static Rule),
    Either(&'static Rule, &'static Rule),
    Required(&'static [&'static str], &'static Rule),
}

impl Rule {
    fn accepts(&self, value: &Value) -> bool {
        match self {
            Self::Boolean => value.is_boolean(),
            Self::Text => value.as_str().is_some_and(super::text_value),
            Self::Strings => super::strings(value),
            Self::Port => value
                .as_u64()
                .is_some_and(|port| (1..=65535).contains(&port)),
            Self::Choice(choices) => value.as_str().is_some_and(|value| choices.contains(&value)),
            Self::Object(fields) => value
                .as_object()
                .is_some_and(|values| values.iter().all(|(key, value)| field(fields, key, value))),
            Self::Map(rule) => value.as_object().is_some_and(|values| {
                values.len() <= 1024
                    && values
                        .iter()
                        .all(|(key, value)| super::identifier(key) && rule.accepts(value))
            }),
            Self::Either(left, right) => left.accepts(value) || right.accepts(value),
            Self::Required(keys, rule) => {
                keys.iter().all(|key| value.get(key).is_some()) && rule.accepts(value)
            }
        }
    }
}

fn field(fields: &[(&str, Rule)], key: &str, value: &Value) -> bool {
    fields
        .iter()
        .find(|(name, _)| *name == key)
        .is_some_and(|(_, rule)| rule.accepts(value))
}

pub(super) fn validate(
    options: &BTreeMap<String, Value>,
    provider: &str,
) -> Result<(), AgentSessionError> {
    let fields = match provider {
        "codex" => CODEX,
        "claude" => CLAUDE,
        _ => return Err(AgentSessionError::Rejected),
    };
    if options.iter().all(|(key, value)| field(fields, key, value)) {
        Ok(())
    } else {
        Err(AgentSessionError::Rejected)
    }
}

const CODEX: &[(&str, Rule)] = &[
    (
        "approval_policy",
        Rule::Either(
            &Rule::Choice(&["untrusted", "on-request", "never"]),
            &Rule::Required(
                &["granular"],
                &Rule::Object(&[(
                    "granular",
                    Rule::Object(&[
                        ("sandbox_approval", Rule::Boolean),
                        ("rules", Rule::Boolean),
                        ("mcp_elicitations", Rule::Boolean),
                        ("request_permissions", Rule::Boolean),
                        ("skill_approval", Rule::Boolean),
                    ]),
                )]),
            ),
        ),
    ),
    (
        "sandbox_mode",
        Rule::Choice(&["read-only", "workspace-write", "danger-full-access"]),
    ),
    (
        "sandbox_workspace_write",
        Rule::Object(&[
            ("writable_roots", Rule::Strings),
            ("network_access", Rule::Boolean),
            ("exclude_slash_tmp", Rule::Boolean),
            ("exclude_tmpdir_env_var", Rule::Boolean),
        ]),
    ),
    (
        "web_search",
        Rule::Choice(&["disabled", "cached", "indexed", "live"]),
    ),
    (
        "features",
        Rule::Object(&[
            ("multi_agent_v2", Rule::Boolean),
            (
                "network_proxy",
                Rule::Either(
                    &Rule::Boolean,
                    &Rule::Object(&[
                        ("enabled", Rule::Boolean),
                        ("proxy_url", Rule::Text),
                        ("socks_url", Rule::Text),
                        ("enable_socks5", Rule::Boolean),
                        ("enable_socks5_udp", Rule::Boolean),
                        ("allow_local_binding", Rule::Boolean),
                        ("allow_upstream_proxy", Rule::Boolean),
                        ("dangerously_allow_all_unix_sockets", Rule::Boolean),
                        ("dangerously_allow_non_loopback_proxy", Rule::Boolean),
                        ("domains", Rule::Map(&Rule::Choice(&["allow", "deny"]))),
                        ("unix_sockets", Rule::Map(&Rule::Choice(&["allow", "deny"]))),
                    ]),
                ),
            ),
        ]),
    ),
];

const CLAUDE_NETWORK: &[(&str, Rule)] = &[
    ("allowedDomains", Rule::Strings),
    ("deniedDomains", Rule::Strings),
    ("strictAllowlist", Rule::Boolean),
    ("allowManagedDomainsOnly", Rule::Boolean),
    ("allowUnixSockets", Rule::Strings),
    ("allowAllUnixSockets", Rule::Boolean),
    ("allowLocalBinding", Rule::Boolean),
    ("allowMachLookup", Rule::Strings),
    ("httpProxyPort", Rule::Port),
    ("socksProxyPort", Rule::Port),
    (
        "tlsTerminate",
        Rule::Object(&[("caCertPath", Rule::Text), ("caKeyPath", Rule::Text)]),
    ),
];

const CLAUDE_FILESYSTEM: &[(&str, Rule)] = &[
    ("allowWrite", Rule::Strings),
    ("denyWrite", Rule::Strings),
    ("allowRead", Rule::Strings),
    ("denyRead", Rule::Strings),
    ("allowManagedReadPathsOnly", Rule::Boolean),
    ("disabled", Rule::Boolean),
];

const CLAUDE_SANDBOX: &[(&str, Rule)] = &[
    ("enabled", Rule::Boolean),
    ("failIfUnavailable", Rule::Boolean),
    ("autoAllowBashIfSandboxed", Rule::Boolean),
    ("excludedCommands", Rule::Strings),
    ("allowUnsandboxedCommands", Rule::Boolean),
    ("network", Rule::Object(CLAUDE_NETWORK)),
    ("filesystem", Rule::Object(CLAUDE_FILESYSTEM)),
    ("ignoreViolations", Rule::Map(&Rule::Strings)),
    ("enableWeakerNestedSandbox", Rule::Boolean),
    (
        "ripgrep",
        Rule::Required(
            &["command"],
            &Rule::Object(&[("command", Rule::Text), ("args", Rule::Strings)]),
        ),
    ),
];

const CLAUDE: &[(&str, Rule)] = &[
    ("allowedTools", Rule::Strings),
    ("disallowedTools", Rule::Strings),
    ("additionalDirectories", Rule::Strings),
    ("sandbox", Rule::Object(CLAUDE_SANDBOX)),
    (
        "settings",
        Rule::Object(&[
            (
                "permissions",
                Rule::Object(&[
                    ("allow", Rule::Strings),
                    ("ask", Rule::Strings),
                    ("deny", Rule::Strings),
                ]),
            ),
            ("sandbox", Rule::Object(CLAUDE_SANDBOX)),
        ]),
    ),
];
