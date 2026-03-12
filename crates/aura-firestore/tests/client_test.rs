use aura_firestore::client::{FirestoreClient, FirestoreFact};

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
    let client = FirestoreClient::new("my-project".into(), "device-123".into());
    // Just verify it constructs without panic
    drop(client);
}
