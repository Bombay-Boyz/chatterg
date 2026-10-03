use async_trait::async_trait;
use reqwest::Client;
use serde::{Deserialize, Serialize};
use url::Url;

use super::{Capabilities, Message, Protocol, Response, Transport, TransportError};

#[derive(Debug, Clone, Deserialize)]
pub struct AgentCard {
    pub name: String,

    #[serde(default)]
    pub description: Option<String>,

    #[serde(rename = "supportedInterfaces")]
    pub supported_interfaces: Vec<AgentInterface>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct AgentInterface {
    pub url: String,

    #[serde(rename = "protocolBinding", alias = "protocol")]
    pub protocol_binding: String,

    #[serde(default, rename = "protocolVersion")]
    pub protocol_version: String,
}

#[derive(Debug, Serialize)]
struct JsonRpcRequest<T> {
    jsonrpc: &'static str,
    id: u64,
    method: &'static str,
    params: T,
}

#[derive(Debug, Serialize)]
struct SendMessageParams {
    message: A2aMessage,
}

#[derive(Debug, Serialize)]
struct A2aMessage {
    #[serde(rename = "messageId")]
    message_id: String,
    role: String,
    parts: Vec<Part>,
}

#[derive(Debug, Serialize)]
struct Part {
    kind: &'static str,
    text: String,
}

#[derive(Debug, Deserialize)]
struct JsonRpcResponse {
    result: Option<SendMessageResult>,
    error: Option<JsonRpcError>,
}

#[derive(Debug, Deserialize)]
#[serde(untagged)]
enum SendMessageResult {
    Message(A2aResponseMessage),
    Task(Task),
}

#[derive(Debug, Deserialize)]
struct A2aResponseMessage {
    parts: Vec<ResponsePart>,
}

#[derive(Debug, Deserialize)]
struct ResponsePart {
    #[allow(dead_code)]
    kind: Option<String>,
    text: Option<String>,
}

#[derive(Debug, Deserialize)]
struct Task {
    #[allow(dead_code)]
    id: String,

    #[allow(dead_code)]
    status: TaskStatus,
}

#[derive(Debug, Deserialize)]
struct TaskStatus {
    #[allow(dead_code)]
    state: String,

    #[allow(dead_code)]
    message: Option<A2aResponseMessage>,
}

#[derive(Debug, Deserialize)]
struct JsonRpcError {
    code: i64,
    message: String,
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
        let mut url = target.clone();
        let path = url.path().trim_end_matches('/');
        url.set_path(&format!("{path}/.well-known/agent-card.json"));
        Ok(url)
    }

    pub async fn agent_card(&self, target: &Url) -> Result<AgentCard, TransportError> {
        let url = Self::agent_card_url(target)?;

        self.client
            .get(url)
            .header("A2A-Version", "1.0")
            .send()
            .await
            .map_err(|error| TransportError::Other(error.to_string()))?
            .error_for_status()
            .map_err(|error| TransportError::Other(error.to_string()))?
            .json::<AgentCard>()
            .await
            .map_err(|error| TransportError::Other(error.to_string()))
    }

    fn interface(card: &AgentCard) -> Result<&AgentInterface, TransportError> {
        card.supported_interfaces
            .iter()
            .find(|interface| {
                interface.protocol_binding.eq_ignore_ascii_case("JSONRPC")
                    && (interface.protocol_version.is_empty()
                        || interface.protocol_version == "1.0")
            })
            .ok_or(TransportError::UnsupportedProtocol)
    }

    async fn send_message(
        &self,
        endpoint: &Url,
        message: Message,
    ) -> Result<Response, TransportError> {
        let request = JsonRpcRequest {
            jsonrpc: "2.0",
            id: 1,
            method: "message/send",
            params: SendMessageParams {
                message: A2aMessage {
                    message_id: "chatterg-1".to_string(),
                    role: "user".to_string(),
                    parts: vec![Part { kind: "text", text: message.text }],
                },
            },
        };

        let response = self
            .client
            .post(endpoint.clone())
            .header("Content-Type", "application/json")
            .header("A2A-Version", "1.0")
            .json(&request)
            .send()
            .await
            .map_err(|error| TransportError::Other(error.to_string()))?
            .error_for_status()
            .map_err(|error| TransportError::Other(error.to_string()))?
            .json::<JsonRpcResponse>()
            .await
            .map_err(|error| TransportError::Other(error.to_string()))?;

        if let Some(error) = response.error {
            return Err(TransportError::Other(format!(
                "A2A error {}: {}",
                error.code, error.message
            )));
        }

        match response.result {
            Some(SendMessageResult::Message(message)) => {
                let text = message
                    .parts
                    .into_iter()
                    .filter_map(|part| part.text)
                    .collect::<Vec<_>>()
                    .join("\n");

                Ok(Response { text })
            }

            Some(SendMessageResult::Task(task)) => task
                .status
                .message
                .map(|message| Response {
                    text: message
                        .parts
                        .into_iter()
                        .filter_map(|part| part.text)
                        .collect::<Vec<_>>()
                        .join("\n"),
                })
                .ok_or_else(|| {
                    TransportError::Other("A2A task response did not contain a message".to_string())
                }),

            None => Err(TransportError::Other(
                "A2A response contained neither result nor error".to_string(),
            )),
        }
    }
}

#[async_trait]
impl Transport for A2aTransport {
    async fn discover(&self, target: &Url) -> Result<Capabilities, TransportError> {
        let card = self.agent_card(target).await?;
        let interface = Self::interface(&card)?;

        let endpoint =
            Url::parse(&interface.url).map_err(|error| TransportError::Other(error.to_string()))?;

        if endpoint.scheme() != "http" && endpoint.scheme() != "https" {
            return Err(TransportError::UnsupportedProtocol);
        }

        Ok(Capabilities { protocols: vec![Protocol::A2a], endpoint })
    }

    async fn send(&self, target: &Url, message: Message) -> Result<Response, TransportError> {
        self.send_message(target, message).await
    }
}
