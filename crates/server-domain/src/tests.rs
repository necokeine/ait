use super::*;

#[test]
fn agent_and_operation_identities_reject_invalid_values() {
    let agent = AgentId::generate();
    let operation = OperationId::generate();
    assert_eq!(agent.to_string().parse(), Ok(agent));
    assert_eq!(operation.to_string().parse(), Ok(operation));
    for invalid in ["", "not-an-id", "00000000-0000-0000-0000-000000000000"] {
        assert!(invalid.parse::<AgentId>().is_err());
        assert!(invalid.parse::<OperationId>().is_err());
    }
}
