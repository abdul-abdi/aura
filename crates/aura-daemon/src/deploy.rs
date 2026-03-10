use std::process::Command as ShellCommand;

use anyhow::{Result, bail};
use dialoguer::{Confirm, Input};

/// Convert Cloud Run HTTPS URL to WebSocket URL.
/// `https://service.run.app` -> `wss://service.run.app/ws`
pub fn service_url_to_ws(https_url: &str) -> String {
    let base = https_url
        .trim_end_matches('/')
        .replacen("https://", "wss://", 1);
    format!("{base}/ws")
}

/// Generate a cryptographically random 32-byte hex auth token.
pub fn generate_auth_token() -> String {
    use std::io::Read;
    let mut buf = [0u8; 32];
    std::fs::File::open("/dev/urandom")
        .expect("Failed to open /dev/urandom")
        .read_exact(&mut buf)
        .expect("Failed to read random bytes");
    buf.iter().map(|b| format!("{b:02x}")).collect()
}

/// Build config.toml content with optional api_key and proxy_url.
pub fn build_config_toml(api_key: Option<&str>, proxy_url: &str) -> String {
    let mut lines = Vec::new();
    if let Some(key) = api_key {
        lines.push(format!("api_key = \"{key}\""));
    }
    lines.push(format!("proxy_url = \"{proxy_url}\""));
    lines.join("\n") + "\n"
}

// ── gcloud helpers ──────────────────────────────────────────────────────────

fn check_gcloud() -> Result<()> {
    let output = ShellCommand::new("which").arg("gcloud").output()?;
    if output.status.success() {
        Ok(())
    } else {
        bail!("gcloud CLI not found. Install it: https://cloud.google.com/sdk/docs/install")
    }
}

fn get_gcloud_account() -> Option<String> {
    let output = ShellCommand::new("gcloud")
        .args([
            "auth",
            "list",
            "--filter=status:ACTIVE",
            "--format=value(account)",
        ])
        .stderr(std::process::Stdio::null())
        .output()
        .ok()?;
    let account = String::from_utf8_lossy(&output.stdout).trim().to_string();
    if account.is_empty() {
        None
    } else {
        Some(account)
    }
}

fn get_gcloud_project() -> Option<String> {
    let output = ShellCommand::new("gcloud")
        .args(["config", "get-value", "project"])
        .stderr(std::process::Stdio::null())
        .output()
        .ok()?;
    let project = String::from_utf8_lossy(&output.stdout).trim().to_string();
    if project.is_empty() || project == "(unset)" {
        None
    } else {
        Some(project)
    }
}

fn get_gcloud_region() -> Option<String> {
    let output = ShellCommand::new("gcloud")
        .args(["config", "get-value", "run/region"])
        .stderr(std::process::Stdio::null())
        .output()
        .ok()?;
    let region = String::from_utf8_lossy(&output.stdout).trim().to_string();
    if region.is_empty() || region == "(unset)" {
        None
    } else {
        Some(region)
    }
}

fn enable_api(project: &str, api: &str) -> Result<()> {
    let status = ShellCommand::new("gcloud")
        .args(["services", "enable", api, "--project", project, "--quiet"])
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()?;
    if status.success() {
        Ok(())
    } else {
        bail!("Failed to enable API: {api}")
    }
}

fn set_secret(project: &str, name: &str, value: &str) -> Result<()> {
    // Try to create the secret (ignore error if it already exists)
    let _ = ShellCommand::new("gcloud")
        .args([
            "secrets",
            "create",
            name,
            "--replication-policy=automatic",
            "--project",
            project,
            "--quiet",
        ])
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status();

    // Add new version via stdin
    let mut child = ShellCommand::new("gcloud")
        .args([
            "secrets",
            "versions",
            "add",
            name,
            "--data-file=-",
            "--project",
            project,
            "--quiet",
        ])
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::piped())
        .spawn()?;

    if let Some(mut stdin) = child.stdin.take() {
        use std::io::Write;
        stdin.write_all(value.as_bytes())?;
    }

    let status = child.wait()?;
    if status.success() {
        Ok(())
    } else {
        bail!("Failed to store secret '{name}' in Secret Manager")
    }
}

/// Grant the default Compute Engine service account access to a secret.
fn grant_secret_access(project: &str, secret_name: &str) -> Result<()> {
    // Get the project number for the default compute service account
    let output = ShellCommand::new("gcloud")
        .args([
            "projects",
            "describe",
            project,
            "--format=value(projectNumber)",
        ])
        .stderr(std::process::Stdio::null())
        .output()?;
    let project_number = String::from_utf8_lossy(&output.stdout).trim().to_string();
    if project_number.is_empty() {
        bail!("Could not determine project number for {project}");
    }
    let sa = format!("{project_number}-compute@developer.gserviceaccount.com");

    let _ = ShellCommand::new("gcloud")
        .args([
            "secrets",
            "add-iam-policy-binding",
            secret_name,
            &format!("--member=serviceAccount:{sa}"),
            "--role=roles/secretmanager.secretAccessor",
            "--project",
            project,
            "--quiet",
        ])
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status();
    Ok(())
}

/// Get the workspace root directory.
fn workspace_root() -> Result<std::path::PathBuf> {
    let daemon_dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
    let root = daemon_dir
        .parent()
        .and_then(|p| p.parent())
        .ok_or_else(|| anyhow::anyhow!("Could not determine workspace root"))?;
    Ok(root.to_path_buf())
}

/// Deploy to Cloud Run. Returns the service URL on success.
fn deploy_cloud_run(project: &str, region: &str, auth_secret: Option<&str>) -> Result<String> {
    let root = workspace_root()?;
    let source = root.to_string_lossy().to_string();

    // Cloud Build expects a Dockerfile at the source root. Copy it temporarily.
    let dockerfile_src = root.join("crates/aura-proxy/Dockerfile");
    let dockerfile_dst = root.join("Dockerfile");
    let needs_cleanup = !dockerfile_dst.exists();
    std::fs::copy(&dockerfile_src, &dockerfile_dst)?;

    let mut args = vec![
        "run".to_string(),
        "deploy".into(),
        "aura-proxy".into(),
        "--source".into(),
        source,
        "--project".into(),
        project.to_string(),
        "--region".into(),
        region.to_string(),
        "--platform".into(),
        "managed".into(),
        "--port".into(),
        "8080".into(),
        "--memory".into(),
        "256Mi".into(),
        "--cpu".into(),
        "1".into(),
        "--min-instances".into(),
        "0".into(),
        "--max-instances".into(),
        "10".into(),
        "--timeout".into(),
        "3600".into(),
        "--allow-unauthenticated".into(),
        "--quiet".into(),
    ];

    if let Some(secret_name) = auth_secret {
        args.push("--set-secrets".into());
        args.push(format!("AURA_PROXY_AUTH_TOKEN={secret_name}:latest"));
    }

    let status = ShellCommand::new("gcloud")
        .args(&args)
        .stderr(std::process::Stdio::inherit())
        .status();

    // Clean up the temporary Dockerfile regardless of deploy result
    if needs_cleanup {
        let _ = std::fs::remove_file(&dockerfile_dst);
    }

    if !status?.success() {
        bail!("Cloud Run deployment failed. Check the output above for details.");
    }

    // Retrieve the service URL via a separate describe call
    let output = ShellCommand::new("gcloud")
        .args([
            "run",
            "services",
            "describe",
            "aura-proxy",
            "--project",
            project,
            "--region",
            region,
            "--format",
            "value(status.url)",
            "--quiet",
        ])
        .stderr(std::process::Stdio::null())
        .output()?;

    let url = String::from_utf8_lossy(&output.stdout).trim().to_string();
    if url.is_empty() {
        bail!("Deployment succeeded but could not retrieve service URL");
    }
    Ok(url)
}

fn read_existing_api_key() -> Option<String> {
    let path = dirs::config_dir()?.join("aura/config.toml");
    let content = std::fs::read_to_string(path).ok()?;
    let table: toml::Table = content.parse().ok()?;
    table.get("api_key")?.as_str().map(String::from)
}

fn print_success(service_url: &str, ws_url: &str, auth_token: &Option<String>) {
    println!();
    println!("  \x1b[32m✓ Deployed successfully!\x1b[0m");
    println!();
    println!("  Service URL:  {service_url}");
    println!("  WebSocket:    {ws_url}");
    println!();
    println!("  Config written to ~/.config/aura/config.toml");
    if let Some(token) = auth_token {
        println!();
        println!("  \x1b[1mAuth token (set this on the client machine):\x1b[0m");
        println!("  export AURA_PROXY_AUTH_TOKEN=\"{token}\"");
    }
    println!();
    println!("  Run \x1b[1maura\x1b[0m to start with the cloud proxy.");
    println!();
}

// ── Main deploy flow ────────────────────────────────────────────────────────

pub fn run_deploy(auto_yes: bool) -> Result<()> {
    println!();
    println!("  \x1b[1mAura Cloud Deploy\x1b[0m");
    println!("  ─────────────────");
    println!();

    // 1. Check gcloud CLI
    check_gcloud()?;

    // 2. Check authentication
    match get_gcloud_account() {
        Some(acct) => {
            println!("  Checking gcloud CLI... \x1b[32m✓\x1b[0m authenticated as {acct}");
        }
        None => {
            println!("  Checking gcloud CLI... \x1b[33mnot authenticated\x1b[0m");
            println!("  Running gcloud auth login...");
            let status = ShellCommand::new("gcloud")
                .args(["auth", "login"])
                .status()?;
            if !status.success() {
                bail!("Authentication failed. Run `gcloud auth login` manually.");
            }
        }
    }
    println!();

    // 3. Project selection
    let project = match get_gcloud_project() {
        Some(p) => {
            println!("  GCP Project: \x1b[1m{p}\x1b[0m (from gcloud config)");
            if auto_yes || confirm_default_yes("  Use this project?")? {
                p
            } else {
                Input::<String>::new()
                    .with_prompt("  Enter GCP project ID")
                    .interact_text()?
            }
        }
        None => {
            if auto_yes {
                bail!("No gcloud project set. Run `gcloud config set project PROJECT_ID` first.");
            }
            Input::<String>::new()
                .with_prompt("  Enter GCP project ID")
                .interact_text()?
        }
    };
    println!();

    // 4. Region selection
    let default_region = get_gcloud_region().unwrap_or_else(|| "us-central1".to_string());
    println!("  Region: \x1b[1m{default_region}\x1b[0m");
    let region = if auto_yes || confirm_default_yes("  Use this region?")? {
        default_region
    } else {
        Input::<String>::new()
            .with_prompt("  Enter Cloud Run region")
            .default("us-central1".to_string())
            .interact_text()?
    };
    println!();

    // 5. Auth token
    let auth_token =
        if auto_yes || confirm_default_yes("  Generate a random auth token for the proxy?")? {
            let token = generate_auth_token();
            println!(
                "  Auth token: \x1b[2m{}...\x1b[0m (stored in Secret Manager)",
                &token[..16]
            );
            Some(token)
        } else {
            println!("  \x1b[33mWarning:\x1b[0m Proxy will accept unauthenticated connections.");
            None
        };
    println!();

    // 6. Enable APIs
    println!("  Enabling GCP APIs...");
    let mut apis = vec![
        "run.googleapis.com",
        "cloudbuild.googleapis.com",
        "artifactregistry.googleapis.com",
    ];
    if auth_token.is_some() {
        apis.push("secretmanager.googleapis.com");
    }
    for api in &apis {
        println!("    Enabling {api}...");
        enable_api(&project, api)?;
    }
    println!("  Enabling GCP APIs... \x1b[32m✓\x1b[0m");

    // 7. Store secret
    let secret_name = if let Some(ref token) = auth_token {
        println!("  Storing auth token in Secret Manager...");
        set_secret(&project, "aura-proxy-auth-token", token)?;
        grant_secret_access(&project, "aura-proxy-auth-token")?;
        println!("  Storing auth token in Secret Manager... \x1b[32m✓\x1b[0m");
        Some("aura-proxy-auth-token")
    } else {
        None
    };

    // 8. Deploy
    println!();
    println!("  Building & deploying to Cloud Run (this takes 2-5 minutes)...");
    let service_url = deploy_cloud_run(&project, &region, secret_name)?;
    println!("  Building & deploying to Cloud Run... \x1b[32m✓\x1b[0m");

    let ws_url = service_url_to_ws(&service_url);

    // 9. Write config
    let existing_api_key = read_existing_api_key();
    let config_content = build_config_toml(existing_api_key.as_deref(), &ws_url);

    if let Some(config_dir) = dirs::config_dir() {
        let aura_dir = config_dir.join("aura");
        let _ = std::fs::create_dir_all(&aura_dir);
        let config_path = aura_dir.join("config.toml");

        if config_path.exists() && !auto_yes {
            let existing = std::fs::read_to_string(&config_path).unwrap_or_default();
            if existing.contains("proxy_url")
                && !confirm_default_yes("  Config already has a proxy_url. Overwrite?")?
            {
                println!("  Skipped config write. Manually add:");
                println!("    proxy_url = \"{ws_url}\"");
                print_success(&service_url, &ws_url, &auth_token);
                return Ok(());
            }
        }

        std::fs::write(&config_path, &config_content)?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let _ = std::fs::set_permissions(&config_path, std::fs::Permissions::from_mode(0o600));
        }
    }

    print_success(&service_url, &ws_url, &auth_token);
    Ok(())
}

fn confirm_default_yes(prompt: &str) -> Result<bool> {
    Ok(Confirm::new()
        .with_prompt(prompt)
        .default(true)
        .interact()?)
}
