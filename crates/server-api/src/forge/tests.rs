use serde_json::{Value, json};
use server_application::forge::*;

use super::execute;

#[derive(Debug)]
struct FakeForge;

impl ForgeRuntime for FakeForge {
    fn search(
        &self,
        _cwd: &str,
        _query: &str,
        _limit: usize,
        _kinds: &[ForgeSearchKind],
    ) -> Result<ForgeSearch, ForgeRuntimeError> {
        Ok(ForgeSearch {
            items: vec![ForgeSearchItem {
                kind: ForgeSearchKind::ChangeRequest,
                forge: Some("github".to_owned()),
                number: 4,
                title: "Fix".to_owned(),
                url: "https://github.com/acme/app/pull/4".to_owned(),
                state: "open".to_owned(),
                body: None,
                labels: vec!["bug".to_owned()],
                project_path: Some("acme/app".to_owned()),
                base_ref_name: Some("main".to_owned()),
                head_ref_name: Some("fix".to_owned()),
                updated_at: Some("2026-01-01T00:00:00Z".to_owned()),
            }],
            auth_state: ForgeAuthState::Authenticated,
        })
    }

    fn create_pull_request(
        &self,
        _cwd: &str,
        _title: &str,
        _body: &str,
        _base_ref: Option<&str>,
    ) -> Result<PullRequestCreated, ForgeRuntimeError> {
        Ok(PullRequestCreated {
            url: "https://github.com/acme/app/pull/4".to_owned(),
            number: 4,
        })
    }

    fn current_pull_request_status(
        &self,
        _cwd: &str,
    ) -> Result<PullRequestStatusRead, ForgeRuntimeError> {
        Ok(PullRequestStatusRead {
            status: Some(PullRequestStatus {
                forge: "github".to_owned(),
                project_path: Some("acme/app".to_owned()),
                number: Some(4),
                url: "https://github.com/acme/app/pull/4".to_owned(),
                title: "Fix".to_owned(),
                state: "open".to_owned(),
                base_ref_name: "main".to_owned(),
                head_ref_name: "fix".to_owned(),
                is_merged: false,
                is_draft: false,
                mergeable: PullRequestMergeable::Mergeable,
                checks: Vec::new(),
                checks_status: "none".to_owned(),
                review_decision: None,
                repo_owner: Some("acme".to_owned()),
                repo_name: Some("app".to_owned()),
                github: None,
                forge_specific: None,
            }),
            auth_state: ForgeAuthState::Authenticated,
            forge: Some("github".to_owned()),
        })
    }

    fn merge_current_pull_request(
        &self,
        _cwd: &str,
        _merge_method: PullRequestMergeMethod,
    ) -> Result<(), ForgeRuntimeError> {
        Ok(())
    }

    fn set_current_pull_request_auto_merge(
        &self,
        _cwd: &str,
        _enabled: bool,
        _merge_method: Option<PullRequestMergeMethod>,
    ) -> Result<(), ForgeRuntimeError> {
        Ok(())
    }

    fn pull_request_timeline(
        &self,
        _cwd: &str,
        pr_number: u64,
        _repo_owner: &str,
        _repo_name: &str,
    ) -> Result<PullRequestTimeline, ForgeRuntimeError> {
        Ok(PullRequestTimeline {
            pr_number,
            items: vec![PullRequestTimelineItem::Review {
                id: "R1".to_owned(),
                author: "octo".to_owned(),
                author_url: None,
                avatar_url: None,
                body: "ship".to_owned(),
                created_at: 1,
                url: "https://example/review".to_owned(),
                review_state: TimelineReviewState::Approved,
            }],
            truncated: false,
            error: None,
            auth_state: ForgeAuthState::Authenticated,
        })
    }

    fn check_details(
        &self,
        _cwd: &str,
        _repo_owner: Option<&str>,
        _repo_name: Option<&str>,
        check_run_id: Option<u64>,
        workflow_run_id: Option<u64>,
        _change_request_number: Option<u64>,
    ) -> Result<CheckDetails, ForgeRuntimeError> {
        Ok(CheckDetails {
            check_run_id: check_run_id.unwrap(),
            workflow_run_id,
            name: "tests".to_owned(),
            status: Some("completed".to_owned()),
            conclusion: Some("success".to_owned()),
            url: None,
            details_url: None,
            output: None,
            annotations: Vec::new(),
            failed_jobs: Vec::new(),
            truncated: false,
            pipeline: None,
        })
    }
}

fn forge() -> Forge {
    Forge::new(Box::new(FakeForge))
}

#[test]
fn projects_neutral_and_github_search_shapes() {
    let neutral = execute(
        &forge(),
        "forge.search.request",
        json!({"cwd":"/repo","query":"fix","kinds":["github-pr"]}),
    )
    .unwrap();
    assert_eq!(neutral["items"][0]["kind"], "change_request");
    assert_eq!(neutral["authState"], "authenticated");
    let legacy = execute(
        &forge(),
        "github.search.request",
        json!({"cwd":"/repo","query":"fix"}),
    )
    .unwrap();
    assert_eq!(legacy["items"][0]["kind"], "pr");
    assert_eq!(legacy["featuresEnabled"], true);
    assert_eq!(legacy["githubFeaturesEnabled"], true);
}

#[test]
fn serves_pr_mutations_status_timeline_and_check_details() {
    let created = execute(
        &forge(),
        "checkout.pr.create.request",
        json!({"cwd":"/repo","title":"Fix","body":"Details"}),
    )
    .unwrap();
    assert_eq!(created["number"], 4);
    assert!(created["error"].is_null());
    let merged = execute(
        &forge(),
        "checkout.pr.merge.request",
        json!({"cwd":"/repo","mergeMethod":"squash"}),
    )
    .unwrap();
    assert_eq!(merged["success"], true);
    let auto = execute(
        &forge(),
        "checkout.forge.set_auto_merge.request",
        json!({"cwd":"/repo","enabled":true,"mergeMethod":"rebase"}),
    )
    .unwrap();
    assert_eq!(auto["enabled"], true);
    let status = execute(
        &forge(),
        "checkout.pr.status.request",
        json!({"cwd":"/repo"}),
    )
    .unwrap();
    assert_eq!(status["status"]["number"], 4);
    let timeline = execute(
        &forge(),
        "checkout.pr.timeline.request",
        json!({"cwd":"/repo","prNumber":4,"repoOwner":"acme","repoName":"app"}),
    )
    .unwrap();
    assert_eq!(timeline["items"][0]["reviewState"], "approved");
    let details = execute(
        &forge(),
        "checkout.github.get_check_details.request",
        json!({"cwd":"/repo","repoOwner":"acme","repoName":"app","checkRunId":9}),
    )
    .unwrap();
    assert_eq!(details["details"]["checkRunId"], 9);
}

#[test]
fn keeps_paseo_inline_errors_for_invalid_application_shapes() {
    let created = execute(
        &forge(),
        "checkout.pr.create.request",
        json!({"cwd":"/repo"}),
    )
    .unwrap();
    assert_eq!(created["url"], Value::Null);
    assert_eq!(created["error"]["code"], "UNKNOWN");
    let auto = execute(
        &forge(),
        "checkout.github.set_auto_merge.request",
        json!({"cwd":"/repo","enabled":true}),
    )
    .unwrap();
    assert_eq!(auto["success"], false);
    assert!(
        auto["error"]["message"]
            .as_str()
            .unwrap()
            .contains("mergeMethod")
    );
    let timeline = execute(
        &forge(),
        "checkout.pr.timeline.request",
        json!({"cwd":"/repo","prNumber":0,"repoOwner":"bad/owner","repoName":"app"}),
    )
    .unwrap();
    assert_eq!(timeline["error"]["kind"], "unknown");
}
