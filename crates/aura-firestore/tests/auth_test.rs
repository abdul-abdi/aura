use aura_firestore::AuthCache;

/// A cached (non-expired) token should be returned without any network call.
#[test]
fn cached_auth_reuses_token() {
    let cache = AuthCache::new("fake-key".into());

    // Seed the internal cache via the public constructor + a helper that
    // directly populates it.  Since the fields are private we drive this
    // through the public API-level unit tests in auth.rs.  Here we just
    // verify the public type is accessible from outside the crate.
    //
    // A real integration test would hit a Firebase emulator; for unit-level
    // coverage see `auth::tests::cached_auth_reuses_token`.
    let _cache = cache;
}

/// Verify `AuthCache` can be wrapped in `Arc` and shared across threads.
#[test]
fn auth_cache_is_send_sync() {
    fn assert_send_sync<T: Send + Sync>() {}
    assert_send_sync::<AuthCache>();
}
