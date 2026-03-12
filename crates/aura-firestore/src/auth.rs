//! Firebase anonymous authentication with token caching.
//!
//! [`AuthCache`] signs up once, caches the id-token + refresh-token, and
//! transparently refreshes before expiry so callers never create orphaned
//! Firebase accounts.

use anyhow::{Context, Result};
use serde::Deserialize;
use std::sync::Mutex;
use std::time::{Duration, Instant};

/// Buffer before actual expiry at which we proactively refresh.
const EXPIRY_BUFFER: Duration = Duration::from_secs(5 * 60);

struct CachedAuth {
    id_token: String,
    refresh_token: String,
    #[allow(dead_code)]
    local_id: String,
    expires_at: Instant,
}

/// Caches a Firebase anonymous identity so we reuse the same UID across calls.
pub struct AuthCache {
    web_api_key: String,
    cached: Mutex<Option<CachedAuth>>,
    client: reqwest::Client,
}

#[derive(Debug, Deserialize)]
struct SignUpResponse {
    #[serde(rename = "idToken")]
    id_token: String,
    #[serde(rename = "refreshToken")]
    refresh_token: String,
    #[serde(rename = "localId")]
    local_id: String,
    #[serde(rename = "expiresIn")]
    expires_in: String,
}

#[derive(Debug, Deserialize)]
struct RefreshResponse {
    id_token: String,
    refresh_token: String,
    expires_in: String,
}

impl AuthCache {
    /// Create a new cache with no stored credentials.
    pub fn new(web_api_key: String) -> Self {
        Self {
            web_api_key,
            cached: Mutex::new(None),
            client: reqwest::Client::new(),
        }
    }

    /// Return a valid id-token, refreshing or signing up as needed.
    pub async fn get_token(&self) -> Result<String> {
        // Check cache under a short lock.
        let maybe = {
            let guard = self.cached.lock().expect("auth cache lock poisoned");
            guard
                .as_ref()
                .map(|c| (c.id_token.clone(), c.refresh_token.clone(), c.expires_at))
        };

        match maybe {
            Some((token, _refresh_tok, expires_at))
                if Instant::now() + EXPIRY_BUFFER < expires_at =>
            {
                Ok(token)
            }
            Some((_, refresh_tok, _)) => self.refresh_token(&refresh_tok).await,
            None => self.sign_up_anonymous().await,
        }
    }

    /// POST to Firebase Identity Toolkit `signUp` endpoint.
    async fn sign_up_anonymous(&self) -> Result<String> {
        let url = format!(
            "https://identitytoolkit.googleapis.com/v1/accounts:signUp?key={}",
            self.web_api_key
        );
        let resp: SignUpResponse = self
            .client
            .post(&url)
            .json(&serde_json::json!({"returnSecureToken": true}))
            .send()
            .await
            .context("Firebase anonymous sign-up request failed")?
            .json()
            .await
            .context("Failed to parse Firebase sign-up response")?;

        let expires_in_secs: u64 = resp
            .expires_in
            .parse()
            .context("invalid expiresIn in sign-up response")?;

        let id_token = resp.id_token.clone();

        {
            let mut guard = self.cached.lock().expect("auth cache lock poisoned");
            *guard = Some(CachedAuth {
                id_token: resp.id_token,
                refresh_token: resp.refresh_token,
                local_id: resp.local_id,
                expires_at: Instant::now() + Duration::from_secs(expires_in_secs),
            });
        }

        Ok(id_token)
    }

    /// POST to `securetoken.googleapis.com/v1/token` to refresh an expired token.
    async fn refresh_token(&self, refresh_token: &str) -> Result<String> {
        let url = format!(
            "https://securetoken.googleapis.com/v1/token?key={}",
            self.web_api_key
        );
        let resp: RefreshResponse = self
            .client
            .post(&url)
            .form(&[
                ("grant_type", "refresh_token"),
                ("refresh_token", refresh_token),
            ])
            .send()
            .await
            .context("Firebase token refresh request failed")?
            .json()
            .await
            .context("Failed to parse Firebase refresh response")?;

        let expires_in_secs: u64 = resp
            .expires_in
            .parse()
            .context("invalid expires_in in refresh response")?;

        let id_token = resp.id_token.clone();

        {
            let mut guard = self.cached.lock().expect("auth cache lock poisoned");
            if let Some(cached) = guard.as_mut() {
                cached.id_token = resp.id_token;
                cached.refresh_token = resp.refresh_token;
                cached.expires_at = Instant::now() + Duration::from_secs(expires_in_secs);
            }
        }

        Ok(id_token)
    }
}

/// Legacy helper — creates a fresh anonymous user on every call.
/// Prefer [`AuthCache`] for production use.
#[deprecated(note = "use AuthCache::get_token() instead")]
pub async fn get_anonymous_token(web_api_key: &str) -> Result<String> {
    let cache = AuthCache::new(web_api_key.to_owned());
    cache.sign_up_anonymous().await
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cached_auth_reuses_token() {
        let cache = AuthCache::new("fake-key".into());

        // Manually seed the cache with a token that expires far in the future.
        {
            let mut guard = cache.cached.lock().unwrap();
            *guard = Some(CachedAuth {
                id_token: "cached-token-abc".into(),
                refresh_token: "rt-xyz".into(),
                local_id: "uid-123".into(),
                expires_at: Instant::now() + Duration::from_secs(3600),
            });
        }

        // get_token should return the cached value without any network call.
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        let token = rt.block_on(cache.get_token()).unwrap();
        assert_eq!(token, "cached-token-abc");
    }

    #[test]
    fn expired_token_triggers_refresh() {
        let cache = AuthCache::new("fake-key".into());

        // Seed with an already-expired token.
        {
            let mut guard = cache.cached.lock().unwrap();
            *guard = Some(CachedAuth {
                id_token: "stale-token".into(),
                refresh_token: "rt-old".into(),
                local_id: "uid-456".into(),
                expires_at: Instant::now() - Duration::from_secs(1),
            });
        }

        // get_token should try to refresh, which will fail (no real server)
        // — the important thing is it does NOT return the stale token.
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        let result = rt.block_on(cache.get_token());
        assert!(result.is_err(), "expected refresh to fail against fake key");
    }
}
