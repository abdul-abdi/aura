//! Gemini tool declarations for dynamic macOS automation.

use crate::protocol::{FunctionDeclaration, Tool};
use serde_json::json;

/// Build the tool declarations sent to Gemini in the setup message.
///
/// Returns a `Vec<Tool>` with a single `Tool` containing three
/// `FunctionDeclaration`s: `run_applescript`, `get_screen_context`, and `shutdown_aura`.
pub fn build_tool_declarations() -> Vec<Tool> {
    vec![Tool {
        function_declarations: vec![
            FunctionDeclaration {
                name: "run_applescript".into(),
                description: "Execute AppleScript or JXA code to control any macOS application \
                    or system feature. You can open apps, manage windows, interact with UI \
                    elements, automate workflows, manipulate files, control system settings, \
                    send keystrokes, and more. Write the script based on what the user needs. \
                    Prefer simple scripts — chain multiple calls over one complex script."
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
            },
            FunctionDeclaration {
                name: "get_screen_context".into(),
                description: "Get the user's current screen context: frontmost application, \
                    window title, list of open windows, and clipboard contents. Always call \
                    this before taking action so you understand what the user is doing."
                    .into(),
                parameters: json!({
                    "type": "object",
                    "properties": {}
                }),
            },
            FunctionDeclaration {
                name: "shutdown_aura".into(),
                description: "Shut down and quit Aura completely. Call this when the user \
                    says they want to exit, quit, shut down, close, or stop Aura. Say goodbye \
                    before calling this tool."
                    .into(),
                parameters: json!({
                    "type": "object",
                    "properties": {}
                }),
            },
            FunctionDeclaration {
                name: "move_mouse".into(),
                description: "Move the mouse cursor to the specified screen coordinates."
                    .into(),
                parameters: json!({
                    "type": "object",
                    "properties": {
                        "x": { "type": "number", "description": "X coordinate (pixels from left)" },
                        "y": { "type": "number", "description": "Y coordinate (pixels from top)" }
                    },
                    "required": ["x", "y"]
                }),
            },
            FunctionDeclaration {
                name: "click".into(),
                description: "Click at the specified screen coordinates. Defaults to single \
                    left click."
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
            },
            FunctionDeclaration {
                name: "type_text".into(),
                description: "Type a string of text at the current cursor position. Use for \
                    entering text in fields, search bars, editors, etc."
                    .into(),
                parameters: json!({
                    "type": "object",
                    "properties": {
                        "text": { "type": "string", "description": "The text to type" }
                    },
                    "required": ["text"]
                }),
            },
            FunctionDeclaration {
                name: "press_key".into(),
                description: "Press a key with optional modifiers. Use for keyboard shortcuts \
                    (Cmd+C, Cmd+V, Cmd+Tab, etc.) and special keys (Return, Escape, Tab, \
                    arrow keys, F1-F12)."
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
            },
            FunctionDeclaration {
                name: "scroll".into(),
                description: "Scroll the view. Positive dy scrolls down, negative dy scrolls \
                    up. Positive dx scrolls right, negative dx scrolls left."
                    .into(),
                parameters: json!({
                    "type": "object",
                    "properties": {
                        "dx": { "type": "integer", "description": "Horizontal scroll amount in pixels. Default: 0" },
                        "dy": { "type": "integer", "description": "Vertical scroll amount in pixels. Positive = down." }
                    },
                    "required": ["dy"]
                }),
            },
            FunctionDeclaration {
                name: "drag".into(),
                description: "Click and drag from one point to another. Used for moving \
                    windows, selecting text, dragging files, etc."
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
            },
        ],
    }]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tool_declarations_returns_nine_functions() {
        let tools = build_tool_declarations();
        assert_eq!(tools.len(), 1, "Should be one Tool object");
        assert_eq!(
            tools[0].function_declarations.len(),
            9,
            "Should have 9 function declarations"
        );
    }

    #[test]
    fn tool_names_are_correct() {
        let tools = build_tool_declarations();
        let names: Vec<&str> = tools[0]
            .function_declarations
            .iter()
            .map(|fd| fd.name.as_str())
            .collect();
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
            ]
        );
    }

    #[test]
    fn tool_declarations_serialize_to_valid_json() {
        let tools = build_tool_declarations();
        let value = serde_json::to_value(&tools).unwrap();
        let decls = value[0]["functionDeclarations"].as_array().unwrap();
        assert_eq!(decls.len(), 9);
        assert_eq!(decls[0]["name"], "run_applescript");
        assert_eq!(decls[1]["name"], "get_screen_context");
        assert_eq!(decls[2]["name"], "shutdown_aura");
        assert_eq!(decls[3]["name"], "move_mouse");
        assert_eq!(decls[8]["name"], "drag");
    }

    #[test]
    fn run_applescript_has_required_script_param() {
        let tools = build_tool_declarations();
        let params = &tools[0].function_declarations[0].parameters;
        assert!(params["properties"]["script"].is_object());
        let required = params["required"].as_array().unwrap();
        assert!(required.iter().any(|v| v == "script"));
    }
}
