use async_trait::async_trait;

use crate::domain::Conversation;

use thiserror::Error;

#[derive(Debug, Error)]
pub enum StorageError {
    #[error("storage error: {0}")]
    Other(String),
}

#[async_trait]
pub trait Store: Send + Sync {
    async fn save(&self, conversation: &Conversation) -> Result<(), StorageError>;

    async fn load(&self) -> Result<Option<Conversation>, StorageError>;
}

pub mod sqlite;
