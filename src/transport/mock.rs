use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use url::Url;

use super::{Capabilities, Message, Protocol, Response, Transport, TransportError};

#[derive(Clone)]
pub struct MockTransport {
    responses: Arc<Mutex<Vec<String>>>,
}

impl MockTransport {
    pub fn new(responses: impl IntoIterator<Item = impl Into<String>>) -> Self {
        Self { responses: Arc::new(Mutex::new(responses.into_iter().map(Into::into).collect())) }
    }
}

#[async_trait]
impl Transport for MockTransport {
    async fn discover(&self, target: &Url) -> Result<Capabilities, TransportError> {
        Ok(Capabilities { protocols: vec![Protocol::Http], endpoint: target.clone() })
    }

    async fn send(&self, _target: &Url, _message: Message) -> Result<Response, TransportError> {
        let mut responses = self
            .responses
            .lock()
            .map_err(|_| TransportError::Other("mock state poisoned".into()))?;

        match responses.is_empty() {
            false => Ok(Response { text: responses.remove(0) }),
            true => Err(TransportError::Other("mock response queue exhausted".into())),
        }
    }
}
