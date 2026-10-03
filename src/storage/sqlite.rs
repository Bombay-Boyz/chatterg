use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use rusqlite::{Connection, OptionalExtension, params};

use crate::domain::Conversation;

use super::{StorageError, Store};

const SCHEMA: &str = "
    CREATE TABLE IF NOT EXISTS conversations (
        id INTEGER PRIMARY KEY CHECK (id = 1),
        data TEXT NOT NULL
    );
";

pub struct SqliteStore {
    connection: Arc<Mutex<Connection>>,
}

fn database(error: rusqlite::Error) -> StorageError {
    StorageError::Database(Box::new(error))
}

impl SqliteStore {
    pub fn open(path: impl AsRef<std::path::Path>) -> Result<Self, StorageError> {
        let path = path.as_ref();

        let connection = Connection::open(path).map_err(|error| StorageError::Open {
            path: path.display().to_string(),
            source: Box::new(error),
        })?;

        Self::initialise(connection, |error| StorageError::Open {
            path: path.display().to_string(),
            source: Box::new(error),
        })
    }

    pub fn memory() -> Result<Self, StorageError> {
        let connection = Connection::open_in_memory().map_err(database)?;
        Self::initialise(connection, database)
    }

    fn initialise(
        connection: Connection,
        on_error: impl FnOnce(rusqlite::Error) -> StorageError,
    ) -> Result<Self, StorageError> {
        connection.execute_batch(SCHEMA).map_err(on_error)?;
        Ok(Self { connection: Arc::new(Mutex::new(connection)) })
    }
}

#[async_trait]
impl Store for SqliteStore {
    async fn save(&self, conversation: &Conversation) -> Result<(), StorageError> {
        let data = serde_json::to_string(conversation).map_err(StorageError::Serialize)?;
        let connection = Arc::clone(&self.connection);

        tokio::task::spawn_blocking(move || {
            let connection = connection
                .lock()
                .map_err(|_| StorageError::Internal("connection lock poisoned".into()))?;

            connection
                .execute(
                    "INSERT INTO conversations (id, data) VALUES (1, ?1)
                     ON CONFLICT(id) DO UPDATE SET data = excluded.data",
                    params![data],
                )
                .map_err(database)?;

            Ok(())
        })
        .await
        .map_err(|error| StorageError::Internal(error.to_string()))?
    }

    async fn load(&self) -> Result<Option<Conversation>, StorageError> {
        let connection = Arc::clone(&self.connection);

        tokio::task::spawn_blocking(move || {
            let connection = connection
                .lock()
                .map_err(|_| StorageError::Internal("connection lock poisoned".into()))?;

            let data: Option<String> = connection
                .query_row("SELECT data FROM conversations WHERE id = 1", [], |row| row.get(0))
                .optional()
                .map_err(database)?;

            data.map(|data| serde_json::from_str(&data).map_err(StorageError::Corrupt)).transpose()
        })
        .await
        .map_err(|error| StorageError::Internal(error.to_string()))?
    }
}
