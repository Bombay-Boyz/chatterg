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

    #[error("unsupported protocol")]
    UnsupportedProtocol,

    #[error("transport error: {0}")]
    Other(String),
}

#[async_trait]
pub trait Transport: Send + Sync {
    async fn discover(&self, target: &Url) -> Result<Capabilities, TransportError>;

    async fn send(&self, target: &Url, message: Message) -> Result<Response, TransportError>;
}

pub mod a2a;
pub mod http;
pub mod mock;
