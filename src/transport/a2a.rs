use std::time::Duration;

use async_trait::async_trait;
use reqwest::Client;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use url::Url;

use super::{Capabilities, Message, Protocol, Response, Transport, TransportError};

const A2A_VERSION: &str = "1.0";
const SEND_METHOD: &str = "message/send";

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
    role: &'static str,
    parts: Vec<Part>,
}

#[derive(Debug, Serialize)]
struct Part {
    kind: &'static str,
    text: String,
}

#[derive(Debug, Deserialize)]
struct JsonRpcResponse {
    result: Option<Value>,
    error: Option<JsonRpcError>,
}

#[derive(Debug, Deserialize)]
struct JsonRpcError {
    code: i64,
    message: String,
}

pub struct A2aTransport {
    client: Client,
}

/// A bot that never answers must not hang the run forever.
pub const DEFAULT_TIMEOUT: Duration = Duration::from_secs(120);

impl Default for A2aTransport {
    fn default() -> Self {
        Self::with_timeout(DEFAULT_TIMEOUT)
    }
}

fn network(error: reqwest::Error) -> TransportError {
    TransportError::Network(Box::new(error))
}

fn check_status(response: reqwest::Response) -> Result<reqwest::Response, TransportError> {
    let status = response.status();

    if status.is_success() {
        Ok(response)
    } else {
        Err(TransportError::Http { status: status.as_u16() })
    }
}

impl A2aTransport {
    pub fn new() -> Self {
        Self::default()
    }

    /// Per-request timeout; a timeout surfaces as a (transient) network error.
    pub fn with_timeout(timeout: Duration) -> Self {
        let client = Client::builder().timeout(timeout).build().unwrap_or_default();
        Self { client }
    }

    /// `<target path>/.well-known/agent-card.json`, preserving any path prefix.
    pub fn agent_card_url(target: &Url) -> Result<Url, TransportError> {
        if !matches!(target.scheme(), "http" | "https") {
            return Err(TransportError::UnsupportedProtocol);
        }

        let mut url = target.clone();
        url.set_query(None);
        url.set_fragment(None);

        let path = url.path().trim_end_matches('/').to_owned();
        url.set_path(&format!("{path}/.well-known/agent-card.json"));

        Ok(url)
    }

    pub async fn agent_card(&self, target: &Url) -> Result<AgentCard, TransportError> {
        let url = Self::agent_card_url(target)?;

        let response = self
            .client
            .get(url)
            .header("A2A-Version", A2A_VERSION)
            .send()
            .await
            .map_err(network)?;

        let body = check_status(response)?.text().await.map_err(network)?;

        serde_json::from_str(&body)
            .map_err(|error| TransportError::MalformedCard(error.to_string()))
    }

    pub fn interface(card: &AgentCard) -> Result<&AgentInterface, TransportError> {
        card.supported_interfaces
            .iter()
            .find(|interface| {
                interface.protocol_binding.eq_ignore_ascii_case("JSONRPC")
                    && (interface.protocol_version.is_empty()
                        || interface.protocol_version == A2A_VERSION)
            })
            .ok_or(TransportError::UnsupportedProtocol)
    }

    /// Extracts the agent's text from a `message/send` JSON-RPC `result`.
    ///
    /// Supported shapes:
    /// - `result.message`
    /// - `result.task.status.message`
    /// - `result` is itself a message (has `parts`)
    /// - `result` is itself a task (has `status`)
    pub fn extract_text(result: &Value) -> Result<String, TransportError> {
        let message = if let Some(message) = result.get("message") {
            Some(message)
        } else if let Some(task) = result.get("task") {
            task.pointer("/status/message")
        } else if result.get("parts").is_some() {
            Some(result)
        } else if result.get("status").is_some() {
            result.pointer("/status/message")
        } else {
            return Err(TransportError::MalformedResponse(
                "result is neither a message nor a task".into(),
            ));
        };

        let message = message
            .filter(|message| !message.is_null())
            .ok_or_else(|| TransportError::MalformedResponse("task contains no message".into()))?;

        let parts = message
            .get("parts")
            .and_then(Value::as_array)
            .ok_or_else(|| TransportError::MalformedResponse("message has no parts".into()))?;

        Ok(parts
            .iter()
            .filter_map(|part| part.get("text").and_then(Value::as_str))
            .collect::<Vec<_>>()
            .join("\n"))
    }

    async fn send_message(
        &self,
        endpoint: &Url,
        message: Message,
    ) -> Result<Response, TransportError> {
        let request = JsonRpcRequest {
            jsonrpc: "2.0",
            id: 1,
            method: SEND_METHOD,
            params: SendMessageParams {
                message: A2aMessage {
                    message_id: message.id,
                    role: "user",
                    parts: vec![Part { kind: "text", text: message.text }],
                },
            },
        };

        let response = self
            .client
            .post(endpoint.clone())
            .header("Content-Type", "application/json")
            .header("A2A-Version", A2A_VERSION)
            .json(&request)
            .send()
            .await
            .map_err(network)?;

        let body = check_status(response)?.text().await.map_err(network)?;

        let response: JsonRpcResponse = serde_json::from_str(&body)
            .map_err(|error| TransportError::MalformedResponse(error.to_string()))?;

        if let Some(error) = response.error {
            return Err(TransportError::Protocol { code: error.code, message: error.message });
        }

        let result = response.result.ok_or_else(|| {
            TransportError::MalformedResponse("response contained neither result nor error".into())
        })?;

        Ok(Response { text: Self::extract_text(&result)? })
    }
}

#[async_trait]
impl Transport for A2aTransport {
    async fn discover(&self, target: &Url) -> Result<Capabilities, TransportError> {
        let card = self.agent_card(target).await?;
        let interface = Self::interface(&card)?;

        let endpoint = Url::parse(&interface.url).map_err(|error| {
            TransportError::MalformedCard(format!("invalid interface url: {error}"))
        })?;

        if !matches!(endpoint.scheme(), "http" | "https") {
            return Err(TransportError::UnsupportedProtocol);
        }

        Ok(Capabilities { protocols: vec![Protocol::A2a], endpoint })
    }

    async fn send(&self, target: &Url, message: Message) -> Result<Response, TransportError> {
        self.send_message(target, message).await
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    fn card_url(target: &str) -> String {
        A2aTransport::agent_card_url(&Url::parse(target).unwrap()).unwrap().to_string()
    }

    #[test]
    fn card_url_for_root() {
        assert_eq!(
            card_url("https://example.com"),
            "https://example.com/.well-known/agent-card.json"
        );
        assert_eq!(
            card_url("https://example.com/"),
            "https://example.com/.well-known/agent-card.json"
        );
    }

    #[test]
    fn card_url_keeps_path_prefix() {
        assert_eq!(
            card_url("https://flowmarket.social/nxtbrane"),
            "https://flowmarket.social/nxtbrane/.well-known/agent-card.json"
        );
        assert_eq!(
            card_url("https://flowmarket.social/nxtbrane/"),
            "https://flowmarket.social/nxtbrane/.well-known/agent-card.json"
        );
    }

    #[test]
    fn card_url_drops_query_and_fragment() {
        assert_eq!(
            card_url("https://example.com/a?x=1#frag"),
            "https://example.com/a/.well-known/agent-card.json"
        );
    }

    #[test]
    fn card_url_rejects_unsupported_scheme() {
        let url = Url::parse("ftp://example.com/a").unwrap();
        assert!(matches!(
            A2aTransport::agent_card_url(&url),
            Err(TransportError::UnsupportedProtocol)
        ));
    }

    fn interface(binding: &str, version: &str) -> AgentInterface {
        AgentInterface {
            url: "https://example.com/rpc".into(),
            protocol_binding: binding.into(),
            protocol_version: version.into(),
        }
    }

    fn card(interfaces: Vec<AgentInterface>) -> AgentCard {
        AgentCard { name: "t".into(), description: None, supported_interfaces: interfaces }
    }

    #[test]
    fn selects_jsonrpc_interface() {
        let card = card(vec![interface("GRPC", "1.0"), interface("jsonrpc", "1.0")]);
        assert_eq!(A2aTransport::interface(&card).unwrap().protocol_binding, "jsonrpc");
    }

    #[test]
    fn rejects_card_without_supported_interface() {
        let card = card(vec![interface("GRPC", "1.0"), interface("JSONRPC", "2.0")]);
        assert!(matches!(A2aTransport::interface(&card), Err(TransportError::UnsupportedProtocol)));
    }

    #[test]
    fn extracts_direct_message() {
        let result = json!({"message": {"parts": [{"kind": "text", "text": "hi"}]}});
        assert_eq!(A2aTransport::extract_text(&result).unwrap(), "hi");
    }

    #[test]
    fn extracts_flat_message() {
        let result = json!({"kind": "message", "parts": [{"kind": "text", "text": "hi"}]});
        assert_eq!(A2aTransport::extract_text(&result).unwrap(), "hi");
    }

    #[test]
    fn extracts_wrapped_task() {
        let result = json!({"task": {"status": {"message": {"parts": [{"text": "done"}]}}}});
        assert_eq!(A2aTransport::extract_text(&result).unwrap(), "done");
    }

    #[test]
    fn extracts_flat_task() {
        let result = json!({"kind": "task", "status": {"state": "completed",
            "message": {"parts": [{"kind": "text", "text": "done"}]}}});
        assert_eq!(A2aTransport::extract_text(&result).unwrap(), "done");
    }

    #[test]
    fn joins_multiple_text_parts_and_skips_non_text() {
        let result = json!({"message": {"parts": [
            {"kind": "text", "text": "a"},
            {"kind": "data", "data": {}},
            {"kind": "text", "text": "b"}
        ]}});
        assert_eq!(A2aTransport::extract_text(&result).unwrap(), "a\nb");
    }

    #[test]
    fn empty_parts_yield_empty_text() {
        let result = json!({"message": {"parts": []}});
        assert_eq!(A2aTransport::extract_text(&result).unwrap(), "");
    }

    #[test]
    fn task_without_message_is_malformed() {
        let result = json!({"kind": "task", "status": {"state": "working"}});
        assert!(matches!(
            A2aTransport::extract_text(&result),
            Err(TransportError::MalformedResponse(_))
        ));
    }

    #[test]
    fn unknown_result_shape_is_malformed() {
        let result = json!({"something": "else"});
        assert!(matches!(
            A2aTransport::extract_text(&result),
            Err(TransportError::MalformedResponse(_))
        ));
    }
}
