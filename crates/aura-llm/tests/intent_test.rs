use aura_llm::intent::{Intent, IntentParser};
use aura_llm::provider::MockProvider;

fn parser_with(responses: Vec<(&str, &str)>) -> IntentParser {
    IntentParser::new(Box::new(MockProvider::new(responses)))
}

#[tokio::test]
async fn test_parse_open_app_intent() {
    let parser = parser_with(vec![("open safari", r#"{"type":"open_app","name":"Safari"}"#)]);
    let intent = parser.parse("open safari").await.unwrap();
    assert_eq!(intent, Intent::OpenApp { name: "Safari".into() });
}

#[tokio::test]
async fn test_parse_search_files_intent() {
    let parser = parser_with(vec![("find my resume", r#"{"type":"search_files","query":"resume"}"#)]);
    let intent = parser.parse("find my resume").await.unwrap();
    assert_eq!(intent, Intent::SearchFiles { query: "resume".into() });
}

#[tokio::test]
async fn test_parse_tile_windows_intent() {
    let parser = parser_with(vec![("tile windows", r#"{"type":"tile_windows","layout":"left-right"}"#)]);
    let intent = parser.parse("tile windows").await.unwrap();
    assert_eq!(intent, Intent::TileWindows { layout: "left-right".into() });
}

#[tokio::test]
async fn test_parse_summarize_screen_intent() {
    let parser = parser_with(vec![("what's on my screen", r#"{"type":"summarize_screen"}"#)]);
    let intent = parser.parse("what's on my screen").await.unwrap();
    assert_eq!(intent, Intent::SummarizeScreen);
}

#[tokio::test]
async fn test_parse_launch_url_intent() {
    let parser = parser_with(vec![("open google", r#"{"type":"launch_url","url":"https://google.com"}"#)]);
    let intent = parser.parse("open google").await.unwrap();
    assert_eq!(intent, Intent::LaunchUrl { url: "https://google.com".into() });
}

#[tokio::test]
async fn test_parse_unknown_intent() {
    let parser = parser_with(vec![("flibbertigibbet", r#"{"type":"unknown","raw":"flibbertigibbet"}"#)]);
    let intent = parser.parse("flibbertigibbet").await.unwrap();
    assert_eq!(intent, Intent::Unknown { raw: "flibbertigibbet".into() });
}

#[tokio::test]
async fn test_malformed_json_falls_back_to_unknown() {
    let parser = parser_with(vec![("hello", "this is not json at all")]);
    let intent = parser.parse("hello").await.unwrap();
    assert_eq!(intent, Intent::Unknown { raw: "hello".into() });
}

#[tokio::test]
async fn test_no_match_falls_back_to_unknown() {
    let parser = parser_with(vec![]);
    let intent = parser.parse("something random").await.unwrap();
    // MockProvider returns {"type":"unknown","raw":"no match"} when nothing matches
    assert!(matches!(intent, Intent::Unknown { .. }));
}
