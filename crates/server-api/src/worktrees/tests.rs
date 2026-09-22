use super::*;

#[test]
fn application_failure_classes_map_to_paseo_codes() {
    assert_eq!(
        checkout_error_code(WorktreeFailureKind::NotGitRepository),
        CheckoutErrorCode::NotGitRepo
    );
    assert_eq!(
        checkout_error_code(WorktreeFailureKind::NotAllowed),
        CheckoutErrorCode::NotAllowed
    );
    assert_eq!(
        checkout_error_code(WorktreeFailureKind::Other),
        CheckoutErrorCode::Unknown
    );
    assert_eq!(
        create_error_code_for_kind(WorktreeFailureKind::BranchAlreadyCheckedOut),
        "branch_already_checked_out"
    );
    assert_eq!(
        create_error_code_for_kind(WorktreeFailureKind::MissingCheckoutTarget),
        "missing_checkout_target"
    );
    assert_eq!(
        create_error_code_for_kind(WorktreeFailureKind::UnknownBranch),
        "unknown_branch"
    );
    assert_eq!(
        create_error_code_for_kind(WorktreeFailureKind::Other),
        "unknown"
    );
}
