use aura_llm::intent::{Intent, IntentParser};
use aura_llm::provider::MockProvider;

#[tokio::test]
async fn test_parse_open_app_intent() {
    let provider = MockProvider::new(vec![
        (
            "open safari",
            r#"{"type":"open_app","name":"Safari"}"#,
        ),
    ]);
    let parser = IntentParser::new(Box::new(provider));

    let intent = parser.parse("open safari").await.unwrap();
    assert!(matches!(intent, Intent::OpenApp { .. }));
}

#[tokio::test]
async fn test_parse_search_files_intent() {
    let provider = MockProvider::new(vec![
        (
            "find my resume",
            r#"{"type":"search_files","query":"resume"}"#,
        ),
    ]);
    let parser = IntentParser::new(Box::new(provider));

    let intent = parser.parse("find my resume").await.unwrap();
    assert!(matches!(intent, Intent::SearchFiles { .. }));
}

#[tokio::test]
async fn test_parse_unknown_intent() {
    let provider = MockProvider::new(vec![
        (
            "flibbertigibbet",
            r#"{"type":"unknown","raw":"flibbertigibbet"}"#,
        ),
    ]);
    let parser = IntentParser::new(Box::new(provider));

    let intent = parser.parse("flibbertigibbet").await.unwrap();
    assert!(matches!(intent, Intent::Unknown { .. }));
}
