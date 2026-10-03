use std::sync::Arc;

use thiserror::Error;
use url::Url;

use crate::{
    domain::{DomainError, Engine, EngineState, Questionnaire, Submission},
    storage::{StorageError, Store},
    transport::{Message, Transport, TransportError},
};

#[derive(Debug, Error)]
pub enum ApplicationError {
    #[error("invalid target URL: {0}")]
    InvalidTarget(#[from] url::ParseError),

    #[error(transparent)]
    Domain(#[from] DomainError),

    #[error(transparent)]
    Transport(#[from] TransportError),

    #[error(transparent)]
    Storage(#[from] StorageError),
}

/// Runs (or resumes) the questionnaire.
///
/// If the store already holds a conversation the engine is rebuilt from it and
/// continues from the persisted position; otherwise a new run starts.
pub async fn run<T, S>(
    questionnaire: Questionnaire,
    target: &str,
    transport: &T,
    store: Arc<S>,
) -> Result<Engine, ApplicationError>
where
    T: Transport,
    S: Store + 'static,
{
    let target = Url::parse(target)?;

    let mut engine = match store.load().await? {
        Some(conversation) => Engine::resume(questionnaire, conversation)?,
        None => Engine::new(questionnaire),
    };

    if !matches!(engine.start(), EngineState::Asking(_)) {
        return Ok(engine);
    }

    let endpoint = transport.discover(&target).await?.endpoint;

    loop {
        let question = match engine.start() {
            EngineState::Asking(question) => question,
            EngineState::Ready | EngineState::Complete | EngineState::Aborted => {
                store.save(engine.conversation()).await?;
                return Ok(engine);
            }
        };

        let message = Message {
            id: format!("chatterg-{}", engine.conversation().next_message_number()),
            text: question.question.clone(),
        };

        let response = transport.send(&endpoint, message).await?;

        match engine.submit(response.text) {
            Submission::Next(next, _) | Submission::Retry(next, _) => {
                store.save(next.conversation()).await?;
                engine = next;
            }

            Submission::Aborted(next) | Submission::Unknown(next) | Submission::Complete(next) => {
                store.save(next.conversation()).await?;
                return Ok(next);
            }
        }
    }
}
