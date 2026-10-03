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

    #[error("storage internal error: {0}")]
    Internal(String),
}

#[async_trait]
pub trait Store: Send + Sync {
    async fn save(&self, conversation: &Conversation) -> Result<(), StorageError>;

    async fn load(&self) -> Result<Option<Conversation>, StorageError>;
}

pub mod sqlite;
