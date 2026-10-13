use async_trait::async_trait;
use thiserror::Error;

use crate::domain::Conversation;

type Source = Box<dyn std::error::Error + Send + Sync>;

#[derive(Debug, Error)]
pub enum StorageError {
    #[error("cannot open database {path}: {source}")]
    Open {
        path: String,
        #[source]
        source: Source,
    },

    #[error("database operation failed: {0}")]
    Database(#[source] Source),

    #[error("cannot serialize conversation: {0}")]
    Serialize(#[source] serde_json::Error),

    #[error("stored conversation is corrupt: {0}")]
    Corrupt(#[source] serde_json::Error),

    #[error(
        "stored conversation uses format version {found}, but this chatterg only understands \
         up to version {supported}; upgrade chatterg"
    )]
    UnsupportedVersion { found: u32, supported: u32 },

    #[error(
        "another chatterg is already using {path}; wait for it to finish or use a different --store"
    )]
    Locked { path: String },

    #[error("the notebook {path} is damaged ({detail})")]
    Damaged { path: String, detail: String },

    #[error("storage internal error: {0}")]
    Internal(String),
}

#[async_trait]
pub trait Store: Send + Sync {
    async fn save(&self, conversation: &Conversation) -> Result<(), StorageError>;

    async fn load(&self) -> Result<Option<Conversation>, StorageError>;

    /// Problems the store hit that did not stop it working (for example a backup
    /// that could not be written). Returned once; the run shows them to the user.
    fn take_warnings(&self) -> Vec<String> {
        Vec::new()
    }
}

pub mod backups;
pub mod migrate;
pub mod sqlite;
