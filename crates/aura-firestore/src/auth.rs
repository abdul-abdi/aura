//! Firebase anonymous authentication for Firestore REST access.

use anyhow::{Context, Result};
use serde::Deserialize;

#[derive(Debug, Deserialize)]
struct AnonAuthResponse {
    #[serde(rename = "idToken")]
    id_token: String,
}

/// Get a Firebase anonymous auth token using the web API key.
pub async fn get_anonymous_token(web_api_key: &str) -> Result<String> {
    let url = format!(
        "https://identitytoolkit.googleapis.com/v1/accounts:signUp?key={web_api_key}"
    );
    let client = reqwest::Client::new();
    let resp: AnonAuthResponse = client
        .post(&url)
        .json(&serde_json::json!({"returnSecureToken": true}))
        .send()
        .await
        .context("Firebase anonymous auth request failed")?
        .json()
        .await
        .context("Failed to parse Firebase auth response")?;
    Ok(resp.id_token)
}
