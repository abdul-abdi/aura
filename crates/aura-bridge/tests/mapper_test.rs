use aura_bridge::mapper::intent_to_action;
use aura_llm::intent::Intent;

#[test]
fn test_open_app_maps() {
    let intent = Intent::OpenApp {
        name: "Safari".into(),
    };
    let action = intent_to_action(&intent).unwrap();
    assert!(
        matches!(action, aura_bridge::actions::Action::OpenApp { name } if name == "Safari")
    );
}

#[test]
fn test_search_files_maps() {
    let intent = Intent::SearchFiles {
        query: "readme".into(),
    };
    let action = intent_to_action(&intent).unwrap();
    assert!(
        matches!(action, aura_bridge::actions::Action::SearchFiles { query } if query == "readme")
    );
}

#[test]
fn test_tile_windows_maps() {
    let intent = Intent::TileWindows {
        layout: "left-right".into(),
    };
    let action = intent_to_action(&intent).unwrap();
    assert!(
        matches!(action, aura_bridge::actions::Action::TileWindows { layout } if layout == "left-right")
    );
}

#[test]
fn test_launch_url_maps() {
    let intent = Intent::LaunchUrl {
        url: "https://example.com".into(),
    };
    let action = intent_to_action(&intent).unwrap();
    assert!(
        matches!(action, aura_bridge::actions::Action::LaunchUrl { url } if url == "https://example.com")
    );
}

#[test]
fn test_summarize_screen_returns_none() {
    let intent = Intent::SummarizeScreen;
    assert!(intent_to_action(&intent).is_none());
}

#[test]
fn test_unknown_returns_none() {
    let intent = Intent::Unknown {
        raw: "gibberish".into(),
    };
    assert!(intent_to_action(&intent).is_none());
}
