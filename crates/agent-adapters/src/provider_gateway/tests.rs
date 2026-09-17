use super::*;
use ait_tools::DEFAULT_SYSTEM_PROMPT;

#[test]
fn gateway_preserves_history_after_default_system_without_advertising_tools() {
    let config = AgentConfiguration {
        model: "fixture-model".into(),
        ..Default::default()
    };
    for provider in [
        LLMProvider::DeepSeek,
        LLMProvider::Gemini,
        LLMProvider::MiniMax,
        LLMProvider::OpenAI,
    ] {
        let client = LLMClient::new(LLMClientConfig::new(provider, "fixture-key")).unwrap();
        let messages = [
            ("system", "Project instructions"),
            ("user", "First"),
            ("assistant", "Answer"),
            ("user", "Current {{literal}}"),
        ]
        .into_iter()
        .map(|(role, text)| ProviderMessage {
            role: role.into(),
            text: text.into(),
        })
        .collect();
        let request = text_request(&client, &config, messages).unwrap();
        assert!(request.tools.is_empty());
        assert_eq!(
            request.chat_history,
            vec![
                Message::system(DEFAULT_SYSTEM_PROMPT),
                Message::system("Project instructions"),
                Message::user("First"),
                Message::assistant("Answer"),
                Message::user("Current {{literal}}"),
            ]
        );
        assert!(text_request(&client, &config, vec![]).is_err());
    }
}
