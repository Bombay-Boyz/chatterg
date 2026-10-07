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

    #[error("storage internal error: {0}")]
    Internal(String),
}

#[async_trait]
pub trait Store: Send + Sync {
    async fn save(&self, conversation: &Conversation) -> Result<(), StorageError>;

    async fn load(&self) -> Result<Option<Conversation>, StorageError>;
}

pub mod migrate;
pub mod sqlite;
