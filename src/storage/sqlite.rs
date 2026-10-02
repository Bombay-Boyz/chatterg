use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use rusqlite::{Connection, params};

use crate::domain::Conversation;

use super::{StorageError, Store};

pub struct SqliteStore {
    connection: Arc<Mutex<Connection>>,
}

impl SqliteStore {
    pub fn open(path: impl AsRef<std::path::Path>) -> Result<Self, StorageError> {
        let connection =
            Connection::open(path).map_err(|error| StorageError::Other(error.to_string()))?;

        connection
            .execute_batch(
                "
                CREATE TABLE IF NOT EXISTS conversations (
                    id INTEGER PRIMARY KEY CHECK (id = 1),
                    data TEXT NOT NULL
                );
                ",
            )
            .map_err(|error| StorageError::Other(error.to_string()))?;

        Ok(Self { connection: Arc::new(Mutex::new(connection)) })
    }

    pub fn memory() -> Result<Self, StorageError> {
        let connection =
            Connection::open_in_memory().map_err(|error| StorageError::Other(error.to_string()))?;

        connection
            .execute_batch(
                "
                CREATE TABLE conversations (
                    id INTEGER PRIMARY KEY CHECK (id = 1),
                    data TEXT NOT NULL
                );
                ",
            )
            .map_err(|error| StorageError::Other(error.to_string()))?;

        Ok(Self { connection: Arc::new(Mutex::new(connection)) })
    }
}

#[async_trait]
impl Store for SqliteStore {
    async fn save(&self, conversation: &Conversation) -> Result<(), StorageError> {
        let data = serde_json::to_string(conversation)
            .map_err(|error| StorageError::Other(error.to_string()))?;

        let connection = Arc::clone(&self.connection);

        tokio::task::spawn_blocking(move || {
            let connection =
                connection.lock().map_err(|error| StorageError::Other(error.to_string()))?;

            connection
                .execute(
                    "INSERT OR REPLACE INTO conversations (id, data) VALUES (1, ?1)",
                    params![data],
                )
                .map_err(|error| StorageError::Other(error.to_string()))?;

            Ok(())
        })
        .await
        .map_err(|error| StorageError::Other(error.to_string()))?
    }

    async fn load(&self) -> Result<Option<Conversation>, StorageError> {
        let connection = Arc::clone(&self.connection);

        tokio::task::spawn_blocking(move || {
            let connection =
                connection.lock().map_err(|error| StorageError::Other(error.to_string()))?;

            let mut statement = connection
                .prepare("SELECT data FROM conversations WHERE id = 1")
                .map_err(|error| StorageError::Other(error.to_string()))?;

            let mut rows =
                statement.query([]).map_err(|error| StorageError::Other(error.to_string()))?;

            match rows.next().map_err(|error| StorageError::Other(error.to_string()))? {
                Some(row) => {
                    let data: String =
                        row.get(0).map_err(|error| StorageError::Other(error.to_string()))?;

                    let conversation = serde_json::from_str(&data)
                        .map_err(|error| StorageError::Other(error.to_string()))?;

                    Ok(Some(conversation))
                }
                None => Ok(None),
            }
        })
        .await
        .map_err(|error| StorageError::Other(error.to_string()))?
    }
}
