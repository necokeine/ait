use serde_json::json;

use super::*;

#[test]
fn decodes_canonical_requests_and_legacy_search_kinds() {
    let search: ForgeSearchRequest = serde_json::from_value(json!({
        "cwd":"/repo",
        "query":"bug",
        "limit":25,
        "kinds":["github-issue","pr"]
    }))
    .unwrap();
    assert_eq!(
        search.kinds,
        Some(vec![ForgeSearchKind::GithubIssue, ForgeSearchKind::Pr])
    );
    let check: CheckDetailsRequest = serde_json::from_value(json!({
        "cwd":"/repo",
        "repoOwner":"acme",
        "repoName":"app",
        "checkRunId":9,
        "workflowRunId":12,
        "changeRequestNumber":3
    }))
    .unwrap();
    assert_eq!(check.check_run_id, Some(9));
    assert_eq!(check.change_request_number, Some(3));
}

#[test]
fn serializes_paseo_status_timeline_and_check_shapes() {
    let status = PullRequestStatusResult {
        cwd: "/repo".to_owned(),
        status: Some(PullRequestStatus {
            forge: "github".to_owned(),
            project_path: Some("acme/app".to_owned()),
            number: Some(7),
            url: "https://github.com/acme/app/pull/7".to_owned(),
            title: "Fix".to_owned(),
            state: "open".to_owned(),
            base_ref_name: "main".to_owned(),
            head_ref_name: "fix".to_owned(),
            is_merged: false,
            is_draft: false,
            mergeable: PullRequestMergeable::Mergeable,
            checks: vec![PullRequestCheck {
                name: "test".to_owned(),
                status: "success".to_owned(),
                url: None,
                workflow: None,
                duration: Some("2m".to_owned()),
                check_run_id: Some(8),
                workflow_run_id: None,
                traits: None,
            }],
            checks_status: "success".to_owned(),
            review_decision: Some("approved".to_owned()),
            repo_owner: Some("acme".to_owned()),
            repo_name: Some("app".to_owned()),
            github: None,
            forge_specific: Some(json!({"forge":"github"})),
        }),
        github_features_enabled: true,
        auth_state: Some(ForgeAuthState::Authenticated),
        forge: Some("github".to_owned()),
        error: None,
    };
    let value = serde_json::to_value(status).unwrap();
    assert_eq!(value["status"]["mergeable"], "MERGEABLE");
    assert_eq!(value["status"]["checks"][0]["checkRunId"], 8);
    assert_eq!(value["authState"], "authenticated");
    assert!(value["status"].get("github").is_none());

    let timeline = PullRequestTimelineResult {
        cwd: "/repo".to_owned(),
        pr_number: Some(7),
        items: vec![PullRequestTimelineItem::Review {
            id: "R1".to_owned(),
            author: "octo".to_owned(),
            author_url: None,
            avatar_url: None,
            body: "ok".to_owned(),
            created_at: 10,
            url: "https://example/review".to_owned(),
            review_state: TimelineReviewState::Approved,
        }],
        truncated: false,
        error: None,
        github_features_enabled: true,
        auth_state: Some(ForgeAuthState::Authenticated),
    };
    let value = serde_json::to_value(timeline).unwrap();
    assert_eq!(value["items"][0]["kind"], "review");
    assert_eq!(value["items"][0]["reviewState"], "approved");
}

#[test]
fn capability_names_are_canonical_and_complete() {
    assert_eq!(CAPABILITIES.len(), 10);
    assert!(
        CAPABILITIES
            .iter()
            .all(|method| method.ends_with(".request"))
    );
    assert!(
        CAPABILITIES
            .iter()
            .all(|method| !method.ends_with("_request"))
    );
}
