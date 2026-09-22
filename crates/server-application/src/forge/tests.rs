use std::sync::{Arc, Mutex};

use super::*;

#[derive(Debug)]
struct FakeForge {
    calls: Arc<Mutex<Vec<String>>>,
}

impl ForgeRuntime for FakeForge {
    fn search(
        &self,
        _cwd: &str,
        _query: &str,
        _limit: usize,
        _kinds: &[ForgeSearchKind],
    ) -> Result<ForgeSearch, ForgeRuntimeError> {
        self.calls.lock().unwrap().push("search".to_owned());
        Ok(ForgeSearch {
            items: Vec::new(),
            auth_state: ForgeAuthState::Authenticated,
        })
    }

    fn create_pull_request(
        &self,
        _cwd: &str,
        title: &str,
        body: &str,
        _base_ref: Option<&str>,
    ) -> Result<PullRequestCreated, ForgeRuntimeError> {
        self.calls
            .lock()
            .unwrap()
            .push(format!("create:{title}:{body}"));
        Ok(PullRequestCreated {
            url: "https://example/pull/1".to_owned(),
            number: 1,
        })
    }

    fn current_pull_request_status(
        &self,
        _cwd: &str,
    ) -> Result<PullRequestStatusRead, ForgeRuntimeError> {
        unimplemented!()
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
        self.calls.lock().unwrap().push("auto".to_owned());
        Ok(())
    }

    fn pull_request_timeline(
        &self,
        _cwd: &str,
        _pr_number: u64,
        _repo_owner: &str,
        _repo_name: &str,
    ) -> Result<PullRequestTimeline, ForgeRuntimeError> {
        unimplemented!()
    }

    fn check_details(
        &self,
        _cwd: &str,
        _repo_owner: Option<&str>,
        _repo_name: Option<&str>,
        _check_run_id: Option<u64>,
        _workflow_run_id: Option<u64>,
        _change_request_number: Option<u64>,
    ) -> Result<CheckDetails, ForgeRuntimeError> {
        unimplemented!()
    }
}

fn service(calls: Arc<Mutex<Vec<String>>>) -> Forge {
    Forge::new(Box::new(FakeForge { calls }))
}

#[test]
fn trims_explicit_pull_request_metadata_and_delegates() {
    let calls = Arc::new(Mutex::new(Vec::new()));
    let result = service(calls.clone())
        .create_pull_request("/repo", "  title  ", " body ", None)
        .unwrap();
    assert_eq!(result.number, 1);
    assert_eq!(calls.lock().unwrap().as_slice(), ["create:title:body"]);
}

#[test]
fn rejects_metadata_and_auto_merge_shape_before_the_adapter() {
    let calls = Arc::new(Mutex::new(Vec::new()));
    let service = service(calls.clone());
    assert_eq!(
        service
            .create_pull_request("/repo", "", "body", None)
            .unwrap_err()
            .kind,
        ForgeFailureKind::Invalid
    );
    assert_eq!(
        service
            .set_current_pull_request_auto_merge("/repo", true, None)
            .unwrap_err()
            .message,
        "mergeMethod is required when enabling auto-merge"
    );
    assert_eq!(
        service
            .set_current_pull_request_auto_merge(
                "/repo",
                false,
                Some(PullRequestMergeMethod::Squash),
            )
            .unwrap_err()
            .message,
        "mergeMethod is not allowed when disabling auto-merge"
    );
    assert!(calls.lock().unwrap().is_empty());
}

#[test]
fn rejects_unaddressed_check_details_before_the_adapter() {
    let calls = Arc::new(Mutex::new(Vec::new()));
    let error = service(calls)
        .check_details("/repo", Some("acme"), Some("app"), None, None, None)
        .unwrap_err();
    assert_eq!(error.kind, ForgeFailureKind::Invalid);
    assert!(error.message.contains("checkRunId or workflowRunId"));
}
