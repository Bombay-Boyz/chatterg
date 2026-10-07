use chatterg::{
    domain::{Conversation, Engine, Questionnaire, Submission},
    storage::{StorageError, Store, sqlite::SqliteStore},
};

fn conversation_at(position: usize) -> Conversation {
    Conversation { position, messages_sent: position as u64, ..Conversation::new() }
}

#[tokio::test]
async fn empty_database_loads_none() {
    let store = SqliteStore::memory().unwrap();
    assert!(store.load().await.unwrap().is_none());
}

#[tokio::test]
async fn first_save_then_load_round_trips() {
    let store = SqliteStore::memory().unwrap();
    let conversation = conversation_at(3);

    store.save(&conversation).await.unwrap();

    assert_eq!(store.load().await.unwrap().unwrap(), conversation);
}

#[tokio::test]
async fn repeated_save_keeps_one_logical_conversation() {
    let store = SqliteStore::memory().unwrap();

    store.save(&conversation_at(1)).await.unwrap();
    store.save(&conversation_at(2)).await.unwrap();

    assert_eq!(store.load().await.unwrap().unwrap().position, 2);
}

#[tokio::test]
async fn state_survives_process_restart() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("chatterg.db");

    {
        let store = SqliteStore::open(&path).unwrap();
        store.save(&conversation_at(4)).await.unwrap();
    }

    let reopened = SqliteStore::open(&path).unwrap();
    assert_eq!(reopened.load().await.unwrap().unwrap().position, 4);
}

#[tokio::test]
async fn malformed_persisted_data_is_an_error() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("chatterg.db");

    drop(SqliteStore::open(&path).unwrap());

    let connection = rusqlite::Connection::open(&path).unwrap();
    connection.execute("INSERT INTO conversations (id, data) VALUES (1, 'not json')", []).unwrap();
    drop(connection);

    let store = SqliteStore::open(&path).unwrap();
    assert!(matches!(store.load().await, Err(StorageError::Corrupt(_))));
}

#[tokio::test]
async fn old_format_without_position_is_rejected_not_guessed() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("chatterg.db");

    drop(SqliteStore::open(&path).unwrap());

    let connection = rusqlite::Connection::open(&path).unwrap();
    connection
        .execute(
            "INSERT INTO conversations (id, data) VALUES (1, '{\"state\":\"Ready\",\"answers\":[]}')",
            [],
        )
        .unwrap();
    drop(connection);

    let store = SqliteStore::open(&path).unwrap();
    assert!(matches!(store.load().await, Err(StorageError::Corrupt(_))));
}

#[test]
fn unavailable_database_is_an_error() {
    let result = SqliteStore::open("/definitely/not/a/dir/chatterg.db");
    assert!(matches!(result, Err(StorageError::Open { .. })));
}

#[tokio::test]
async fn engine_state_round_trips_through_store() {
    let questionnaire: Questionnaire =
        serde_yaml::from_str(include_str!("../questions.yaml")).unwrap();
    let Submission::Next(engine, _) = Engine::new(questionnaire).submit("Acme".into()) else {
        panic!("expected next");
    };

    let store = SqliteStore::memory().unwrap();
    store.save(engine.conversation()).await.unwrap();

    assert_eq!(&store.load().await.unwrap().unwrap(), engine.conversation());
}

// ---- schema versions, locking, WAL, archive --------------------------------

use chatterg::domain::SCHEMA_VERSION;

fn insert_raw(path: &std::path::Path, json: &str) {
    let connection = rusqlite::Connection::open(path).unwrap();
    connection.execute("INSERT INTO conversations (id, data) VALUES (1, ?1)", [json]).unwrap();
}

#[tokio::test]
async fn version_one_data_is_migrated_on_load() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("c.db");
    drop(SqliteStore::open(&path).unwrap());

    // exactly what a pre-timestamp chatterg wrote: no schema_version, no timing
    insert_raw(
        &path,
        r#"{"state":"Ready","answers":[{"question_id":"a","attempts":[
            {"number":1,"request":"A?","response":"x","validation":"accepted"}]}],
            "position":1,"messages_sent":1}"#,
    );

    let loaded = SqliteStore::open(&path).unwrap().load().await.unwrap().unwrap();

    assert_eq!(loaded.schema_version, SCHEMA_VERSION);
    assert_eq!(loaded.position, 1);
    assert_eq!(loaded.answers[0].attempts[0].sent_at, None);
    assert_eq!(loaded.questionnaire_fingerprint, None);
}

#[tokio::test]
async fn data_from_a_newer_chatterg_is_refused() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("c.db");
    drop(SqliteStore::open(&path).unwrap());
    insert_raw(
        &path,
        r#"{"schema_version":99,"state":"Ready","answers":[],"position":0,"messages_sent":0}"#,
    );

    let result = SqliteStore::open(&path).unwrap().load().await;

    assert!(matches!(result, Err(StorageError::UnsupportedVersion { found: 99, .. })));
}

#[tokio::test]
async fn timing_survives_a_round_trip() {
    let questionnaire: Questionnaire =
        serde_yaml::from_str(include_str!("../questions.yaml")).unwrap();
    let at = chrono::Utc::now();

    let mut engine = Engine::new(questionnaire);
    engine.begin(at);
    let Submission::Next(engine, _) =
        engine.submit_timed("Acme".into(), chatterg::domain::Timing::new(at, at))
    else {
        panic!("expected next");
    };

    let store = SqliteStore::memory().unwrap();
    store.save(engine.conversation()).await.unwrap();

    assert_eq!(&store.load().await.unwrap().unwrap(), engine.conversation());
}

#[test]
fn second_open_of_the_same_store_is_refused() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("c.db");

    let first = SqliteStore::open(&path).unwrap();
    let second = SqliteStore::open(&path);

    assert!(matches!(second, Err(StorageError::Locked { .. })));
    assert!(second.err().unwrap().to_string().contains("another chatterg"));

    drop(first);
    assert!(SqliteStore::open(&path).is_ok(), "lock must be released on drop");
}

#[test]
fn different_stores_do_not_block_each_other() {
    let dir = tempfile::tempdir().unwrap();

    let _a = SqliteStore::open(dir.path().join("a.db")).unwrap();
    let _b = SqliteStore::open(dir.path().join("b.db")).unwrap();
}

#[test]
fn database_uses_write_ahead_logging() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("c.db");
    let _store = SqliteStore::open(&path).unwrap();

    let mode: String = rusqlite::Connection::open(&path)
        .unwrap()
        .query_row("PRAGMA journal_mode", [], |row| row.get(0))
        .unwrap();

    assert_eq!(mode, "wal");
}

#[tokio::test]
async fn archive_copies_the_run_then_clears_it() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("c.db");
    let backup = dir.path().join("c.db.bak");

    let store = SqliteStore::open(&path).unwrap();
    store.save(&conversation_at(3)).await.unwrap();

    assert!(store.archive_and_reset(&backup).unwrap());

    assert!(store.load().await.unwrap().is_none(), "store must be empty afterwards");

    let rows: i64 = rusqlite::Connection::open(&backup)
        .unwrap()
        .query_row("SELECT COUNT(*) FROM conversations", [], |row| row.get(0))
        .unwrap();
    assert_eq!(rows, 1, "backup keeps the old run");
}

#[test]
fn archiving_an_empty_store_does_nothing() {
    let dir = tempfile::tempdir().unwrap();
    let backup = dir.path().join("none.bak");

    let store = SqliteStore::memory().unwrap();

    assert!(!store.archive_and_reset(&backup).unwrap());
    assert!(!backup.exists());
}

// ---- read-only opening (used by `chatterg report`) ----------------------------------

#[tokio::test]
async fn read_only_store_reads_while_a_run_holds_the_lock() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("c.db");

    let writer = SqliteStore::open(&path).unwrap();
    writer.save(&conversation_at(3)).await.unwrap();

    // the writer is still open and still owns the lock
    let reader = SqliteStore::open_read_only(&path).unwrap();

    assert_eq!(reader.load().await.unwrap().unwrap().position, 3);

    // and it sees later saves too
    writer.save(&conversation_at(4)).await.unwrap();
    assert_eq!(reader.load().await.unwrap().unwrap().position, 4);
}

#[tokio::test]
async fn read_only_store_cannot_change_anything() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("c.db");
    drop(SqliteStore::open(&path).unwrap());

    let reader = SqliteStore::open_read_only(&path).unwrap();

    assert!(matches!(reader.save(&conversation_at(1)).await, Err(StorageError::Database(_))));
    assert!(reader.load().await.unwrap().is_none());
}

#[test]
fn read_only_open_never_creates_a_notebook() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("missing.db");

    let result = SqliteStore::open_read_only(&path);

    assert!(matches!(&result, Err(StorageError::Open { .. })));
    assert!(result.err().unwrap().to_string().contains("no such notebook"));
    assert!(!path.exists());
    assert!(!dir.path().join("missing.db.lock").exists(), "no lock file may be created either");
}

#[test]
fn read_only_open_rejects_a_database_that_is_not_a_notebook() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("other.db");

    let connection = rusqlite::Connection::open(&path).unwrap();
    connection.execute("CREATE TABLE something_else (x INTEGER)", []).unwrap();
    drop(connection);

    let error = SqliteStore::open_read_only(&path).err().unwrap();

    assert!(error.to_string().contains("not a chatterg notebook"), "{error}");
}
