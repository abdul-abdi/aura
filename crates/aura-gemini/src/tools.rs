//! Gemini tool declarations mapping to macOS actions.

use crate::protocol::{FunctionDeclaration, Tool};
use aura_bridge::actions::Action;
use serde_json::json;

/// Build the tool declarations sent to Gemini in the setup message.
///
/// Returns a `Vec<Tool>` with a single `Tool` containing five
/// `FunctionDeclaration`s that map to macOS desktop actions.
pub fn build_tool_declarations() -> Vec<Tool> {
    vec![Tool {
        function_declarations: vec![
            FunctionDeclaration {
                name: "open_app".into(),
                description: "Open an application by name on macOS".into(),
                parameters: json!({
                    "type": "object",
                    "properties": {
                        "app_name": {
                            "type": "string",
                            "description": "Name of the application to open (e.g. Safari, Terminal, Finder)"
                        }
                    },
                    "required": ["app_name"]
                }),
            },
            FunctionDeclaration {
                name: "search_files".into(),
                description: "Search for files on the user's computer using Spotlight".into(),
                parameters: json!({
                    "type": "object",
                    "properties": {
                        "query": {
                            "type": "string",
                            "description": "The search query to find files"
                        }
                    },
                    "required": ["query"]
                }),
            },
            FunctionDeclaration {
                name: "tile_windows".into(),
                description: "Arrange windows in a tiling layout on screen".into(),
                parameters: json!({
                    "type": "object",
                    "properties": {
                        "layout": {
                            "type": "string",
                            "enum": ["left-right", "grid", "stack"],
                            "description": "The tiling layout to apply"
                        }
                    },
                    "required": ["layout"]
                }),
            },
            FunctionDeclaration {
                name: "launch_url".into(),
                description: "Open a URL in the default web browser".into(),
                parameters: json!({
                    "type": "object",
                    "properties": {
                        "url": {
                            "type": "string",
                            "description": "The URL to open (must start with http:// or https://)"
                        }
                    },
                    "required": ["url"]
                }),
            },
            FunctionDeclaration {
                name: "summarize_screen".into(),
                description: "Capture and describe what is currently visible on the user's screen"
                    .into(),
                parameters: json!({
                    "type": "object",
                    "properties": {},
                    "required": []
                }),
            },
        ],
    }]
}

/// Map a Gemini function call name and arguments to an [`Action`].
///
/// Returns `None` for `summarize_screen` (handled specially by the caller)
/// and for any unknown function name or missing required arguments.
pub fn function_call_to_action(name: &str, args: &serde_json::Value) -> Option<Action> {
    match name {
        "open_app" => Some(Action::OpenApp {
            name: args["app_name"].as_str()?.to_string(),
        }),
        "search_files" => Some(Action::SearchFiles {
            query: args["query"].as_str()?.to_string(),
        }),
        "tile_windows" => Some(Action::TileWindows {
            layout: args["layout"].as_str()?.to_string(),
        }),
        "launch_url" => Some(Action::LaunchUrl {
            url: args["url"].as_str()?.to_string(),
        }),
        // summarize_screen is handled specially by the caller (requires screen capture)
        "summarize_screen" => None,
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn tool_declarations_are_valid_json() {
        let tools = build_tool_declarations();
        let value = serde_json::to_value(&tools).unwrap();

        let names: Vec<&str> = value[0]["functionDeclarations"]
            .as_array()
            .unwrap()
            .iter()
            .map(|fd| fd["name"].as_str().unwrap())
            .collect();

        assert_eq!(
            names,
            vec![
                "open_app",
                "search_files",
                "tile_windows",
                "launch_url",
                "summarize_screen"
            ]
        );
    }

    #[test]
    fn map_open_app() {
        let action = function_call_to_action("open_app", &json!({"app_name": "Safari"}));
        assert!(matches!(action, Some(Action::OpenApp { name }) if name == "Safari"));
    }

    #[test]
    fn map_search_files() {
        let action = function_call_to_action("search_files", &json!({"query": "test"}));
        assert!(matches!(action, Some(Action::SearchFiles { query }) if query == "test"));
    }

    #[test]
    fn map_launch_url() {
        let action = function_call_to_action("launch_url", &json!({"url": "https://example.com"}));
        assert!(matches!(action, Some(Action::LaunchUrl { url }) if url == "https://example.com"));
    }

    #[test]
    fn map_unknown_function() {
        let action = function_call_to_action("unknown", &json!({}));
        assert!(action.is_none());
    }

    #[test]
    fn map_missing_args() {
        let action = function_call_to_action("open_app", &json!({}));
        assert!(action.is_none());
    }
}
