use async_trait::async_trait;
use reqwest::Client;
use serde::Deserialize;
use url::Url;

use super::{Capabilities, Message, Protocol, Response, Transport, TransportError};

#[derive(Debug, Clone, Deserialize)]
pub struct AgentCard {
    pub name: String,
    pub description: Option<String>,
    pub url: String,
    #[serde(default)]
    pub protocol_version: Option<String>,
}

pub struct A2aTransport {
    client: Client,
}

impl Default for A2aTransport {
    fn default() -> Self {
        Self { client: Client::new() }
    }
}

impl A2aTransport {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn agent_card_url(target: &Url) -> Result<Url, TransportError> {
        target
            .join(".well-known/agent-card.json")
            .map_err(|error| TransportError::Other(error.to_string()))
    }

    pub async fn agent_card(&self, target: &Url) -> Result<AgentCard, TransportError> {
        let url = Self::agent_card_url(target)?;

        self.client
            .get(url)
            .send()
            .await
            .map_err(|error| TransportError::Other(error.to_string()))?
            .error_for_status()
            .map_err(|error| TransportError::Other(error.to_string()))?
            .json::<AgentCard>()
            .await
            .map_err(|error| TransportError::Other(error.to_string()))
    }
}

#[async_trait]
impl Transport for A2aTransport {
    async fn discover(&self, target: &Url) -> Result<Capabilities, TransportError> {
        let card = self.agent_card(target).await?;

        let endpoint =
            Url::parse(&card.url).map_err(|error| TransportError::Other(error.to_string()))?;

        if endpoint.scheme() != "http" && endpoint.scheme() != "https" {
            return Err(TransportError::UnsupportedProtocol);
        }

        Ok(Capabilities { protocols: vec![Protocol::A2a], endpoint })
    }

    async fn send(&self, _target: &Url, _message: Message) -> Result<Response, TransportError> {
        Err(TransportError::UnsupportedProtocol)
    }
}
