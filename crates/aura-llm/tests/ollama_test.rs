use aura_llm::ollama::{OllamaConfig, OllamaProvider};
use aura_llm::provider::LlmProvider;

#[tokio::test]
async fn test_ollama_provider_connection_refused() {
    let config = OllamaConfig {
        base_url: "http://localhost:1".into(), // nothing listening here
        model: "test".into(),
        timeout_secs: 2,
    };
    let provider = OllamaProvider::new(config).unwrap();
    let result = provider.complete("hello").await;
    assert!(result.is_err(), "Should fail when Ollama is not running");
}

#[tokio::test]
async fn test_ollama_health_check_connection_refused() {
    let config = OllamaConfig {
        base_url: "http://localhost:1".into(),
        model: "test".into(),
        timeout_secs: 2,
    };
    let provider = OllamaProvider::new(config).unwrap();
    let result = provider.health_check().await;
    assert!(result.is_err());
}

#[tokio::test]
async fn test_ollama_config_defaults() {
    let config = OllamaConfig::default();
    assert_eq!(config.base_url, "http://localhost:11434");
    assert_eq!(config.model, "qwen3.5:4b");
    assert_eq!(config.timeout_secs, 30);
}
