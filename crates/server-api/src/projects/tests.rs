use super::*;

#[test]
fn business_errors_map_to_safe_wire_codes() {
    for (cause, expected) in [
        (ProjectError::Invalid, ErrorCode::InvalidMessage),
        (
            ProjectError::UnsupportedWorkspace,
            ErrorCode::UnsupportedWorkspace,
        ),
        (ProjectError::LegacyProject, ErrorCode::LegacyProject),
        (ProjectError::Busy, ErrorCode::ProjectBusy),
        (
            ProjectError::UnsupportedFormat,
            ErrorCode::UnsupportedFormat,
        ),
        (
            ProjectError::IdempotencyConflict,
            ErrorCode::IdempotencyConflict,
        ),
        (ProjectError::IdentityConflict, ErrorCode::IdentityConflict),
        (ProjectError::NotFound, ErrorCode::ProjectNotFound),
        (ProjectError::NotOpen, ErrorCode::ProjectNotOpen),
        (ProjectError::StaleOwner, ErrorCode::StaleOwner),
        (ProjectError::Io, ErrorCode::ProjectIo),
    ] {
        assert_eq!(error(cause), expected);
    }
}
