use std::sync::atomic::AtomicBool;
use std::sync::{Arc, Mutex};

use anyhow::{Context, Result};
use clap::Parser;
use tokio::sync::broadcast;
use tokio_util::sync::CancellationToken;

use aura_daemon::bus::EventBus;
use aura_daemon::protocol::DaemonEvent;
use aura_daemon::setup::AuraSetup;
use aura_gemini::config::GeminiConfig;
use aura_memory::SessionMemory;
use aura_menubar::app::MenuBarApp;
use tracing_subscriber::EnvFilter;

mod cloud;
mod deploy;
mod orchestrator;
mod pipeline;
mod processor;
mod screen_capture;
mod tool_helpers;
mod tools;

use orchestrator::SessionMode;

const EVENT_BUS_CAPACITY: usize = 256;
const IPC_BROADCAST_CAPACITY: usize = 256;

/// Save device token to the macOS Keychain via the `security` CLI.
/// Uses `security add-generic-password` with `-U` (update if exists)
/// which writes to the data protection keychain without ACL issues.
fn save_keychain_token(token: &str) -> Result<(), String> {
    let status = std::process::Command::new("security")
        .args([
            "add-generic-password",
            "-s", "com.aura.desktop",
            "-a", "device_token",
            "-w", token,
            "-U", // Update if exists
        ])
        .status()
        .map_err(|e| format!("Failed to run security command: {e}"))?;
    if status.success() {
        Ok(())
    } else {
        Err(format!("security add-generic-password failed with {status}"))
    }
}

#[derive(Parser)]
#[command(name = "aura", about = "Voice-first AI desktop companion")]
struct Cli {
    /// Run without the menu bar UI (headless mode)
    #[arg(long, global = true)]
    headless: bool,

    /// Enable verbose logging
    #[arg(short, long, global = true)]
    verbose: bool,

    #[command(subcommand)]
    command: Option<Command>,
}

#[derive(clap::Subcommand)]
enum Command {
    /// Deploy aura-proxy to Google Cloud Run
    Deploy {
        /// Accept all defaults without prompting (for non-interactive use)
        #[arg(short, long)]
        yes: bool,
    },
}

fn main() -> Result<()> {
    let cli = Cli::parse();

    // Initialize tracing
    let filter = if cli.verbose { "debug" } else { "info" };
    tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new(filter)),
        )
        .init();

    // Handle subcommands
    if let Some(Command::Deploy { yes }) = cli.command {
        return deploy::run_deploy(yes);
    }

    // Validate GEMINI_API_KEY — prompt user if missing.
    // In headless mode (launched by SwiftUI app), never show an AppleScript dialog —
    // the SwiftUI WelcomeView handles API key entry before launching the daemon.
    let gemini_config = match GeminiConfig::from_env() {
        Ok(config) => config,
        Err(_) if cli.headless => {
            anyhow::bail!(
                "No API key found. The SwiftUI app should configure the key before launching the daemon.\n\
                 Set it manually: echo 'api_key = \"YOUR_KEY\"' > ~/.config/aura/config.toml"
            );
        }
        Err(_) => {
            tracing::info!("No API key found, prompting user...");
            match prompt_api_key() {
                Some(_) => {
                    tracing::info!("API key saved to config");
                    GeminiConfig::from_env()
                        .context("Failed to load config after saving API key")?
                }
                None => {
                    anyhow::bail!(
                        "Aura requires a Gemini API key. Get one at https://aistudio.google.com/apikey\n\
                         Then set it: echo 'api_key = \"YOUR_KEY\"' > ~/.config/aura/config.toml"
                    );
                }
            }
        }
    };
    tracing::info!("Gemini API key validated");

    // Background registration retry: if we have a device_id but no token yet,
    // attempt registration against the proxy and store the result in Keychain.
    if gemini_config.device_id.is_some()
        && gemini_config.device_token.is_none()
        && let Some(ref proxy_url) = gemini_config.proxy_url
    {
        let api_key = gemini_config.api_key.clone();
        let device_id = gemini_config.device_id.clone().unwrap();
        let proxy_base = proxy_url.replace("/ws", "").replace("wss://", "https://");
        std::thread::spawn(move || {
            let rt = match tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
            {
                Ok(rt) => rt,
                Err(e) => {
                    tracing::warn!("Background registration: failed to build runtime: {e}");
                    return;
                }
            };
            rt.block_on(async move {
                tracing::info!("Device token missing, attempting background registration");
                let client = match reqwest::Client::builder()
                    .timeout(std::time::Duration::from_secs(10))
                    .build()
                {
                    Ok(c) => c,
                    Err(e) => {
                        tracing::warn!("Background registration: failed to build client: {e}");
                        return;
                    }
                };
                let resp = match client
                    .post(format!("{proxy_base}/register"))
                    .json(&serde_json::json!({
                        "device_id": device_id,
                        "gemini_api_key": api_key,
                    }))
                    .send()
                    .await
                {
                    Ok(r) => r,
                    Err(e) => {
                        tracing::warn!("Background registration failed: {e}");
                        return;
                    }
                };
                if !resp.status().is_success() {
                    tracing::warn!("Background registration returned {}", resp.status());
                    return;
                }
                let json: serde_json::Value = match resp.json().await {
                    Ok(j) => j,
                    Err(e) => {
                        tracing::warn!("Background registration bad response: {e}");
                        return;
                    }
                };
                if let Some(token) = json.get("device_token").and_then(|v| v.as_str()) {
                    match save_keychain_token(token) {
                        Ok(_) => {
                            tracing::info!("Device registered and token stored in Keychain")
                        }
                        Err(e) => {
                            tracing::warn!("Failed to store device token in Keychain: {e}")
                        }
                    }
                }
            });
        });
    }

    // First-run setup
    let data_dir = AuraSetup::default_data_dir();
    let setup = AuraSetup::new(data_dir.clone());
    setup.ensure_dirs()?;
    setup.print_status();

    // Initialize session memory
    let db_path = data_dir.join("aura.db");
    let memory = SessionMemory::open(&db_path).context("Failed to open session memory database")?;
    let memory = Arc::new(Mutex::new(memory));

    let bus = EventBus::new(EVENT_BUS_CAPACITY);
    let cancel = CancellationToken::new();
    let (ipc_tx, _) = broadcast::channel::<DaemonEvent>(IPC_BROADCAST_CAPACITY);

    if cli.headless {
        // No menu bar — run tokio directly with a single fresh session.
        let session_id = {
            let mem = memory
                .lock()
                .map_err(|e| anyhow::anyhow!("Memory lock poisoned: {e}"))?;
            mem.start_session()
                .context("Failed to start memory session")?
        };
        tracing::info!(session_id = %session_id, "Session memory initialized");
        let has_permission_error = Arc::new(AtomicBool::new(false));
        let rt = tokio::runtime::Runtime::new()?;
        rt.block_on(orchestrator::run_daemon(
            gemini_config,
            bus,
            cancel,
            Arc::clone(&memory),
            session_id,
            None,
            has_permission_error,
            ipc_tx.clone(),
            SessionMode::Fresh,
        ))?;
    } else {
        // Create menu bar app (must run on main thread).
        let (menu_app, menu_tx, reconnect_rx, shutdown_rx) = MenuBarApp::new();

        // Spawn tokio runtime on a background thread and run the reconnect loop.
        let bg_bus = bus.clone();
        let bg_cancel = cancel.clone();
        let bg_ipc_tx = ipc_tx.clone();
        std::thread::spawn(move || {
            let rt = match tokio::runtime::Runtime::new() {
                Ok(rt) => rt,
                Err(e) => {
                    tracing::error!("Failed to create tokio runtime: {e}");
                    return;
                }
            };
            rt.block_on(orchestrator::run_reconnect_loop(
                gemini_config,
                bg_bus,
                bg_cancel,
                memory,
                menu_tx,
                reconnect_rx,
                shutdown_rx,
                bg_ipc_tx,
            ));
        });

        // Run menu bar on main thread (blocks forever)
        menu_app.run();
    }

    Ok(())
}

/// Show a native macOS dialog prompting for the Gemini API key.
/// Returns the key if entered, None if cancelled.
fn prompt_api_key() -> Option<String> {
    let script = r#"
        set dialogResult to display dialog "Welcome to Aura!" & return & return & "Enter your Gemini API key to get started." & return & "Get one free at aistudio.google.com/apikey" with title "Aura Setup" default answer "" buttons {"Cancel", "Save"} default button "Save" with icon note
        set apiKey to text returned of dialogResult
        return apiKey
    "#;

    let output = std::process::Command::new("osascript")
        .arg("-e")
        .arg(script)
        .output()
        .ok()?;

    if !output.status.success() {
        return None;
    }

    let key = String::from_utf8_lossy(&output.stdout).trim().to_string();
    if key.is_empty() {
        return None;
    }

    // U4: Validate API key format before saving
    if !validate_api_key(&key) {
        tracing::warn!("Invalid API key format entered");
        show_error_dialog(
            "Invalid API key format.\n\nA valid Gemini API key is at least 20 characters long, \
             ASCII-only, and contains no whitespace.\n\n\
             Get a key at aistudio.google.com/apikey",
        );
        return None;
    }

    // Save to config file
    if let Some(config_dir) = dirs::config_dir() {
        let aura_dir = config_dir.join("aura");
        let _ = std::fs::create_dir_all(&aura_dir);
        let config_path = aura_dir.join("config.toml");
        let content = format!("api_key = \"{key}\"\n");
        if std::fs::write(&config_path, &content).is_ok() {
            // Secure the file (owner read/write only)
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                let _ =
                    std::fs::set_permissions(&config_path, std::fs::Permissions::from_mode(0o600));
            }
        }
    }

    Some(key)
}

/// Validate that an API key has a plausible format.
/// Checks length, ASCII-only, and no whitespace.
fn validate_api_key(key: &str) -> bool {
    key.len() >= 20 && key.is_ascii() && !key.chars().any(|c| c.is_whitespace())
}

/// Show a native macOS error dialog via osascript.
/// Escapes backslashes and double-quotes to prevent AppleScript injection.
fn show_error_dialog(message: &str) {
    let escaped = message
        .replace('\\', "\\\\")
        .replace('\"', "\\\"")
        .replace('\n', "\" & return & \"");
    let script = format!(
        "display dialog \"{escaped}\" with title \"Aura\" buttons {{\"OK\"}} default button \"OK\" with icon stop"
    );
    let _ = std::process::Command::new("osascript")
        .arg("-e")
        .arg(&script)
        .output();
}

/// Detect Automation/AppleEvents denial from osascript stderr.
/// macOS returns error -1743 (errAEEventNotPermitted) when the user has denied
/// Automation access, and -1744 when consent would be required.
fn is_automation_denied(stderr: &str) -> bool {
    let lower = stderr.to_lowercase();
    lower.contains("-1743")
        || lower.contains("-1744")
        || lower.contains("not authorized to send apple events")
        || lower.contains("is not allowed to send keystrokes")
        || lower.contains("erraeventnotpermitted")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_validate_api_key_valid() {
        assert!(validate_api_key("AIzaSyD1234567890abcdef"));
    }

    #[test]
    fn test_validate_api_key_too_short() {
        assert!(!validate_api_key("short"));
    }

    #[test]
    fn test_validate_api_key_whitespace() {
        assert!(!validate_api_key("AIzaSyD1234567890 abcdef"));
    }

    #[test]
    fn test_validate_api_key_non_ascii() {
        assert!(!validate_api_key("AIzaSyD1234567890🔑abcdef"));
    }

    #[test]
    fn test_is_automation_denied_true() {
        assert!(is_automation_denied("error: -1743 blah"));
        assert!(is_automation_denied(
            "Not authorized to send Apple events to Safari"
        ));
        assert!(is_automation_denied(
            "Terminal is not allowed to send keystrokes"
        ));
    }

    #[test]
    fn test_is_automation_denied_false() {
        assert!(!is_automation_denied("execution error: something else"));
        assert!(!is_automation_denied(""));
    }
}
