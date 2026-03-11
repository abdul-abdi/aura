//! Gemini tool declarations for dynamic macOS automation.

use crate::protocol::{FunctionDeclaration, GoogleSearch, Tool};
use serde_json::json;

/// Build the tool declarations sent to Gemini in the setup message.
///
/// Returns a `Vec<Tool>` with:
/// - 10 function declarations for macOS automation and computer control
/// - Google Search grounding (current events, weather, facts, etc.)
pub fn build_tool_declarations() -> Vec<Tool> {
    vec![
        Tool {
            function_declarations: Some(vec![
                FunctionDeclaration {
                    name: "run_applescript".into(),
                    description:
                        "Execute AppleScript or JXA code to control any macOS application \
                        or system feature. You can open apps, manage windows, interact with UI \
                        elements, automate workflows, manipulate files, control system settings, \
                        send keystrokes, and more. Write the script based on what the user needs. \
                        Prefer simple scripts — chain multiple calls over one complex script. \
                        Invoke this tool only after you have confirmed the user's intent and \
                        understand what action to take."
                            .into(),
                    parameters: json!({
                        "type": "object",
                        "properties": {
                            "script": {
                                "type": "string",
                                "description": "The AppleScript or JXA code to execute"
                            },
                            "language": {
                                "type": "string",
                                "enum": ["applescript", "javascript"],
                                "description": "Script language. Default: applescript"
                            },
                            "timeout_secs": {
                                "type": "integer",
                                "description": "Max execution time in seconds. Default: 30"
                            }
                        },
                        "required": ["script"]
                    }),
                    behavior: Some("NON_BLOCKING".into()),
                },
                FunctionDeclaration {
                    name: "get_screen_context".into(),
                    description: "Get the user's current screen context: frontmost application, \
                        window title, list of open windows, and clipboard contents. \
                        Invoke this tool only after the user asks you to interact with \
                        something on screen or when you need to understand their current context."
                        .into(),
                    parameters: json!({
                        "type": "object",
                        "properties": {}
                    }),
                    behavior: Some("NON_BLOCKING".into()),
                },
                FunctionDeclaration {
                    name: "shutdown_aura".into(),
                    description: "Shut down and quit Aura completely. Say goodbye \
                        before calling this tool. \
                        Invoke this tool only after the user explicitly asks to exit, quit, \
                        shut down, close, or stop Aura."
                        .into(),
                    parameters: json!({
                        "type": "object",
                        "properties": {}
                    }),
                    behavior: None,
                },
                FunctionDeclaration {
                    name: "move_mouse".into(),
                    description: "Move the mouse cursor to the specified screen coordinates. \
                        Invoke this tool only after you have identified the target coordinates \
                        from screen context or user instruction."
                        .into(),
                    parameters: json!({
                        "type": "object",
                        "properties": {
                            "x": { "type": "number", "description": "X coordinate (pixels from left)" },
                            "y": { "type": "number", "description": "Y coordinate (pixels from top)" }
                        },
                        "required": ["x", "y"]
                    }),
                    behavior: Some("NON_BLOCKING".into()),
                },
                FunctionDeclaration {
                    name: "click".into(),
                    description: "Click at the specified screen coordinates. Defaults to single \
                        left click. \
                        Invoke this tool only after you have identified the target coordinates \
                        from screen context or user instruction."
                        .into(),
                    parameters: json!({
                        "type": "object",
                        "properties": {
                            "x": { "type": "number", "description": "X coordinate" },
                            "y": { "type": "number", "description": "Y coordinate" },
                            "button": { "type": "string", "enum": ["left", "right"], "description": "Mouse button. Default: left" },
                            "click_count": { "type": "integer", "description": "Number of clicks (2 for double-click). Default: 1" }
                        },
                        "required": ["x", "y"]
                    }),
                    behavior: Some("NON_BLOCKING".into()),
                },
                FunctionDeclaration {
                    name: "type_text".into(),
                    description: "Type a string of text at the current cursor position. Use for \
                        entering text in fields, search bars, editors, etc. \
                        Invoke this tool only after you have confirmed a text field is focused \
                        or have clicked into the target field."
                        .into(),
                    parameters: json!({
                        "type": "object",
                        "properties": {
                            "text": { "type": "string", "description": "The text to type" }
                        },
                        "required": ["text"]
                    }),
                    behavior: Some("NON_BLOCKING".into()),
                },
                FunctionDeclaration {
                    name: "press_key".into(),
                    description: "Press a key with optional modifiers. Use for keyboard shortcuts \
                        (Cmd+C, Cmd+V, Cmd+Tab, etc.) and special keys (Return, Escape, Tab, \
                        arrow keys, F1-F12). \
                        Invoke this tool only after you know which key combination is needed \
                        for the user's request."
                        .into(),
                    parameters: json!({
                        "type": "object",
                        "properties": {
                            "key": { "type": "string", "description": "Key name: a-z, return, escape, tab, space, delete, up, down, left, right, f1-f12" },
                            "modifiers": {
                                "type": "array",
                                "items": { "type": "string", "enum": ["cmd", "shift", "alt", "ctrl"] },
                                "description": "Modifier keys to hold. Example: ['cmd', 'shift']"
                            }
                        },
                        "required": ["key"]
                    }),
                    behavior: Some("NON_BLOCKING".into()),
                },
                FunctionDeclaration {
                    name: "scroll".into(),
                    description: "Scroll the view. Positive dy scrolls down, negative dy scrolls \
                        up. Positive dx scrolls right, negative dx scrolls left. \
                        Invoke this tool only after you know the scroll direction and amount \
                        needed."
                        .into(),
                    parameters: json!({
                        "type": "object",
                        "properties": {
                            "dx": { "type": "integer", "description": "Horizontal scroll amount in pixels. Default: 0" },
                            "dy": { "type": "integer", "description": "Vertical scroll amount in pixels. Positive = down." }
                        },
                        "required": ["dy"]
                    }),
                    behavior: Some("NON_BLOCKING".into()),
                },
                FunctionDeclaration {
                    name: "drag".into(),
                    description: "Click and drag from one point to another. Used for moving \
                        windows, selecting text, dragging files, etc. \
                        Invoke this tool only after you have identified the start and end \
                        coordinates from screen context."
                        .into(),
                    parameters: json!({
                        "type": "object",
                        "properties": {
                            "from_x": { "type": "number", "description": "Start X coordinate" },
                            "from_y": { "type": "number", "description": "Start Y coordinate" },
                            "to_x": { "type": "number", "description": "End X coordinate" },
                            "to_y": { "type": "number", "description": "End Y coordinate" }
                        },
                        "required": ["from_x", "from_y", "to_x", "to_y"]
                    }),
                    behavior: Some("NON_BLOCKING".into()),
                },
                FunctionDeclaration {
                    name: "recall_memory".into(),
                    description: "Search Aura's memory for information from past sessions. \
                        Returns matching facts and session summaries ranked by relevance. \
                        If no results are found, tell the user you don't have that in memory. \
                        Invoke this tool only after the user asks about something from a \
                        previous session, references past context, or when historical \
                        information would help the current task."
                        .into(),
                    parameters: json!({
                        "type": "object",
                        "properties": {
                            "query": {
                                "type": "string",
                                "description": "Natural language search query. Example: 'dark mode preference', 'report.pdf', 'what was the user working on yesterday'"
                            }
                        },
                        "required": ["query"]
                    }),
                    behavior: Some("NON_BLOCKING".into()),
                },
            ]),
            google_search: None,
            code_execution: None,
        },
        // Google Search grounding — lets Gemini answer current events, weather, etc.
        Tool {
            function_declarations: None,
            google_search: Some(GoogleSearch {}),
            code_execution: None,
        },
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tool_declarations_returns_two_tool_objects() {
        let tools = build_tool_declarations();
        assert_eq!(tools.len(), 2, "Function declarations + Google Search");
        let decls = tools[0].function_declarations.as_ref().unwrap();
        assert_eq!(decls.len(), 10, "Should have 10 function declarations");
    }

    #[test]
    fn tool_names_are_correct() {
        let tools = build_tool_declarations();
        let decls = tools[0].function_declarations.as_ref().unwrap();
        let names: Vec<&str> = decls.iter().map(|fd| fd.name.as_str()).collect();
        assert_eq!(
            names,
            vec![
                "run_applescript",
                "get_screen_context",
                "shutdown_aura",
                "move_mouse",
                "click",
                "type_text",
                "press_key",
                "scroll",
                "drag",
                "recall_memory",
            ]
        );
    }

    #[test]
    fn google_search_present() {
        let tools = build_tool_declarations();
        assert!(
            tools[1].google_search.is_some(),
            "Second tool should be Google Search"
        );
    }

    #[test]
    fn tool_declarations_serialize_to_valid_json() {
        let tools = build_tool_declarations();
        let value = serde_json::to_value(&tools).unwrap();
        let decls = value[0]["functionDeclarations"].as_array().unwrap();
        assert_eq!(decls.len(), 10);
        assert_eq!(decls[0]["name"], "run_applescript");
        assert_eq!(decls[1]["name"], "get_screen_context");
        assert_eq!(decls[2]["name"], "shutdown_aura");
        assert_eq!(decls[3]["name"], "move_mouse");
        assert_eq!(decls[8]["name"], "drag");
        assert_eq!(decls[9]["name"], "recall_memory");
        // Google Search
        assert!(value[1]["googleSearch"].is_object());
    }

    #[test]
    fn run_applescript_has_required_script_param() {
        let tools = build_tool_declarations();
        let decls = tools[0].function_declarations.as_ref().unwrap();
        let params = &decls[0].parameters;
        assert!(params["properties"]["script"].is_object());
        let required = params["required"].as_array().unwrap();
        assert!(required.iter().any(|v| v == "script"));
    }

    #[test]
    fn non_blocking_behavior_set_on_async_tools() {
        let tools = build_tool_declarations();
        let decls = tools[0].function_declarations.as_ref().unwrap();
        for decl in decls {
            if decl.name == "shutdown_aura" {
                assert!(
                    decl.behavior.is_none(),
                    "shutdown_aura should have no behavior"
                );
            } else {
                assert_eq!(
                    decl.behavior.as_deref(),
                    Some("NON_BLOCKING"),
                    "{} should be NON_BLOCKING",
                    decl.name
                );
            }
        }
    }

    #[test]
    fn descriptions_have_invocation_conditions() {
        let tools = build_tool_declarations();
        let decls = tools[0].function_declarations.as_ref().unwrap();
        for decl in decls {
            assert!(
                decl.description.contains("Invoke this tool only after"),
                "{} description missing invocation condition",
                decl.name
            );
        }
    }
}
