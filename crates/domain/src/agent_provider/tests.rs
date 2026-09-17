use super::ProviderKind;

#[test]
fn gemini_provider_kind_round_trips() {
    let kind: ProviderKind = serde_json::from_str(r#""gemini""#).unwrap();
    assert_eq!(kind, ProviderKind::Gemini);
    assert_eq!(serde_json::to_string(&kind).unwrap(), r#""gemini""#);
}

#[test]
fn minimax_provider_kind_round_trips() {
    let kind: ProviderKind = serde_json::from_str(r#""minimax""#).unwrap();
    assert_eq!(kind, ProviderKind::MiniMax);
    assert_eq!(serde_json::to_string(&kind).unwrap(), r#""minimax""#);
}

#[cfg(not(all(feature = "dev-mock-provider", debug_assertions)))]
#[test]
fn production_contract_cannot_deserialize_mock_provider_kind() {
    assert!(serde_json::from_str::<ProviderKind>(r#""mock""#).is_err());
}

#[cfg(all(feature = "dev-mock-provider", debug_assertions))]
#[test]
fn development_contract_round_trips_mock_provider_kind() {
    let kind: ProviderKind = serde_json::from_str(r#""mock""#).unwrap();
    assert_eq!(kind, ProviderKind::Mock);
    assert_eq!(serde_json::to_string(&kind).unwrap(), r#""mock""#);
}
