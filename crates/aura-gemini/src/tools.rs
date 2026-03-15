//! Gemini tool declarations for dynamic macOS automation.

use crate::protocol::{FunctionDeclaration, GoogleSearch, Tool};
use serde_json::json;

/// Build the tool declarations sent to Gemini in the setup message.
///
/// Returns a `Vec<Tool>` with:
/// - 20 function declarations for macOS automation and computer control
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
                        For complex workflows, a single well-written script is better than multiple calls. \
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
                            },
                            "verify": {
                                "type": "boolean",
                                "description": "Whether to verify screen changed after execution. Default true. Set false for read-only queries that don't modify the screen."
                            }
                        },
                        "required": ["script"]
                    }),
                    behavior: Some("NON_BLOCKING".into()),
                },
                FunctionDeclaration {
                    name: "get_screen_context".into(),
                    description: "Get the user's current screen context: frontmost application, \
                        window title, list of open windows, clipboard contents, and interactive \
                        UI elements with their accessibility labels and precise bounds. \
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
                    description: "Click at the specified screen coordinates. Always include a target \
                        description of what you're clicking so the targeting system can visually locate \
                        the exact element. Defaults to single left click. \
                        Invoke this tool only after you have identified the target coordinates \
                        from screen context or user instruction."
                        .into(),
                    parameters: json!({
                        "type": "object",
                        "properties": {
                            "x": { "type": "number", "description": "X coordinate" },
                            "y": { "type": "number", "description": "Y coordinate" },
                            "target": { "type": "string", "description": "Short UNIQUE description of the UI element you're clicking (e.g. 'blue Submit button at bottom of form', 'Safari address bar'). Include label text, color, or position. Used by the vision targeting system." },
                            "button": { "type": "string", "enum": ["left", "right"], "description": "Mouse button. Default: left" },
                            "click_count": { "type": "integer", "description": "Number of clicks (2 for double-click). Default: 1" },
                            "modifiers": {
                                "type": "array",
                                "items": { "type": "string", "enum": ["cmd", "shift", "alt", "ctrl"] },
                                "description": "Modifier keys to hold during click. Use for Cmd+click (multi-select, new tab), Shift+click (range select)."
                            },
                            "expected_bounds": {
                                "type": "array",
                                "items": { "type": "integer" },
                                "description": "Optional bounding box [y0, x0, y1, x1] (normalized 0-1000) of the expected target element. If provided, the system validates your click coordinates fall within this region and warns if they don't."
                            }
                        },
                        "required": ["x", "y", "target"]
                    }),
                    behavior: Some("NON_BLOCKING".into()),
                },
                FunctionDeclaration {
                    name: "type_text".into(),
                    description: "Type text into a UI element. If label/role are provided, targets \
                        that specific element directly via the accessibility API (most reliable). \
                        Otherwise types at the currently focused element. \
                        Invoke this tool only after you have confirmed a text field is focused \
                        or have identified the target element from screen context."
                        .into(),
                    parameters: json!({
                        "type": "object",
                        "properties": {
                            "text": { "type": "string", "description": "The text to type" },
                            "label": { "type": "string", "description": "Accessibility label of the target element. If provided, types directly into this element." },
                            "role": { "type": "string", "description": "Element role (e.g. textfield, textarea, combobox). Narrows the search when combined with label." }
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
                        windows, selecting text, dragging files, etc. Interpolates intermediate \
                        points for reliable drag operations. \
                        Invoke this tool only after you have identified the start and end \
                        coordinates from screen context."
                        .into(),
                    parameters: json!({
                        "type": "object",
                        "properties": {
                            "from_x": { "type": "number", "description": "Start X coordinate" },
                            "from_y": { "type": "number", "description": "Start Y coordinate" },
                            "to_x": { "type": "number", "description": "End X coordinate" },
                            "to_y": { "type": "number", "description": "End Y coordinate" },
                            "modifiers": {
                                "type": "array",
                                "items": { "type": "string", "enum": ["cmd", "shift", "alt", "ctrl"] },
                                "description": "Modifier keys to hold during drag."
                            }
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
                FunctionDeclaration {
                    name: "click_element".into(),
                    description:
                        "Click a UI element by its accessibility label and/or role. More reliable than \
                        clicking by coordinates — finds the element in the app's accessibility tree and \
                        clicks its exact center. Use for buttons, text fields, checkboxes, links, tabs, \
                        and other labeled UI elements. \
                        Invoke this tool only after you have identified the element you want to click \
                        from screen context or the user's instruction."
                            .into(),
                    parameters: json!({
                        "type": "object",
                        "properties": {
                            "label": {
                                "type": "string",
                                "description": "Text label to match (case-insensitive substring). Matches against the element's title, description, or value."
                            },
                            "role": {
                                "type": "string",
                                "description": "Element type to match: button, textfield, checkbox, link, tab, menuitem, popupbutton, slider, combobox"
                            },
                            "index": {
                                "type": "integer",
                                "description": "If multiple elements match, click the Nth one (0-indexed). Default: 0"
                            }
                        }
                    }),
                    behavior: Some("NON_BLOCKING".into()),
                },
                FunctionDeclaration {
                    name: "activate_app".into(),
                    description:
                        "Launch an application or bring it to the front. More reliable than clicking \
                        the Dock or using Spotlight. \
                        Invoke this tool only after the user asks to open, switch to, or launch an app."
                            .into(),
                    parameters: json!({
                        "type": "object",
                        "properties": {
                            "name": {
                                "type": "string",
                                "description": "Application name, e.g. 'Safari', 'Terminal', 'Slack', 'Visual Studio Code'"
                            }
                        },
                        "required": ["name"]
                    }),
                    behavior: Some("NON_BLOCKING".into()),
                },
                FunctionDeclaration {
                    name: "click_menu_item".into(),
                    description:
                        "Click a menu bar item by path. More reliable than clicking menus by coordinates \
                        (menus dismiss on mis-click). Supports nested submenus up to 3 levels. \
                        Invoke this tool only after you know the exact menu path needed."
                            .into(),
                    parameters: json!({
                        "type": "object",
                        "properties": {
                            "menu_path": {
                                "type": "array",
                                "items": { "type": "string" },
                                "description": "Menu item path from menu bar, e.g. [\"File\", \"Save As...\"] or [\"View\", \"Developer\", \"JavaScript Console\"]"
                            },
                            "app": {
                                "type": "string",
                                "description": "Target app name. Defaults to the frontmost app if omitted."
                            }
                        },
                        "required": ["menu_path"]
                    }),
                    behavior: Some("NON_BLOCKING".into()),
                },
                FunctionDeclaration {
                    name: "write_clipboard".into(),
                    description: "Write text to the system clipboard. Use with Cmd+V to paste. \
                        Useful for large text blocks or special characters that are hard to type. \
                        Invoke this tool only after you have the text to place on the clipboard."
                        .into(),
                    parameters: json!({
                        "type": "object",
                        "properties": {
                            "text": { "type": "string", "description": "Text to place on the clipboard" }
                        },
                        "required": ["text"]
                    }),
                    behavior: Some("NON_BLOCKING".into()),
                },
                FunctionDeclaration {
                    name: "save_memory".into(),
                    description: "Save a fact to persistent memory for recall in future sessions. \
                        Use for user preferences, learned workflows, and app-specific knowledge. \
                        Don't save transient observations. \
                        Invoke this tool only after you have identified something worth remembering."
                        .into(),
                    parameters: json!({
                        "type": "object",
                        "properties": {
                            "category": {
                                "type": "string",
                                "enum": ["preference", "habit", "entity", "task", "context"],
                                "description": "Category of the fact"
                            },
                            "content": {
                                "type": "string",
                                "description": "The fact to remember"
                            }
                        },
                        "required": ["category", "content"]
                    }),
                    behavior: Some("NON_BLOCKING".into()),
                },
                FunctionDeclaration {
                    name: "key_state".into(),
                    description: "Hold or release a key. Use before drag to hold Shift/Option \
                        during drag. Always release keys after use. \
                        Invoke this tool only after you know which key to hold or release."
                        .into(),
                    parameters: json!({
                        "type": "object",
                        "properties": {
                            "key": { "type": "string", "description": "Key name (e.g., 'shift', 'a', 'cmd')" },
                            "action": { "type": "string", "enum": ["down", "up"], "description": "'down' to hold, 'up' to release" },
                            "modifiers": {
                                "type": "array",
                                "items": { "type": "string", "enum": ["cmd", "shift", "alt", "ctrl"] },
                                "description": "Additional modifier keys"
                            }
                        },
                        "required": ["key", "action"]
                    }),
                    behavior: Some("NON_BLOCKING".into()),
                },
                FunctionDeclaration {
                    name: "context_menu_click".into(),
                    description: "Right-click at coordinates and click a menu item by label. \
                        Atomic — no timing gap. Use instead of separate right-click + click \
                        for context menus. \
                        Invoke this tool only after you know the coordinates and menu item label."
                        .into(),
                    parameters: json!({
                        "type": "object",
                        "properties": {
                            "x": { "type": "number", "description": "X pixel coordinate for right-click" },
                            "y": { "type": "number", "description": "Y pixel coordinate for right-click" },
                            "item_label": { "type": "string", "description": "Label of the menu item to click (case-insensitive substring match)" }
                        },
                        "required": ["x", "y", "item_label"]
                    }),
                    behavior: Some("NON_BLOCKING".into()),
                },
                FunctionDeclaration {
                    name: "run_javascript".into(),
                    description: "Execute JavaScript in Safari or Chrome's active tab. Returns the \
                        result of the last expression. Use for DOM queries, form filling, clicking \
                        web elements, reading page content, and any web interaction where coordinates \
                        are unreliable. \
                        Invoke this tool only after you have confirmed the user wants to interact \
                        with a web page and the target browser is open."
                        .into(),
                    parameters: json!({
                        "type": "object",
                        "properties": {
                            "app": {
                                "type": "string",
                                "enum": ["Safari", "Chrome"],
                                "description": "Target browser"
                            },
                            "code": {
                                "type": "string",
                                "description": "JavaScript code to execute in the active tab"
                            },
                            "timeout_secs": {
                                "type": "integer",
                                "description": "Max execution time in seconds. Default: 30"
                            },
                            "verify": {
                                "type": "boolean",
                                "description": "Whether to verify screen changed. Default: false (most JS is read-only)"
                            }
                        },
                        "required": ["app", "code"]
                    }),
                    behavior: Some("NON_BLOCKING".into()),
                },
                FunctionDeclaration {
                    name: "select_text".into(),
                    description: "Select text using the appropriate keyboard/mouse method. Use before \
                        copy operations. 'all' selects everything (Cmd+A), 'word' double-clicks at \
                        coordinates, 'line' triple-clicks at coordinates, 'to_start' selects from \
                        cursor to document start, 'to_end' selects from cursor to document end. \
                        Invoke this tool only after you know what text needs to be selected."
                        .into(),
                    parameters: json!({
                        "type": "object",
                        "properties": {
                            "method": {
                                "type": "string",
                                "enum": ["all", "word", "line", "to_start", "to_end"],
                                "description": "Selection strategy"
                            },
                            "x": {
                                "type": "number",
                                "description": "X coordinate for word/line selection (image pixels)"
                            },
                            "y": {
                                "type": "number",
                                "description": "Y coordinate for word/line selection (image pixels)"
                            }
                        },
                        "required": ["method"]
                    }),
                    behavior: Some("NON_BLOCKING".into()),
                },
                FunctionDeclaration {
                    name: "run_shell_command".into(),
                    description: "Execute an allowlisted shell command for system configuration. \
                        Allowed commands: defaults (read/write macOS preferences), open (open files/URLs/apps), \
                        killall (terminate apps), say (text-to-speech), launchctl (manage services). \
                        Use defaults write + killall to apply system preference changes. \
                        Invoke this tool only after you have confirmed the user wants to change \
                        a system setting or perform an operation that requires shell access."
                        .into(),
                    parameters: json!({
                        "type": "object",
                        "properties": {
                            "command": {
                                "type": "string",
                                "enum": ["defaults", "open", "killall", "say", "launchctl"],
                                "description": "The command to run (must be in allowlist)"
                            },
                            "args": {
                                "type": "array",
                                "items": { "type": "string" },
                                "description": "Command arguments as separate strings"
                            },
                            "timeout_secs": {
                                "type": "integer",
                                "description": "Max execution time in seconds. Default: 15"
                            },
                            "verify": {
                                "type": "boolean",
                                "description": "Whether to verify screen changed. Default: false"
                            }
                        },
                        "required": ["command", "args"]
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
        assert_eq!(decls.len(), 20, "Should have 20 function declarations");
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
                "click_element",
                "activate_app",
                "click_menu_item",
                "write_clipboard",
                "save_memory",
                "key_state",
                "context_menu_click",
                "run_javascript",
                "select_text",
                "run_shell_command",
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
        assert_eq!(decls.len(), 20);
        assert_eq!(decls[0]["name"], "run_applescript");
        assert_eq!(decls[1]["name"], "get_screen_context");
        assert_eq!(decls[2]["name"], "shutdown_aura");
        assert_eq!(decls[3]["name"], "move_mouse");
        assert_eq!(decls[8]["name"], "drag");
        assert_eq!(decls[9]["name"], "recall_memory");
        assert_eq!(decls[13]["name"], "write_clipboard");
        assert_eq!(decls[14]["name"], "save_memory");
        assert_eq!(decls[15]["name"], "key_state");
        assert_eq!(decls[16]["name"], "context_menu_click");
        assert_eq!(decls[17]["name"], "run_javascript");
        assert_eq!(decls[18]["name"], "select_text");
        assert_eq!(decls[19]["name"], "run_shell_command");
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

    #[test]
    fn click_element_has_label_and_role_params() {
        let tools = build_tool_declarations();
        let decls = tools[0].function_declarations.as_ref().unwrap();
        let ce = decls.iter().find(|d| d.name == "click_element").unwrap();
        assert!(ce.parameters["properties"]["label"].is_object());
        assert!(ce.parameters["properties"]["role"].is_object());
        assert!(ce.parameters["properties"]["index"].is_object());
        // No required params — both label and role are optional
        let required = ce.parameters.get("required");
        assert!(
            required.is_none() || required.unwrap().as_array().is_none_or(|a| a.is_empty()),
            "click_element should have no required params"
        );
    }

    #[test]
    fn activate_app_has_required_name() {
        let tools = build_tool_declarations();
        let decls = tools[0].function_declarations.as_ref().unwrap();
        let aa = decls.iter().find(|d| d.name == "activate_app").unwrap();
        let required = aa.parameters["required"].as_array().unwrap();
        assert!(required.iter().any(|v| v == "name"));
    }

    #[test]
    fn click_menu_item_has_required_menu_path() {
        let tools = build_tool_declarations();
        let decls = tools[0].function_declarations.as_ref().unwrap();
        let cmi = decls.iter().find(|d| d.name == "click_menu_item").unwrap();
        let required = cmi.parameters["required"].as_array().unwrap();
        assert!(required.iter().any(|v| v == "menu_path"));
    }

    #[test]
    fn click_tool_has_target_parameter() {
        let tools = build_tool_declarations();
        let decls = tools[0].function_declarations.as_ref().unwrap();
        let click = decls.iter().find(|fd| fd.name == "click").unwrap();
        let props = click.parameters["properties"].as_object().unwrap();
        assert!(props.contains_key("target"), "click tool should have 'target' parameter");
    }
}
