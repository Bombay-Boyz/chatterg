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

// ---- the server says how long to wait ---------------------------------------------------

async fn send_with_headers(
    status: usize,
    headers: &[(&str, &str)],
) -> Result<String, TransportError> {
    let mut server = Server::new_async().await;
    let mut mock = server.mock("POST", "/rpc").with_status(status).with_body("{}");
    for (name, value) in headers {
        mock = mock.with_header(*name, value);
    }
    let _mock = mock.create_async().await;

    let endpoint = Url::parse(&format!("{}/rpc", server.url())).unwrap();

    A2aTransport::new().send(&endpoint, message("chatterg-1", "Q")).await.map(|r| r.text)
}

#[tokio::test]
async fn a_429_with_retry_after_carries_the_wait() {
    match send_with_headers(429, &[("retry-after", "37")]).await {
        Err(TransportError::RetryLater { status: 429, retry_after }) => {
            assert_eq!(retry_after, std::time::Duration::from_secs(37));
        }
        other => panic!("expected RetryLater, got {other:?}"),
    }
}

#[tokio::test]
async fn a_503_with_a_ratelimit_header_carries_the_wait() {
    assert!(matches!(
        send_with_headers(503, &[("ratelimit-reset", "12")]).await,
        Err(TransportError::RetryLater { status: 503, retry_after }) if retry_after.as_secs() == 12
    ));
}

#[tokio::test]
async fn the_error_text_says_how_long_the_server_asked_for() {
    let error = send_with_headers(429, &[("retry-after", "37")]).await.unwrap_err();

    assert_eq!(
        error.to_string(),
        "HTTP request failed with status 429 (the server asks us to wait 37s)"
    );
    assert!(error.is_transient());
}

#[tokio::test]
async fn without_a_hint_a_429_stays_a_plain_http_error() {
    assert!(matches!(send_with_headers(429, &[]).await, Err(TransportError::Http { status: 429 })));
    assert!(matches!(
        send_with_headers(429, &[("retry-after", "later")]).await,
        Err(TransportError::Http { status: 429 })
    ));
}

#[tokio::test]
async fn a_hint_on_a_real_error_is_ignored() {
    // 404 and 400 are mistakes, not "try later", whatever headers come with them
    assert!(matches!(
        send_with_headers(404, &[("retry-after", "30")]).await,
        Err(TransportError::Http { status: 404 })
    ));
    assert!(matches!(
        send_with_headers(400, &[("retry-after", "30")]).await,
        Err(TransportError::Http { status: 400 })
    ));
}

#[tokio::test]
async fn discovery_also_carries_the_wait() {
    let mut server = Server::new_async().await;
    let _mock = server
        .mock("GET", "/.well-known/agent-card.json")
        .with_status(429)
        .with_header("retry-after", "5")
        .create_async()
        .await;

    let error =
        A2aTransport::new().discover(&Url::parse(&server.url()).unwrap()).await.unwrap_err();

    assert!(matches!(error, TransportError::RetryLater { status: 429, .. }));
}

// ---- the words used when the connection fails ----------------------------------------------

#[tokio::test]
async fn a_refused_connection_says_the_internet_or_the_bot_may_be_down() {
    let endpoint = Url::parse("http://127.0.0.1:1/rpc").unwrap();

    let error = A2aTransport::new().send(&endpoint, message("chatterg-1", "Q")).await.unwrap_err();

    assert!(error.is_outage());
    assert!(
        error.to_string().contains(
            "could not connect to the bot; the internet connection or the bot may be down"
        ),
        "{error}"
    );
}

#[tokio::test]
async fn a_bot_that_never_answers_is_reported_as_a_timeout() {
    // accepts the connection and then says nothing
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    std::thread::spawn(move || {
        let mut held = Vec::new();
        for stream in listener.incoming().flatten() {
            held.push(stream);
        }
    });

    let endpoint = Url::parse(&format!("http://127.0.0.1:{port}/rpc")).unwrap();
    let transport = A2aTransport::with_timeout(std::time::Duration::from_millis(200));

    let started = std::time::Instant::now();
    let error = transport.send(&endpoint, message("chatterg-1", "Q")).await.unwrap_err();

    assert!(error.is_outage());
    assert!(error.to_string().contains("did not answer in time (timed out)"), "{error}");
    assert!(started.elapsed() < std::time::Duration::from_secs(5));
}

#[tokio::test]
async fn real_errors_from_the_bot_are_not_outages() {
    for status in [400, 404] {
        let error = send_with_headers(status, &[]).await.unwrap_err();
        assert!(!error.is_outage(), "{status}");
    }
    // 503 means "reachable but busy": transient, yet not a connection outage
    let busy = send_with_headers(503, &[]).await.unwrap_err();
    assert!(busy.is_transient() && !busy.is_outage());
}
