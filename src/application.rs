use std::sync::Arc;

use url::Url;

use crate::{
    domain::{Engine, EngineState, Submission},
    storage::Store,
    transport::{Message, Transport},
};

pub async fn run<T, S>(
    mut engine: Engine,
    target: &str,
    transport: &T,
    store: Arc<S>,
) -> Result<Engine, Box<dyn std::error::Error>>
where
    T: Transport,
    S: Store + 'static,
{
    let target = Url::parse(target)?;

    loop {
        let question = match engine.start() {
            EngineState::Asking(question) => question,
            EngineState::Complete => {
                store.save(engine.conversation()).await?;
                return Ok(engine);
            }
            EngineState::Ready => {
                return Err("engine is not ready".into());
            }
        };

        transport.discover(&target).await?;

        let message = Message { text: question.question.clone() };

        let response = transport.send(message).await?;

        match engine.submit(response.text) {
            Submission::Next(next, _) => {
                engine = next;
                store.save(engine.conversation()).await?;
            }

            Submission::Retry(next, _) => {
                engine = next;
                store.save(engine.conversation()).await?;
            }

            Submission::Aborted(next) => {
                store.save(next.conversation()).await?;
                return Ok(next);
            }

            Submission::Unknown(next) => {
                store.save(next.conversation()).await?;
                return Ok(next);
            }

            Submission::Complete(next) => {
                store.save(next.conversation()).await?;
                return Ok(next);
            }
        }
    }
}
