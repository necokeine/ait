use serde_json::json;

use super::{
    GithubCloneProtocol, GithubRepositorySearchRequest, GithubRepositorySearchResult,
    GithubRepositorySearchStatus, ProjectGithubCloneRequest,
};

#[test]
fn parses_paseo_github_requests_under_canonical_envelope_params() {
    let search: GithubRepositorySearchRequest = serde_json::from_value(json!({
        "query": "paseo", "limit": 12
    }))
    .expect("search request");
    assert_eq!(search.limit, Some(12));
    let clone: ProjectGithubCloneRequest = serde_json::from_value(json!({
        "repo": "a/b", "cloneProtocol": "https", "targetDirectory": "~/workspace"
    }))
    .expect("clone request");
    assert_eq!(clone.clone_protocol, Some(GithubCloneProtocol::Https));
    assert_eq!(clone.target_directory, "~/workspace");
}

#[test]
fn rejects_unknown_clone_protocol_and_serializes_missing_cli_status() {
    assert!(
        serde_json::from_value::<ProjectGithubCloneRequest>(json!({
            "repo":"a/b", "cloneProtocol":"ftp", "targetDirectory":"/tmp"
        }))
        .is_err()
    );
    let value = serde_json::to_value(GithubRepositorySearchResult {
        status: GithubRepositorySearchStatus::Unavailable,
        repositories: Vec::new(),
        available: false,
        reason: Some("gh_missing"),
        error: Some("GitHub CLI (gh) is not installed or not in PATH".to_owned()),
    })
    .expect("search result");
    assert_eq!(value["status"], "unavailable");
    assert_eq!(value["reason"], "gh_missing");
    assert_eq!(value["available"], false);
}
