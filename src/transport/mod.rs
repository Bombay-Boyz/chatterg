use async_trait::async_trait;
use thiserror::Error;
use url::Url;

#[derive(Debug, Clone)]
pub struct Capabilities {
    pub protocols: Vec<Protocol>,
    pub endpoint: Url,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Protocol {
    A2a,
    Http,
    JsonRpc,
}

#[derive(Debug, Clone)]
pub struct Message {
    /// Unique per message within a conversation.
    pub id: String,
    pub text: String,
}

#[derive(Debug, Clone)]
pub struct Response {
    pub text: String,
}

#[derive(Debug, Error)]
pub enum TransportError {
    #[error("target is unreachable")]
    Unreachable,

    #[error("network failure: {0}")]
    Network(#[source] Box<dyn std::error::Error + Send + Sync>),

    #[error("HTTP request failed with status {status}")]
    Http { status: u16 },

    #[error("agent returned a protocol error ({code}): {message}")]
    Protocol { code: i64, message: String },

    #[error("unsupported protocol")]
    UnsupportedProtocol,

    #[error("malformed agent card: {0}")]
    MalformedCard(String),

    #[error("malformed agent response: {0}")]
    MalformedResponse(String),

    #[error("transport internal error: {0}")]
    Internal(String),
}

#[async_trait]
pub trait Transport: Send + Sync {
    async fn discover(&self, target: &Url) -> Result<Capabilities, TransportError>;

    async fn send(&self, target: &Url, message: Message) -> Result<Response, TransportError>;
}

pub mod a2a;
pub mod http;
pub mod mock;
