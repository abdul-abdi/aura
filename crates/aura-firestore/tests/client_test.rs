use aura_firestore::client::{FirestoreClient, FirestoreFact, validate_device_id};

#[test]
fn fact_roundtrip_serialization() {
    let fact = FirestoreFact {
        category: "preference".into(),
        content: "User prefers dark mode".into(),
        entities: vec!["dark mode".into()],
        importance: 0.8,
        session_id: "test-session".into(),
    };
    let json = serde_json::to_string(&fact).unwrap();
    let parsed: FirestoreFact = serde_json::from_str(&json).unwrap();
    assert_eq!(parsed.category, "preference");
    assert_eq!(parsed.entities.len(), 1);
}

#[test]
fn firestore_client_construction() {
    let client = FirestoreClient::new("my-project".into(), "device-123".into()).unwrap();
    // Just verify it constructs without panic
    drop(client);
}

#[test]
fn rejects_path_traversal_device_id() {
    assert!(validate_device_id("../other-user").is_err());
}

#[test]
fn rejects_slash_in_device_id() {
    assert!(validate_device_id("foo/bar").is_err());
}

#[test]
fn rejects_empty_device_id() {
    assert!(validate_device_id("").is_err());
}

#[test]
fn accepts_valid_device_id() {
    assert!(validate_device_id("abc-123-def_456").is_ok());
}

#[test]
fn rejects_overlong_device_id() {
    let long_id = "a".repeat(129);
    assert!(validate_device_id(&long_id).is_err());
}

#[test]
fn firestore_client_rejects_invalid_device_id() {
    assert!(FirestoreClient::new("my-project".into(), "../evil".into()).is_err());
}
