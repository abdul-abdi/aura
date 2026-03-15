# Per-Device Token Registration

## Problem

The current auth model uses a single shared secret for both the proxy and memory agent. If this token is compiled into the release binary, anyone can extract it via `strings`. There is no way to revoke a leaked token without rotating the secret for all users.

## Solution

Replace the shared secret with per-device tokens. Each device registers once during onboarding, receives a unique token, and stores it in the macOS Keychain. The proxy and memory agent validate tokens against a Firestore device registry.

## Registration Flow

1. User completes API key entry in SwiftUI onboarding (existing flow).
2. `AppState.completeWelcome()` generates a UUID v4 `device_id`, writes it to `~/.config/aura/config.toml`.
3. Client calls `POST /register` on the proxy with `{ device_id, gemini_api_key }`.
4. Proxy validates `device_id` format: alphanumeric, hyphens, underscores only, max 128 chars. Invalid → 400.
5. Proxy validates the Gemini key by calling `GET https://generativelanguage.googleapis.com/v1beta/models?key={gemini_api_key}`. Invalid key → 401.
6. Proxy generates a 64-char cryptographically random hex token.
7. Proxy stores `{ token_hash (SHA-256), gemini_key_hash (SHA-256, for binding), created_at, last_seen }` in Firestore `devices/{device_id}`.
8. Proxy returns `{ device_token }` to the client.
9. Client stores `device_token` in macOS Keychain under service `com.aura.desktop`, account `device_token`.

**Rate limiting:** `/register` is rate-limited to 3 requests per IP per hour. This prevents bulk device registration abuse.

**Failure handling:** If registration fails (no network, proxy down), onboarding still completes. The app works in direct mode (Gemini API key only, no proxy or memory agent). On next daemon launch, if `device_id` exists in config but no Keychain token is found, the daemon retries registration in the background.

**Re-registration:** If a `devices/{device_id}` document already exists, the proxy generates a new token, overwrites the old `token_hash` in Firestore, and returns the new token. The old token is immediately invalidated. The Gemini key must match the one used at original registration (compared via `gemini_key_hash`); mismatch → 403. If a daemon re-registers while an active session is using the old token, the daemon must reconnect the WebSocket with the new token promptly (within the 60-second cache window).

**TLS:** All `/register` calls are over HTTPS. Cloud Run enforces TLS termination — plaintext HTTP is not accepted.

**Logging:** The proxy must not log the raw `gemini_api_key` from registration requests. Only the `gemini_key_prefix` (first 8 chars) may appear in logs.

## Auth Validation

### Proxy (`/ws` WebSocket upgrade)

1. Client sends `x-device-id` and `x-device-token` headers (replaces `x-auth-token`).
2. Proxy checks in-memory cache first (`device_id` → `token_hash`, TTL 60 seconds).
3. Cache miss → reads `devices/{device_id}` from Firestore, SHA-256 hashes the provided token, constant-time compares.
4. Valid → proceed with WebSocket relay. Client still sends `x-gemini-key` for the upstream Gemini connection.
5. Invalid or missing → 401.

The `/ws/auth` preflight endpoint is updated to accept `x-device-id` + `x-device-token` (same as `/ws`), with legacy fallback when enabled.

### Memory Agent (`/query`, `/ingest`, `/consolidate`)

1. Client sends `Bearer {device_token}` in the Authorization header and `device_id` in the request body.
2. Agent checks in-memory cache (same pattern, TTL 60 seconds).
3. Cache miss → reads `devices/{device_id}` from Firestore, validates token hash.
4. Verifies `device_id` in the request body matches the token's device — prevents impersonation.

### Consolidation Service

The Rust consolidation service (`infrastructure/consolidation/`) currently uses `AURA_AUTH_TOKEN`. It receives the same dual-auth treatment: accept legacy shared token OR device token during rollout. It reads `devices/{device_id}` from Firestore using the same service account credentials.

**Note:** Registration is handled exclusively by the proxy. The memory agent and consolidation service only validate tokens — they never issue them.

### Backward Compatibility

During rollout, all three services (proxy, memory agent, consolidation) accept the old shared token OR a valid device token. Per-service env var flags control this:

- Proxy: `LEGACY_AUTH_ENABLED=true` (checks `AURA_PROXY_AUTH_TOKEN`)
- Memory agent: `LEGACY_AUTH_ENABLED=true` (checks `AURA_AUTH_TOKEN`)
- Consolidation: `LEGACY_AUTH_ENABLED=true` (checks `AURA_AUTH_TOKEN`)

Once all clients have registered, set all to `false` and delete the shared secrets from GCP Secret Manager.

## Data Model

### New Firestore Collection: `devices/{device_id}`

```
{
  token_hash: string,         // SHA-256 hex of the device token
  gemini_key_hash: string,    // SHA-256 hex of the Gemini key (for re-registration binding)
  created_at: timestamp,
  last_seen: timestamp         // fire-and-forget update on cache miss, at most once per TTL
}
```

### Updated Firestore Rules

```
match /devices/{deviceId} {
  // Server-side only. Proxy, memory agent, and consolidation service
  // use service account credentials (Admin/REST API) which bypass rules.
  allow read, write: if false;
}

match /users/{deviceId}/{document=**} {
  // No change for now. All client access to /users goes through the
  // memory agent (server-side). This rule is kept permissive until
  // direct client Firestore access is introduced.
  allow read, write: if request.auth != null;
}
```

## Client-Side Changes

### Swift (AuraApp)

- New `KeychainHelper` utility: `save(service, account, data)`, `read(service, account)`, `delete(service, account)` wrapping `SecItemAdd`, `SecItemCopyMatching`, `SecItemDelete`.
- `AppState.completeWelcome()`: after saving API key, generate UUID `device_id` → write to `config.toml` → call `/register` → store token in Keychain.
- The app is not sandboxed (no App Store distribution). Default Keychain access works without an entitlements file or access group. No entitlements changes needed.

### Rust (aura-gemini config.rs)

- Add `security-framework` crate dependency.
- New `read_keychain_token()`: reads `device_token` from Keychain service `com.aura.desktop`.
- `from_env()` auth priority: env var (dev override) > Keychain > config.toml.
- Remove `option_env!()` for auth tokens. Keep `option_env!()` for URLs only (not secret).

### Rust (aura-daemon startup)

- On launch, if `device_id` exists in config but no Keychain token → attempt background registration retry.
- `CloudConfig` carries `device_id` + `device_token` (from Keychain) instead of shared auth tokens.

## Proxy Implementation Notes

The proxy currently has no Firestore dependency. The following are needed:

- **Firestore client:** Use the Firestore REST API (`reqwest`) rather than a full gRPC client crate, keeping the dependency light. The proxy only needs `get` and `set` on `devices/{device_id}`.
- **In-memory cache:** `DashMap<String, CachedDevice>` with a 60-second TTL. Eviction on read (check timestamp, discard if stale).
- **GCP credentials:** The proxy runs on Cloud Run with a service account. Use Application Default Credentials (ADC) via the metadata server — no credential files needed.
- **Startup change:** During dual-auth rollout, the proxy should not panic if `AURA_PROXY_AUTH_TOKEN` is unset. Only panic if neither legacy auth nor Firestore credentials are available.

## What Gets Removed

- `prod_defaults::PROXY_AUTH_TOKEN` and `CLOUD_RUN_AUTH_TOKEN` from `config.rs`.
- `proxy_auth_token` and `cloud_run_auth_token` fields from `GeminiConfig`.
- GitHub secrets: `AURA_PROD_PROXY_AUTH_TOKEN`, `AURA_PROD_CLOUD_RUN_AUTH_TOKEN`, `AURA_STAGING_PROXY_AUTH_TOKEN`, `AURA_STAGING_CLOUD_RUN_AUTH_TOKEN`.
- Release workflow env vars for auth tokens (URL env vars kept).
- After rollout: `AURA_PROXY_AUTH_TOKEN` from proxy, `AURA_AUTH_TOKEN` from memory agent and consolidation service.

## What Gets Kept

- `prod_defaults::PROXY_URL` and `CLOUD_RUN_URL` as `option_env!()` — service addresses are not secret.
- GitHub secrets for URLs.
- Legacy shared tokens on servers temporarily during rollout (`LEGACY_AUTH_ENABLED=true`).

## Token Revocation

Manual via Firestore console: delete the `devices/{device_id}` document. All three services fail auth on next request (after cache TTL expires, max 60 seconds). No admin API needed at this stage.

Token expiration is intentionally deferred. The current user base is small and manual revocation is sufficient. If the user base grows, add an `expires_at` field and a client-side refresh flow (re-call `/register` with the Gemini key).

## Testing

### Unit Tests

- `config.rs`: `from_env()` reads device token from Keychain mock, falls back when absent. Keychain takes priority over config.toml.
- `KeychainHelper` (Swift): save/read/delete round-trip.
- Proxy `/register`: valid key → 200 + token, invalid key → 401, re-registration → new token + old invalidated, mismatched Gemini key → 403, invalid device_id format → 400, rate limit exceeded → 429.
- Proxy `/ws` auth: valid device token → upgrade, invalid → 401, legacy shared token → upgrade (when `LEGACY_AUTH_ENABLED=true`), legacy disabled → 401.
- Memory agent auth: valid device token, invalid, legacy fallback.
- Consolidation service auth: same pattern as memory agent.

### Integration Tests

- Full flow: register → Keychain store → proxy connect via device token → WebSocket relay works.
- Memory agent: register → ingest → query with device token.
- Revocation: delete device doc → auth fails after cache expires (≤60s).
- Migration: old binary with shared token → upgrade to new binary → device registers → shared token can be disabled.

### Manual Tests

- Fresh install: API key entry → device registered → proxy + memory agent connected.
- Reinstall: Keychain token survives, no re-registration.
- No network during onboarding: direct mode works, background retry succeeds later.
- Keychain access denied: graceful fallback to direct mode, warning logged.
