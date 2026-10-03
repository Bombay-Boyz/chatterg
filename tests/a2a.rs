use chatterg::transport::{Message, Protocol, Transport, TransportError, a2a::A2aTransport};
use mockito::{Matcher, Server, ServerGuard};
use serde_json::json;
use url::Url;

fn card_body(endpoint: &str, binding: &str, version: &str) -> String {
    json!({
        "name": "Test Agent",
        "supportedInterfaces": [
            {"url": endpoint, "protocolBinding": binding, "protocolVersion": version}
        ]
    })
    .to_string()
}

async fn discover_with(
    server: &mut ServerGuard,
    path: &str,
    body: String,
    target: &str,
) -> Result<chatterg::transport::Capabilities, TransportError> {
    let _mock = server
        .mock("GET", path)
        .with_status(200)
        .with_header("content-type", "application/json")
        .with_body(body)
        .create_async()
        .await;

    A2aTransport::new().discover(&Url::parse(target).unwrap()).await
}

// ---- discovery ---------------------------------------------------------

#[tokio::test]
async fn discovers_from_root_url() {
    let mut server = Server::new_async().await;
    let endpoint = format!("{}/rpc", server.url());
    let target = server.url();

    let capabilities = discover_with(
        &mut server,
        "/.well-known/agent-card.json",
        card_body(&endpoint, "JSONRPC", "1.0"),
        &target,
    )
    .await
    .unwrap();

    assert_eq!(capabilities.protocols, vec![Protocol::A2a]);
    assert_eq!(capabilities.endpoint.as_str(), endpoint);
}

#[tokio::test]
async fn discovers_from_path_based_url_without_touching_root() {
    let mut server = Server::new_async().await;
    let endpoint = format!("{}/rpc", server.url());
    let target = format!("{}/nxtbrane", server.url());

    let root = server
        .mock("GET", "/.well-known/agent-card.json")
        .with_status(500)
        .expect(0)
        .create_async()
        .await;

    let capabilities = discover_with(
        &mut server,
        "/nxtbrane/.well-known/agent-card.json",
        card_body(&endpoint, "JSONRPC", "1.0"),
        &target,
    )
    .await
    .unwrap();

    assert_eq!(capabilities.endpoint.as_str(), endpoint);
    root.assert_async().await;
}

#[tokio::test]
async fn discovery_sends_a2a_version_header() {
    let mut server = Server::new_async().await;
    let endpoint = format!("{}/rpc", server.url());

    let mock = server
        .mock("GET", "/.well-known/agent-card.json")
        .match_header("a2a-version", "1.0")
        .with_status(200)
        .with_body(card_body(&endpoint, "JSONRPC", "1.0"))
        .create_async()
        .await;

    A2aTransport::new().discover(&Url::parse(&server.url()).unwrap()).await.unwrap();
    mock.assert_async().await;
}

#[tokio::test]
async fn discovery_rejects_malformed_card() {
    let mut server = Server::new_async().await;
    let target = server.url();

    let error =
        discover_with(&mut server, "/.well-known/agent-card.json", "{\"nope\":1}".into(), &target)
            .await
            .unwrap_err();

    assert!(matches!(error, TransportError::MalformedCard(_)));
}

#[tokio::test]
async fn discovery_rejects_unsupported_binding() {
    let mut server = Server::new_async().await;
    let target = server.url();
    let body = card_body("https://example.com/grpc", "GRPC", "1.0");

    let error = discover_with(&mut server, "/.well-known/agent-card.json", body, &target)
        .await
        .unwrap_err();

    assert!(matches!(error, TransportError::UnsupportedProtocol));
}

#[tokio::test]
async fn discovery_rejects_malformed_interface_url() {
    let mut server = Server::new_async().await;
    let target = server.url();
    let body = card_body("not a url", "JSONRPC", "1.0");

    let error = discover_with(&mut server, "/.well-known/agent-card.json", body, &target)
        .await
        .unwrap_err();

    assert!(matches!(error, TransportError::MalformedCard(_)));
}

#[tokio::test]
async fn discovery_rejects_unsupported_scheme() {
    let mut server = Server::new_async().await;
    let target = server.url();
    let body = card_body("ftp://example.com/rpc", "JSONRPC", "1.0");

    let error = discover_with(&mut server, "/.well-known/agent-card.json", body, &target)
        .await
        .unwrap_err();

    assert!(matches!(error, TransportError::UnsupportedProtocol));
}

#[tokio::test]
async fn discovery_reports_http_failure() {
    let mut server = Server::new_async().await;
    let _mock =
        server.mock("GET", "/.well-known/agent-card.json").with_status(404).create_async().await;

    let error =
        A2aTransport::new().discover(&Url::parse(&server.url()).unwrap()).await.unwrap_err();

    assert!(matches!(error, TransportError::Http { status: 404 }));
}

#[tokio::test]
async fn discovery_rejects_non_http_target() {
    let error = A2aTransport::new()
        .discover(&Url::parse("ftp://example.com/agent").unwrap())
        .await
        .unwrap_err();

    assert!(matches!(error, TransportError::UnsupportedProtocol));
}

// ---- message/send ------------------------------------------------------

fn message(id: &str, text: &str) -> Message {
    Message { id: id.into(), text: text.into() }
}

async fn send_with(status: usize, body: &str) -> Result<String, TransportError> {
    let mut server = Server::new_async().await;
    let _mock = server
        .mock("POST", "/rpc")
        .with_status(status)
        .with_header("content-type", "application/json")
        .with_body(body)
        .create_async()
        .await;

    let endpoint = Url::parse(&format!("{}/rpc", server.url())).unwrap();

    A2aTransport::new().send(&endpoint, message("chatterg-1", "Q")).await.map(|r| r.text)
}

#[tokio::test]
async fn send_emits_exact_request() {
    let mut server = Server::new_async().await;

    let mock = server
        .mock("POST", "/rpc")
        .match_header("content-type", "application/json")
        .match_header("a2a-version", "1.0")
        .match_body(Matcher::Json(json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": "message/send",
            "params": {"message": {
                "messageId": "chatterg-7",
                "role": "user",
                "parts": [{"kind": "text", "text": "What is your name?"}]
            }}
        })))
        .with_status(200)
        .with_body(
            json!({"jsonrpc": "2.0", "id": 1,
            "result": {"message": {"parts": [{"kind": "text", "text": "ok"}]}}})
            .to_string(),
        )
        .create_async()
        .await;

    let endpoint = Url::parse(&format!("{}/rpc", server.url())).unwrap();
    let response = A2aTransport::new()
        .send(&endpoint, message("chatterg-7", "What is your name?"))
        .await
        .unwrap();

    assert_eq!(response.text, "ok");
    mock.assert_async().await;
}

fn rpc(result: serde_json::Value) -> String {
    json!({"jsonrpc": "2.0", "id": 1, "result": result}).to_string()
}

#[tokio::test]
async fn send_reads_direct_message() {
    let body = rpc(json!({"kind": "message", "role": "agent", "messageId": "a",
        "parts": [{"kind": "text", "text": "direct"}]}));
    assert_eq!(send_with(200, &body).await.unwrap(), "direct");
}

#[tokio::test]
async fn send_reads_nxtbrane_style_completed_task() {
    let body = rpc(json!({"kind": "task", "id": "t", "contextId": "c",
        "status": {"state": "completed", "message": {"kind": "message", "role": "agent",
            "parts": [{"kind": "text", "text": "I am Test Agent."}]}}}));
    assert_eq!(send_with(200, &body).await.unwrap(), "I am Test Agent.");
}

#[tokio::test]
async fn send_reads_wrapped_task() {
    let body = rpc(json!({"task": {"id": "t", "status": {"state": "completed",
        "message": {"parts": [{"kind": "text", "text": "wrapped"}]}}}}));
    assert_eq!(send_with(200, &body).await.unwrap(), "wrapped");
}

#[tokio::test]
async fn send_joins_multiple_text_parts() {
    let body = rpc(json!({"message": {"parts": [
        {"kind": "text", "text": "one"}, {"kind": "text", "text": "two"}]}}));
    assert_eq!(send_with(200, &body).await.unwrap(), "one\ntwo");
}

#[tokio::test]
async fn send_with_empty_parts_returns_empty_text() {
    let body = rpc(json!({"message": {"parts": []}}));
    assert_eq!(send_with(200, &body).await.unwrap(), "");
}

#[tokio::test]
async fn send_task_without_message_is_malformed() {
    let body = rpc(json!({"kind": "task", "id": "t", "status": {"state": "working"}}));
    assert!(matches!(send_with(200, &body).await, Err(TransportError::MalformedResponse(_))));
}

#[tokio::test]
async fn send_reports_json_rpc_error() {
    let body = json!({"jsonrpc": "2.0", "id": 1,
        "error": {"code": -32601, "message": "Method not found"}})
    .to_string();

    match send_with(200, &body).await {
        Err(TransportError::Protocol { code, message }) => {
            assert_eq!(code, -32601);
            assert_eq!(message, "Method not found");
        }
        other => panic!("expected protocol error, got {other:?}"),
    }
}

#[tokio::test]
async fn send_reports_malformed_json() {
    assert!(matches!(send_with(200, "not json").await, Err(TransportError::MalformedResponse(_))));
}

#[tokio::test]
async fn send_reports_response_with_neither_result_nor_error() {
    let body = json!({"jsonrpc": "2.0", "id": 1}).to_string();
    assert!(matches!(send_with(200, &body).await, Err(TransportError::MalformedResponse(_))));
}

#[tokio::test]
async fn send_reports_http_4xx() {
    assert!(matches!(send_with(400, "{}").await, Err(TransportError::Http { status: 400 })));
}

#[tokio::test]
async fn send_reports_http_5xx() {
    assert!(matches!(send_with(503, "{}").await, Err(TransportError::Http { status: 503 })));
}

#[tokio::test]
async fn send_reports_network_failure() {
    let endpoint = Url::parse("http://127.0.0.1:1/rpc").unwrap();
    let result = A2aTransport::new().send(&endpoint, message("chatterg-1", "Q")).await;
    assert!(matches!(result, Err(TransportError::Network(_))));
}
