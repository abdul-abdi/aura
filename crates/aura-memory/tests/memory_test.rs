use aura_memory::{MessageRole, SessionMemory};
use tempfile::TempDir;

fn memory_in_tmpdir() -> (SessionMemory, TempDir) {
    let dir = TempDir::new().unwrap();
    let db_path = dir.path().join("sessions.db");
    let mem = SessionMemory::open(&db_path).unwrap();
    (mem, dir)
}

#[test]
fn test_create_and_list_sessions() {
    let (mem, _dir) = memory_in_tmpdir();
    let id = mem.start_session().unwrap();
    let sessions = mem.list_sessions(10).unwrap();
    assert_eq!(sessions.len(), 1);
    assert_eq!(sessions[0].id, id);
}

#[test]
fn test_add_and_retrieve_messages() {
    let (mem, _dir) = memory_in_tmpdir();
    let sid = mem.start_session().unwrap();
    mem.add_message(&sid, MessageRole::User, "Hello Aura", None)
        .unwrap();
    mem.add_message(&sid, MessageRole::Assistant, "Hey. What's up?", None)
        .unwrap();

    let messages = mem.get_messages(&sid).unwrap();
    assert_eq!(messages.len(), 2);
    assert_eq!(messages[0].role, MessageRole::User);
    assert_eq!(messages[0].content, "Hello Aura");
    assert_eq!(messages[1].role, MessageRole::Assistant);
}

#[test]
fn test_end_session_with_summary() {
    let (mem, _dir) = memory_in_tmpdir();
    let sid = mem.start_session().unwrap();
    mem.add_message(&sid, MessageRole::User, "Open Safari", None)
        .unwrap();
    mem.end_session(&sid, Some("Opened Safari")).unwrap();

    let sessions = mem.list_sessions(10).unwrap();
    assert_eq!(sessions[0].summary.as_deref(), Some("Opened Safari"));
    assert!(sessions[0].ended_at.is_some());
}

#[test]
fn test_multiple_sessions_ordered_by_recency() {
    let (mem, _dir) = memory_in_tmpdir();
    let _s1 = mem.start_session().unwrap();
    std::thread::sleep(std::time::Duration::from_millis(10));
    let s2 = mem.start_session().unwrap();

    let sessions = mem.list_sessions(10).unwrap();
    assert_eq!(sessions[0].id, s2);
}
