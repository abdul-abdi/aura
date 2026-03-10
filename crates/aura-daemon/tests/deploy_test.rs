#[test]
fn test_service_url_to_ws_url() {
    assert_eq!(
        aura_daemon::deploy::service_url_to_ws("https://aura-proxy-abc123.us-central1.run.app"),
        "wss://aura-proxy-abc123.us-central1.run.app/ws"
    );
}

#[test]
fn test_service_url_to_ws_url_trailing_slash() {
    assert_eq!(
        aura_daemon::deploy::service_url_to_ws("https://proxy.run.app/"),
        "wss://proxy.run.app/ws"
    );
}

#[test]
fn test_generate_auth_token_length() {
    let token = aura_daemon::deploy::generate_auth_token();
    assert_eq!(token.len(), 64);
    assert!(token.chars().all(|c| c.is_ascii_hexdigit()));
}

#[test]
fn test_generate_auth_token_unique() {
    let a = aura_daemon::deploy::generate_auth_token();
    let b = aura_daemon::deploy::generate_auth_token();
    assert_ne!(a, b);
}

#[test]
fn test_build_config_toml_with_proxy() {
    let content =
        aura_daemon::deploy::build_config_toml(Some("my-api-key"), "wss://proxy.run.app/ws");
    assert!(content.contains("proxy_url = \"wss://proxy.run.app/ws\""));
    assert!(content.contains("api_key = \"my-api-key\""));
}

#[test]
fn test_build_config_toml_no_api_key() {
    let content = aura_daemon::deploy::build_config_toml(None, "wss://proxy.run.app/ws");
    assert!(!content.contains("api_key"));
    assert!(content.contains("proxy_url = \"wss://proxy.run.app/ws\""));
}
