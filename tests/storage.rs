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
