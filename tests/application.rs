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
