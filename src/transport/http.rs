use async_trait::async_trait;
use reqwest::Client;
use url::Url;

use super::{Capabilities, Message, Protocol, Response, Transport, TransportError};

pub struct HttpTransport {
    client: Client,
}

impl Default for HttpTransport {
    fn default() -> Self {
        Self { client: Client::new() }
    }
}

impl HttpTransport {
    pub fn new() -> Self {
        Self::default()
    }
}

#[async_trait]
impl Transport for HttpTransport {
    async fn discover(&self, target: &Url) -> Result<Capabilities, TransportError> {
        let response = self
            .client
            .get(target.clone())
            .send()
            .await
            .map_err(|_| TransportError::Unreachable)?;

        if response.status().is_success() {
            Ok(Capabilities { protocols: vec![Protocol::Http] })
        } else {
            Err(TransportError::Unreachable)
        }
    }

    async fn send(&self, _target: &Url, _message: Message) -> Result<Response, TransportError> {
        Err(TransportError::UnsupportedProtocol)
    }
}
