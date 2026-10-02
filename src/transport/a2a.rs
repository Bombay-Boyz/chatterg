use async_trait::async_trait;
use url::Url;

use super::{Capabilities, Message, Protocol, Response, Transport, TransportError};

pub struct A2aTransport;

impl Default for A2aTransport {
    fn default() -> Self {
        Self
    }
}

impl A2aTransport {
    pub fn new() -> Self {
        Self
    }

    pub fn agent_card_url(target: &Url) -> Result<Url, TransportError> {
        target
            .join("/.well-known/agent-card.json")
            .map_err(|error| TransportError::Other(error.to_string()))
    }
}

#[async_trait]
impl Transport for A2aTransport {
    async fn discover(&self, _target: &Url) -> Result<Capabilities, TransportError> {
        Ok(Capabilities { protocols: vec![Protocol::A2a] })
    }

    async fn send(&self, _message: Message) -> Result<Response, TransportError> {
        Err(TransportError::UnsupportedProtocol)
    }
}
