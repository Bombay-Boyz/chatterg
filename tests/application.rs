use std::sync::Arc;

use chatterg::{
    application::run,
    domain::{Engine, Questionnaire},
    storage::{Store, sqlite::SqliteStore},
    transport::mock::MockTransport,
};

#[tokio::test]
async fn mock_bot_completes_questionnaire() {
    let questionnaire: Questionnaire =
        serde_yaml::from_str(include_str!("../questions.yaml")).unwrap();

    let engine = Engine::new(questionnaire);

    let transport =
        MockTransport::new(["Acme", "Mumbai", "Software", "India, Singapore", "commercial"]);

    let store = Arc::new(SqliteStore::memory().unwrap());

    let engine = run(engine, "http://mock.local", &transport, Arc::clone(&store)).await.unwrap();

    assert_eq!(engine.conversation().answers.len(), 5);

    let stored = store.load().await.unwrap().unwrap();

    assert_eq!(stored, *engine.conversation());
}

#[tokio::test]
async fn a2a_send_receives_message_response() {
    use chatterg::transport::{Message, Transport, a2a::A2aTransport};
    use url::Url;

    let mut server = mockito::Server::new_async().await;

    let _mock = server
        .mock("POST", "/a2a")
        .match_header("content-type", "application/json")
        .match_header("a2a-version", "1.0")
        .match_body(mockito::Matcher::Json(serde_json::json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": "SendMessage",
            "params": {
                "message": {
                    "messageId": "chatterg-1",
                    "role": "ROLE_USER",
                    "parts": [
                        {
                            "text": "What is your name?"
                        }
                    ]
                }
            }
        })))
        .with_status(200)
        .with_header("content-type", "application/json")
        .with_body(
            r#"{
                "jsonrpc": "2.0",
                "id": 1,
                "result": {
                    "messageId": "agent-1",
                    "role": "ROLE_AGENT",
                    "parts": [
                        {
                            "text": "I am Test Agent."
                        }
                    ]
                }
            }"#,
        )
        .create_async()
        .await;

    let transport = A2aTransport::new();
    let endpoint = Url::parse(&format!("{}/a2a", server.url())).unwrap();

    let response = transport
        .send(&endpoint, Message { text: "What is your name?".to_string() })
        .await
        .unwrap();

    assert_eq!(response.text, "I am Test Agent.");
}

#[tokio::test]
async fn a2a_discovery_reads_agent_card() {
    use chatterg::transport::{Protocol, Transport, a2a::A2aTransport};
    use url::Url;

    let mut server = mockito::Server::new_async().await;

    let endpoint = format!("{}/a2a", server.url());

    let _mock = server
        .mock("GET", "/.well-known/agent-card.json")
        .with_status(200)
        .with_header("content-type", "application/json")
        .with_body(format!(
            r#"{{
                "name": "Test Agent",
                "description": "Test",
                "supportedInterfaces": [
                    {{
                        "url": "{}",
                        "protocolBinding": "JSONRPC",
                        "protocolVersion": "1.0"
                    }}
                ]
            }}"#,
            endpoint
        ))
        .create_async()
        .await;

    let transport = A2aTransport::new();
    let target = Url::parse(&server.url()).unwrap();

    let capabilities = transport.discover(&target).await.unwrap();

    assert_eq!(capabilities.protocols, vec![Protocol::A2a]);
    assert_eq!(capabilities.endpoint.as_str(), endpoint);
}
