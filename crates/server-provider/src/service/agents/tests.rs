use super::{AgentError, validate_key};

#[test]
fn retry_keys_keep_their_bounds_without_the_retired_project_service() {
    for key in ["create-agent", "!~", &"a".repeat(128)] {
        assert_eq!(validate_key(key), Ok(()));
    }
    for key in [
        "",
        "white space",
        "line\n",
        "control\u{7f}",
        "é",
        &"a".repeat(129),
    ] {
        assert_eq!(validate_key(key), Err(AgentError::Invalid));
    }
}
