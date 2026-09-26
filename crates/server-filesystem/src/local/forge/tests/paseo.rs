//! Observable GitHub payload contracts from Paseo services/github-service.test.ts.
use super::*;
use serde_json::json;

fn status(fields: Value) -> PullRequestStatus {
    let mut value = json!({"number":42,"url":"https://github.example/parent/project/pull/42",
        "title":"Change","state":"OPEN"});
    let Value::Object(fields) = fields else {
        panic!("status fields must be an object")
    };
    value.as_object_mut().unwrap().extend(fields);
    parse_status(
        &value.to_string(),
        "local-head",
        &ForgeContext {
            host: "github.example".into(),
            project_path: "fork/project".into(),
        },
    )
    .unwrap()
    .unwrap()
}

fn timeline(pr: Value) -> PullRequestTimeline {
    let mut response = json!({"data":{"repository":{}}});
    response["data"]["repository"]["pullRequest"] = pr;
    parse_timeline(&response.to_string(), 42).unwrap()
}

#[test]
fn enterprise_status_uses_pull_url_identity_and_preserves_checkout_head_fallback() {
    let result = status(json!({"isDraft":true}));
    assert_eq!(result.repo_owner.as_deref(), Some("parent"));
    assert_eq!(result.repo_name.as_deref(), Some("project"));
    assert_eq!(result.project_path.as_deref(), Some("fork/project"));
    assert_eq!(result.head_ref_name, "local-head");
    assert!(result.is_draft);
    assert_eq!(result.number, Some(42));
}

#[test]
fn check_rollup_failure_dominates_pending_and_supports_graphql_context_nodes() {
    let cases = [
        ("FAILURE", "failure"),
        ("IN_PROGRESS", "pending"),
        ("SUCCESS", "success"),
    ];
    for (state, expected) in cases {
        let result = status(json!({"statusCheckRollup":{"contexts":{"nodes":[
            {"__typename":"StatusContext","context":"legacy","state":"SUCCESS"},
            {"__typename":"CheckRun","name":"tests","status":if state == "IN_PROGRESS" {"IN_PROGRESS"} else {"COMPLETED"},"conclusion":state}
        ]}}}));
        assert_eq!(result.checks_status, expected);
        assert_eq!(result.checks.len(), 2);
    }
    let mixed = status(json!({"statusCheckRollup":[
        {"__typename":"StatusContext","context":"legacy","state":"ERROR"},
        {"__typename":"CheckRun","name":"tests","status":"IN_PROGRESS"}
    ]}));
    assert_eq!(mixed.checks_status, "failure");
    assert_eq!(status(json!({})).checks_status, "none");
}

#[test]
fn status_check_metadata_includes_workflow_ids_and_formatted_duration() {
    let result = status(
        json!({"statusCheckRollup":[{"__typename":"CheckRun","name":"tests",
        "status":"COMPLETED","conclusion":"SUCCESS","databaseId":9,"workflowName":"CI",
        "checkSuite":{"workflowRun":{"databaseId":44}},"detailsUrl":"https://example/check",
        "startedAt":"2026-01-01T00:00:00Z","completedAt":"2026-01-01T01:02:03Z"}]}),
    );
    let check = &result.checks[0];
    assert_eq!(check.workflow.as_deref(), Some("CI"));
    assert_eq!(check.duration.as_deref(), Some("1h 2m 3s"));
    assert_eq!(check.check_run_id, Some(9));
    assert_eq!(check.workflow_run_id, Some(44));
    assert_eq!(check.url.as_deref(), Some("https://example/check"));
}

#[test]
fn unexpected_mergeability_stays_unknown_while_review_decisions_are_normalized() {
    for value in [Value::Null, json!("unexpected"), json!(42)] {
        let result = status(json!({"mergeable":value,"reviewDecision":"REVIEW_REQUIRED"}));
        assert_eq!(result.mergeable, PullRequestMergeable::Unknown);
        assert_eq!(result.review_decision.as_deref(), Some("pending"));
    }
    let merged = status(json!({"state":"CLOSED","mergedAt":"2026-01-01T00:00:00Z"}));
    assert!(merged.is_merged);
    assert_eq!(merged.state, "merged");
}

#[test]
fn inline_comments_are_deduplicated_without_losing_review_and_location_identity() {
    let comment = json!({"id":"comment","body":"inline","createdAt":"2026-01-01T00:00:00Z",
        "pullRequestReview":{"id":"review"},"author":{"login":"author"}});
    let result = timeline(
        json!({"comments":{"nodes":[comment.clone()]},"reviewThreads":{"nodes":[
            {"id":"thread","path":"src/lib.rs","line":24,"startLine":20,"isResolved":true,
            "isOutdated":false,"comments":{"nodes":[comment]}}
        ]}}),
    );
    assert_eq!(result.items.len(), 1);
    let PullRequestTimelineItem::Comment {
        review_id,
        location: Some(location),
        ..
    } = &result.items[0]
    else {
        panic!("inline comment missing")
    };
    assert_eq!(review_id.as_deref(), Some("review"));
    assert_eq!(location.thread_id.as_deref(), Some("thread"));
    assert_eq!(location.path, "src/lib.rs");
    assert_eq!((location.start_line, location.line), (Some(20), Some(24)));
    assert_eq!(location.is_resolved, Some(true));
}

#[test]
fn timeline_sorts_reviews_and_comments_chronologically_with_stable_id_ties() {
    let result = timeline(
        json!({"reviews":{"nodes":[{"id":"z","state":"APPROVED","submittedAt":"2026-01-02T00:00:00Z"}]},
        "comments":{"nodes":[{"id":"b","createdAt":"2026-01-01T00:00:00Z","author":null},
        {"id":"a","createdAt":"2026-01-01T00:00:00Z"}]}}),
    );
    assert_eq!(
        result
            .items
            .iter()
            .map(|item| timeline_key(item).1)
            .collect::<Vec<_>>(),
        ["a", "b", "z"]
    );
    let PullRequestTimelineItem::Comment { author, .. } = &result.items[0] else {
        panic!("comment missing")
    };
    assert_eq!(author, "unknown");
}

#[test]
fn every_timeline_connection_including_nested_thread_comments_can_mark_truncation() {
    for field in ["reviews", "comments", "reviewThreads"] {
        let mut pr = json!({});
        pr[field] = json!({"nodes":[],"pageInfo":{"hasNextPage":true}});
        assert!(timeline(pr).truncated, "{field}");
    }
    assert!(timeline(json!({"reviewThreads":{"nodes":[{"comments":{"nodes":[],"pageInfo":{"hasNextPage":true}}}]}})).truncated);
    assert!(!timeline(json!({})).truncated);
}

#[test]
fn unsupported_empty_reviews_are_omitted_but_meaningful_review_bodies_are_retained() {
    let result = timeline(json!({"reviews":{"nodes":[
        {"id":"empty","state":"PENDING","body":"  "},
        {"id":"body","state":"DISMISSED","body":"Explanation"},
        {"id":"approved","state":"APPROVED","body":""}
    ]}}));
    assert_eq!(result.items.len(), 2);
    let PullRequestTimelineItem::Review { review_state, .. } = &result.items[1] else {
        panic!("review missing")
    };
    assert_eq!(*review_state, TimelineReviewState::Commented);
}

#[test]
fn failed_jobs_include_actionable_conclusions_and_cap_the_result_at_five() {
    let jobs: Vec<_> = ["success", "failure", "timed_out", "action_required", "cancelled", "failure", "failure"]
        .iter().enumerate().map(|(index,conclusion)|json!({"id":index+1,"name":format!("job-{index}"),"conclusion":conclusion})).collect();
    let (failed, truncated) = parse_failed_jobs(&json!({"jobs":jobs})).unwrap();
    assert_eq!(
        failed.iter().map(|job| job.job_id).collect::<Vec<_>>(),
        [2, 3, 4, 5, 6]
    );
    assert!(truncated);
    assert!(failed.iter().all(|job| job.log_tail.is_none()));
}

#[test]
fn full_workflow_page_marks_truncation_even_when_all_jobs_succeeded() {
    let jobs = vec![json!({"id":1,"name":"ok","conclusion":"success"}); 100];
    let (failed, truncated) = parse_failed_jobs(&json!({"jobs":jobs})).unwrap();
    assert!(failed.is_empty());
    assert!(truncated);
    assert!(!parse_failed_jobs(&json!({"jobs":[]})).unwrap().1);
}
