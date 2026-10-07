use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use url::Url;

use super::{Capabilities, Message, Protocol, Response, Transport, TransportError};

#[derive(Clone)]
pub struct MockTransport {
    responses: Arc<Mutex<Vec<String>>>,
    sent: Arc<Mutex<Vec<Message>>>,
}

impl MockTransport {
    pub fn new(responses: impl IntoIterator<Item = impl Into<String>>) -> Self {
        Self {
            responses: Arc::new(Mutex::new(responses.into_iter().map(Into::into).collect())),
            sent: Arc::new(Mutex::new(Vec::new())),
        }
    }

    /// Messages that were successfully exchanged, in order.
    pub fn sent(&self) -> Vec<Message> {
        self.sent.lock().map(|sent| sent.clone()).unwrap_or_default()
    }
}

#[async_trait]
impl Transport for MockTransport {
    async fn discover(&self, target: &Url) -> Result<Capabilities, TransportError> {
        Ok(Capabilities {
            protocols: vec![Protocol::Http],
            endpoint: target.clone(),
            agent_name: None,
        })
    }

    async fn send(&self, _target: &Url, message: Message) -> Result<Response, TransportError> {
        let mut responses = self
            .responses
            .lock()
            .map_err(|_| TransportError::Internal("mock state poisoned".into()))?;

        if responses.is_empty() {
            return Err(TransportError::Internal("mock response queue exhausted".into()));
        }

        self.sent
            .lock()
            .map_err(|_| TransportError::Internal("mock state poisoned".into()))?
            .push(message);

        Ok(Response { text: responses.remove(0) })
    }
}
