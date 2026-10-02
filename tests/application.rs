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
async fn a2a_discovery_reads_agent_card() {
    use chatterg::transport::{Transport, a2a::A2aTransport};
    use url::Url;

    let mut server = mockito::Server::new_async().await;

    let _mock = server
        .mock("GET", "/.well-known/agent-card.json")
        .with_status(200)
        .with_header("content-type", "application/json")
        .with_body(format!(
            r#"{{
                "name": "Test Agent",
                "description": "Test",
                "url": "{}/a2a"
            }}"#,
            server.url()
        ))
        .create_async()
        .await;

    let transport = A2aTransport::new();
    let target = Url::parse(&server.url()).unwrap();

    let capabilities = transport.discover(&target).await.unwrap();

    assert_eq!(capabilities.protocols, vec![chatterg::transport::Protocol::A2a]);
}
