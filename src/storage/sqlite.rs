use std::{
    fs::{File, OpenOptions},
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
    time::Duration,
};

use async_trait::async_trait;
use fs2::FileExt;
use rusqlite::{Connection, OptionalExtension, params};

use crate::domain::Conversation;

use super::{StorageError, Store, migrate::migrate};

const SCHEMA: &str = "
    CREATE TABLE IF NOT EXISTS conversations (
        id INTEGER PRIMARY KEY CHECK (id = 1),
        data TEXT NOT NULL
    );
";

const BUSY_TIMEOUT: Duration = Duration::from_secs(5);

pub struct SqliteStore {
    connection: Arc<Mutex<Connection>>,

    /// Held for as long as the store lives; the OS releases it when this drops
    /// (or the process dies), so a crash never leaves a stale lock.
    _lock: Option<File>,
}

fn database(error: rusqlite::Error) -> StorageError {
    StorageError::Database(Box::new(error))
}

fn lock_path(path: &Path) -> PathBuf {
    let mut name = path.as_os_str().to_owned();
    name.push(".lock");
    PathBuf::from(name)
}

impl SqliteStore {
    /// Opens (creating if needed) the database at `path` and takes an exclusive
    /// lock so two chatterg processes cannot share one run.
    pub fn open(path: impl AsRef<Path>) -> Result<Self, StorageError> {
        let path = path.as_ref();
        let open_error = |source: Box<dyn std::error::Error + Send + Sync>| StorageError::Open {
            path: path.display().to_string(),
            source,
        };

        let lock = OpenOptions::new()
            .create(true)
            .truncate(false)
            .write(true)
            .open(lock_path(path))
            .map_err(|error| open_error(Box::new(error)))?;

        lock.try_lock_exclusive().map_err(|error| {
            if error.kind() == std::io::ErrorKind::WouldBlock
                || error.raw_os_error() == fs2::lock_contended_error().raw_os_error()
            {
                StorageError::Locked { path: path.display().to_string() }
            } else {
                open_error(Box::new(error))
            }
        })?;

        let connection = Connection::open(path).map_err(|error| open_error(Box::new(error)))?;

        // Write-ahead logging: readers never block the writer and a crash mid-write
        // leaves the previous state intact.
        connection
            .query_row("PRAGMA journal_mode = WAL", [], |row| row.get::<_, String>(0))
            .map_err(|error| open_error(Box::new(error)))?;
        connection
            .execute_batch("PRAGMA synchronous = NORMAL;")
            .map_err(|error| open_error(Box::new(error)))?;

        Self::initialise(connection, Some(lock), |error| open_error(Box::new(error)))
    }

    pub fn memory() -> Result<Self, StorageError> {
        let connection = Connection::open_in_memory().map_err(database)?;
        Self::initialise(connection, None, database)
    }

    fn initialise(
        connection: Connection,
        lock: Option<File>,
        on_error: impl FnOnce(rusqlite::Error) -> StorageError,
    ) -> Result<Self, StorageError> {
        connection.busy_timeout(BUSY_TIMEOUT).map_err(database)?;
        connection.execute_batch(SCHEMA).map_err(on_error)?;

        Ok(Self { connection: Arc::new(Mutex::new(connection)), _lock: lock })
    }

    /// Copies the database to `backup` and then forgets the stored conversation, so
    /// the next run starts fresh. Returns `false` (and copies nothing) if there was
    /// nothing stored.
    pub fn archive_and_reset(&self, backup: &Path) -> Result<bool, StorageError> {
        let connection = self
            .connection
            .lock()
            .map_err(|_| StorageError::Internal("connection lock poisoned".into()))?;

        let stored: i64 = connection
            .query_row("SELECT COUNT(*) FROM conversations", [], |row| row.get(0))
            .map_err(database)?;

        if stored == 0 {
            return Ok(false);
        }

        connection
            .execute("VACUUM INTO ?1", params![backup.to_string_lossy()])
            .map_err(database)?;
        connection.execute("DELETE FROM conversations", []).map_err(database)?;

        Ok(true)
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

            // A single statement is atomic: a crash leaves the old or the new row.
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

            data.map(|data| {
                let value = serde_json::from_str(&data).map_err(StorageError::Corrupt)?;
                serde_json::from_value(migrate(value)?).map_err(StorageError::Corrupt)
            })
            .transpose()
        })
        .await
        .map_err(|error| StorageError::Internal(error.to_string()))?
    }
}
