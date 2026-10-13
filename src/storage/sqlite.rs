use std::{
    fs::{File, OpenOptions},
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
    time::Duration,
};

use async_trait::async_trait;
use fs2::FileExt;
use rusqlite::{Connection, ErrorCode, OpenFlags, OptionalExtension, params};

use crate::domain::Conversation;

use super::{
    StorageError, Store,
    backups::{self, BackupPolicy, Kind},
    migrate::migrate,
};

const SCHEMA: &str = "
    CREATE TABLE IF NOT EXISTS conversations (
        id INTEGER PRIMARY KEY CHECK (id = 1),
        data TEXT NOT NULL
    );
";

const BUSY_TIMEOUT: Duration = Duration::from_secs(5);

/// Automatic snapshots: where they go and how often.
struct AutoBackups {
    store_path: PathBuf,
    policy: BackupPolicy,
    /// The position a snapshot was last taken at, so retries of one question do
    /// not produce a snapshot each.
    last_position: Mutex<Option<usize>>,
}

impl AutoBackups {
    /// Called right after a successful save. Returns a warning if a due snapshot
    /// could not be written; that never stops the run.
    fn after_save(&self, connection: &Connection, position: usize) -> Option<String> {
        if position == 0 || position % self.policy.every != 0 {
            return None;
        }

        let mut last = self.last_position.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        if *last == Some(position) {
            return None;
        }
        *last = Some(position);

        let path = backups::unused_path(&self.store_path, Kind::Auto, chrono::Utc::now(), position);

        let result = path
            .parent()
            .map_or(Ok(()), std::fs::create_dir_all)
            .map_err(|error| error.to_string())
            .and_then(|()| {
                connection
                    .execute("VACUUM INTO ?1", params![path.to_string_lossy()])
                    .map(|_| ())
                    .map_err(|error| error.to_string())
            });

        match result {
            Ok(()) => {
                backups::prune(&self.store_path, self.policy.keep);
                None
            }
            Err(error) => Some(format!("could not write a backup to {}: {error}", path.display())),
        }
    }
}

pub struct SqliteStore {
    connection: Arc<Mutex<Connection>>,

    backups: Option<Arc<AutoBackups>>,
    warnings: Arc<Mutex<Vec<String>>>,

    /// Held for as long as the store lives; the OS releases it when this drops
    /// (or the process dies), so a crash never leaves a stale lock.
    _lock: Option<File>,
}

fn database(error: rusqlite::Error) -> StorageError {
    StorageError::Database(Box::new(error))
}

/// A notebook that is not a database at all, or whose pages are broken.
fn damage(path: &Path, error: &rusqlite::Error) -> Option<StorageError> {
    let broken = matches!(
        error.sqlite_error_code(),
        Some(ErrorCode::NotADatabase | ErrorCode::DatabaseCorrupt)
    );

    broken.then(|| StorageError::Damaged {
        path: path.display().to_string(),
        detail: error.to_string(),
    })
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
            .map_err(|error| damage(path, &error).unwrap_or_else(|| open_error(Box::new(error))))?;

        // Every save reaches the disk before the next question is asked, so a power
        // cut loses at most the answer that was being written.
        connection
            .execute_batch("PRAGMA synchronous = FULL;")
            .map_err(|error| open_error(Box::new(error)))?;

        let health: String = connection
            .query_row("PRAGMA quick_check(1)", [], |row| row.get(0))
            .map_err(|error| damage(path, &error).unwrap_or_else(|| open_error(Box::new(error))))?;

        if health != "ok" {
            return Err(StorageError::Damaged { path: path.display().to_string(), detail: health });
        }

        Self::initialise(connection, Some(lock), |error| {
            damage(path, &error).unwrap_or_else(|| open_error(Box::new(error)))
        })
    }

    /// Opens an existing notebook for reading only. It takes no lock and never
    /// creates or changes anything, so it is safe to use while a run is in
    /// progress (the database uses write-ahead logging, so readers do not block).
    pub fn open_read_only(path: impl AsRef<Path>) -> Result<Self, StorageError> {
        let path = path.as_ref();
        let open_error = |source: Box<dyn std::error::Error + Send + Sync>| StorageError::Open {
            path: path.display().to_string(),
            source,
        };

        if !path.is_file() {
            return Err(open_error("no such notebook".into()));
        }

        let connection = Connection::open_with_flags(path, OpenFlags::SQLITE_OPEN_READ_ONLY)
            .map_err(|error| open_error(Box::new(error)))?;
        connection.busy_timeout(BUSY_TIMEOUT).map_err(database)?;

        let has_table: bool = connection
            .query_row(
                "SELECT COUNT(*) > 0 FROM sqlite_master WHERE type = 'table' AND name = 'conversations'",
                [],
                |row| row.get(0),
            )
            .map_err(|error| damage(path, &error).unwrap_or_else(|| open_error(Box::new(error))))?;

        if !has_table {
            return Err(open_error("this file is not a chatterg notebook".into()));
        }

        Ok(Self::from_connection(connection, None))
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

        Ok(Self::from_connection(connection, lock))
    }

    fn from_connection(connection: Connection, lock: Option<File>) -> Self {
        Self {
            connection: Arc::new(Mutex::new(connection)),
            backups: None,
            warnings: Arc::new(Mutex::new(Vec::new())),
            _lock: lock,
        }
    }

    /// Turns on automatic snapshots next to the notebook at `store_path`.
    pub fn with_backups(mut self, store_path: impl Into<PathBuf>, policy: BackupPolicy) -> Self {
        if policy.is_on() {
            self.backups = Some(Arc::new(AutoBackups {
                store_path: store_path.into(),
                policy,
                last_position: Mutex::new(None),
            }));
        }

        self
    }

    /// Writes a snapshot of the notebook to `path` (a complete, consistent copy). Works
    /// on a notebook opened read-only, so it is safe during a run.
    pub fn snapshot_to(&self, path: &Path) -> Result<(), StorageError> {
        if let Some(parent) = path.parent().filter(|parent| !parent.as_os_str().is_empty()) {
            std::fs::create_dir_all(parent)
                .map_err(|error| StorageError::Database(Box::new(error)))?;
        }

        let connection = self
            .connection
            .lock()
            .map_err(|_| StorageError::Internal("connection lock poisoned".into()))?;

        connection.execute("VACUUM INTO ?1", params![path.to_string_lossy()]).map_err(database)?;

        Ok(())
    }

    /// Whether a conversation is stored at all.
    pub fn has_conversation(&self) -> Result<bool, StorageError> {
        let connection = self
            .connection
            .lock()
            .map_err(|_| StorageError::Internal("connection lock poisoned".into()))?;

        connection
            .query_row("SELECT COUNT(*) > 0 FROM conversations", [], |row| row.get(0))
            .map_err(database)
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
        let position = conversation.position;
        let connection = Arc::clone(&self.connection);
        let backups = self.backups.clone();
        let warnings = Arc::clone(&self.warnings);

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

            if let Some(warning) =
                backups.as_ref().and_then(|backups| backups.after_save(&connection, position))
            {
                warnings.lock().unwrap_or_else(std::sync::PoisonError::into_inner).push(warning);
            }

            Ok(())
        })
        .await
        .map_err(|error| StorageError::Internal(error.to_string()))?
    }

    fn take_warnings(&self) -> Vec<String> {
        std::mem::take(
            &mut *self.warnings.lock().unwrap_or_else(std::sync::PoisonError::into_inner),
        )
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
