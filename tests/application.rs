use std::sync::Arc;

use chatterg::{
    application::{ApplicationError, run},
    domain::{EngineState, Questionnaire, State},
    storage::{Store, sqlite::SqliteStore},
    transport::{TransportError, mock::MockTransport},
};

const ANSWERS: [&str; 5] = ["Acme", "Mumbai", "Software", "India, Singapore", "commercial"];

fn questionnaire() -> Questionnaire {
    serde_yaml::from_str(include_str!("../questions.yaml")).unwrap()
}

#[tokio::test]
async fn mock_bot_completes_questionnaire() {
    let transport = MockTransport::new(ANSWERS);
    let store = Arc::new(SqliteStore::memory().unwrap());

    let engine =
        run(questionnaire(), "http://mock.local", &transport, Arc::clone(&store)).await.unwrap();

    assert_eq!(engine.conversation().answers.len(), 5);
    assert_eq!(engine.conversation().position, 5);
    assert_eq!(engine.conversation().state, State::Complete);

    let stored = store.load().await.unwrap().unwrap();
    assert_eq!(stored, *engine.conversation());
}

#[tokio::test]
async fn message_ids_are_sequential_and_unique() {
    let transport = MockTransport::new(ANSWERS);
    let store = Arc::new(SqliteStore::memory().unwrap());

    run(questionnaire(), "http://mock.local", &transport, store).await.unwrap();

    let ids: Vec<_> = transport.sent().into_iter().map(|message| message.id).collect();
    assert_eq!(ids, ["chatterg-1", "chatterg-2", "chatterg-3", "chatterg-4", "chatterg-5"]);
}

#[tokio::test]
async fn interrupted_run_resumes_and_matches_uninterrupted_run() {
    // Uninterrupted reference run.
    let reference_store = Arc::new(SqliteStore::memory().unwrap());
    let reference = run(
        questionnaire(),
        "http://mock.local",
        &MockTransport::new(ANSWERS),
        Arc::clone(&reference_store),
    )
    .await
    .unwrap();

    // Interrupted run: a fresh process (new transport, reopened store) per answer.
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("chatterg.db");
    let mut ids = Vec::new();
    let mut asked = Vec::new();
    let mut last = None;

    for (index, answer) in ANSWERS.iter().enumerate() {
        let transport = MockTransport::new([*answer]);
        let store = Arc::new(SqliteStore::open(&path).unwrap());

        let result = run(questionnaire(), "http://mock.local", &transport, store).await;

        for message in transport.sent() {
            ids.push(message.id);
            asked.push(message.text);
        }

        if index + 1 < ANSWERS.len() {
            // The next question is attempted but the "process" dies (queue exhausted).
            assert!(matches!(
                result,
                Err(ApplicationError::Transport(TransportError::Internal(_)))
            ));

            let stored = SqliteStore::open(&path).unwrap().load().await.unwrap().unwrap();
            assert_eq!(stored.position, index + 1);
        } else {
            last = Some(result.unwrap());
        }
    }

    let resumed = last.unwrap();

    assert_eq!(resumed.conversation(), reference.conversation());
    assert_eq!(ids, ["chatterg-1", "chatterg-2", "chatterg-3", "chatterg-4", "chatterg-5"]);

    // No question repeated.
    let mut unique = asked.clone();
    unique.dedup();
    assert_eq!(unique, asked);

    let stored = SqliteStore::open(&path).unwrap().load().await.unwrap().unwrap();
    assert_eq!(stored, *reference.conversation());
}

#[tokio::test]
async fn completed_conversation_is_not_asked_again() {
    let store = Arc::new(SqliteStore::memory().unwrap());

    run(questionnaire(), "http://mock.local", &MockTransport::new(ANSWERS), Arc::clone(&store))
        .await
        .unwrap();

    let second = MockTransport::new(Vec::<String>::new());
    let engine = run(questionnaire(), "http://mock.local", &second, store).await.unwrap();

    assert_eq!(engine.start(), EngineState::Complete);
    assert!(second.sent().is_empty());
}

#[tokio::test]
async fn aborted_conversation_is_not_resumed() {
    let store = Arc::new(SqliteStore::memory().unwrap());

    // Required integer-like enum answer is invalid and default policy is abort.
    let questionnaire: Questionnaire = serde_yaml::from_str(
        "questions:\n  - {id: a, question: Q, required: true, type: enum, values: [x]}\n",
    )
    .unwrap();

    let first = run(
        questionnaire.clone(),
        "http://mock.local",
        &MockTransport::new(["nope"]),
        Arc::clone(&store),
    )
    .await
    .unwrap();
    assert_eq!(first.start(), EngineState::Aborted);

    let second = MockTransport::new(["x"]);
    let engine = run(questionnaire, "http://mock.local", &second, store).await.unwrap();

    assert_eq!(engine.start(), EngineState::Aborted);
    assert!(second.sent().is_empty());
}

#[tokio::test]
async fn invalid_target_is_reported() {
    let store = Arc::new(SqliteStore::memory().unwrap());
    let result = run(questionnaire(), "not a url", &MockTransport::new(ANSWERS), store).await;

    assert!(matches!(result, Err(ApplicationError::InvalidTarget(_))));
}

#[tokio::test]
async fn resuming_with_a_different_questionnaire_is_rejected() {
    let store = Arc::new(SqliteStore::memory().unwrap());

    run(questionnaire(), "http://mock.local", &MockTransport::new(["Acme"]), Arc::clone(&store))
        .await
        .unwrap_err();

    let other: Questionnaire = serde_yaml::from_str(
        "questions:\n  - {id: other, question: Q, required: true, type: string}\n",
    )
    .unwrap();

    let result = run(other, "http://mock.local", &MockTransport::new(["x"]), store).await;
    assert!(matches!(result, Err(ApplicationError::Domain(_))));
}
