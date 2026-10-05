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

#[tokio::test]
async fn evasive_answer_is_retried_with_followup_then_accepted() {
    let questionnaire: Questionnaire = serde_yaml::from_str(
        r#"
questions:
  - id: company
    question: "What is your company name?"
    followup: "Please answer directly: what is your company name? No meetings."
    required: true
    type: string
    max_followups: 2
    reject_if_contains: ["meeting", "schedule a call"]
"#,
    )
    .unwrap();

    let transport = MockTransport::new(["Let's set up a MEETING first", "Acme"]);
    let store = Arc::new(SqliteStore::memory().unwrap());

    let engine = run(questionnaire, "http://mock.local", &transport, store).await.unwrap();

    let sent = transport.sent();
    assert_eq!(sent[0].text, "What is your company name?");
    assert!(sent[1].text.starts_with("Please answer directly"));
    assert_eq!(sent[1].id, "chatterg-2");
    assert_eq!(engine.conversation().answers[0].attempts.len(), 2);
    assert_eq!(engine.conversation().state, State::Complete);
}

#[tokio::test]
async fn persistent_evasion_stops_after_max_followups_plus_one_asks() {
    let questionnaire: Questionnaire = serde_yaml::from_str(
        r#"
questions:
  - id: company
    question: "Name?"
    required: true
    type: string
    max_followups: 2
    on_failure: continue
    reject_if_contains: ["meeting"]
  - id: city
    question: "City?"
    required: true
    type: string
"#,
    )
    .unwrap();

    let transport = MockTransport::new(["meeting?", "meeting!", "meeting please", "Mumbai"]);
    let store = Arc::new(SqliteStore::memory().unwrap());

    let engine = run(questionnaire, "http://mock.local", &transport, store).await.unwrap();

    // 1 initial ask + 2 follow-ups = 3 asks for `company`, then moved on to `city`.
    let texts: Vec<_> = transport.sent().into_iter().map(|m| m.text).collect();
    assert_eq!(texts, ["Name?", "Name?", "Name?", "City?"]);
    assert_eq!(engine.conversation().state, State::Complete);
}

// ---- agent stops answering: cooldown and resume --------------------------

mod cooldown {
    use std::{
        collections::VecDeque,
        sync::{Arc, Mutex},
        time::Duration,
    };

    use async_trait::async_trait;
    use chatterg::{
        application::{ApplicationError, Progress, RunOptions, run_with},
        domain::{Questionnaire, State},
        storage::sqlite::SqliteStore,
        transport::{Capabilities, Message, Protocol, Response, Transport, TransportError},
    };
    use url::Url;

    /// Replays a script of outcomes, recording every message it receives.
    struct Scripted {
        outcomes: Mutex<VecDeque<Result<String, TransportError>>>,
        received: Mutex<Vec<Message>>,
    }

    impl Scripted {
        fn new(outcomes: Vec<Result<String, TransportError>>) -> Self {
            Self { outcomes: Mutex::new(outcomes.into()), received: Mutex::new(Vec::new()) }
        }

        fn received(&self) -> Vec<Message> {
            self.received.lock().unwrap().clone()
        }
    }

    #[async_trait]
    impl Transport for Scripted {
        async fn discover(&self, target: &Url) -> Result<Capabilities, TransportError> {
            Ok(Capabilities { protocols: vec![Protocol::A2a], endpoint: target.clone() })
        }

        async fn send(&self, _: &Url, message: Message) -> Result<Response, TransportError> {
            self.received.lock().unwrap().push(message);

            self.outcomes
                .lock()
                .unwrap()
                .pop_front()
                .expect("script exhausted")
                .map(|text| Response { text })
        }
    }

    fn one_question() -> Questionnaire {
        serde_yaml::from_str(
            "questions:\n  - {id: q001, question: \"What is a zeolite?\", required: true, type: text}\n",
        )
        .unwrap()
    }

    fn fast(max_waits: usize) -> (RunOptions, Arc<Mutex<Vec<Progress>>>) {
        let events = Arc::new(Mutex::new(Vec::new()));
        let sink = Arc::clone(&events);

        let options = RunOptions {
            cooldown: Duration::from_millis(5),
            max_waits,
            on_progress: Some(Arc::new(move |p: &Progress| sink.lock().unwrap().push(p.clone()))),
            ..RunOptions::default()
        };

        (options, events)
    }

    fn waits(events: &Mutex<Vec<Progress>>) -> usize {
        events.lock().unwrap().iter().filter(|e| matches!(e, Progress::Waiting { .. })).count()
    }

    async fn run(
        script: Vec<Result<String, TransportError>>,
        options: &RunOptions,
    ) -> (Result<chatterg::domain::Engine, ApplicationError>, Scripted) {
        let transport = Scripted::new(script);
        let store = Arc::new(SqliteStore::memory().unwrap());
        let result =
            run_with(one_question(), "http://mock.local", &transport, store, options).await;
        (result, transport)
    }

    #[tokio::test]
    async fn http_429_pauses_then_resends_the_same_message() {
        let (options, events) = fast(5);

        let (result, transport) = run(
            vec![
                Err(TransportError::Http { status: 429 }),
                Err(TransportError::Http { status: 503 }),
                Ok("A crystalline aluminosilicate with uniform pores.".into()),
            ],
            &options,
        )
        .await;

        let engine = result.unwrap();
        assert_eq!(engine.conversation().state, State::Complete);
        assert_eq!(waits(&events), 2);

        // Undelivered message is re-sent with the same id, and counted once.
        let ids: Vec<_> = transport.received().into_iter().map(|m| m.id).collect();
        assert_eq!(ids, ["chatterg-1", "chatterg-1", "chatterg-1"]);
        assert_eq!(engine.conversation().messages_sent, 1);
        assert_eq!(engine.conversation().answers[0].attempts.len(), 1);
    }

    #[tokio::test]
    async fn network_failure_and_timeout_pause_and_resume() {
        let (options, events) = fast(5);

        let (result, _) = run(
            vec![
                Err(TransportError::Network("connection reset".into())),
                Err(TransportError::Unreachable),
                Ok("Answer".into()),
            ],
            &options,
        )
        .await;

        assert!(result.is_ok());
        assert_eq!(waits(&events), 2);
    }

    #[tokio::test]
    async fn rate_limit_notice_in_reply_is_not_recorded_as_an_answer() {
        let (options, events) = fast(5);

        let (result, _) = run(
            vec![
                Ok("You have exceeded the rate limit. Please try again later.".into()),
                Ok("A microporous crystalline material.".into()),
            ],
            &options,
        )
        .await;

        let engine = result.unwrap();
        assert_eq!(waits(&events), 1);
        assert_eq!(engine.conversation().answers.len(), 1);
        assert_eq!(engine.conversation().answers[0].attempts.len(), 1);
        assert_eq!(
            engine.conversation().answers[0].attempts[0].response,
            "A microporous crystalline material."
        );
    }

    #[tokio::test]
    async fn json_rpc_rate_limit_error_pauses() {
        let (options, events) = fast(5);

        let (result, _) = run(
            vec![
                Err(TransportError::Protocol {
                    code: -32000,
                    message: "Rate limit exceeded".into(),
                }),
                Ok("Answer".into()),
            ],
            &options,
        )
        .await;

        assert!(result.is_ok());
        assert_eq!(waits(&events), 1);
    }

    #[tokio::test]
    async fn long_real_answer_mentioning_rate_limiting_is_accepted() {
        let (options, events) = fast(5);
        let answer = format!(
            "Diffusion through the micropores is usually the rate limiting step. {}",
            "Zeolite pores are comparable to molecular dimensions. ".repeat(10)
        );

        let (result, _) = run(vec![Ok(answer.clone())], &options).await;

        let engine = result.unwrap();
        assert_eq!(waits(&events), 0);
        assert_eq!(engine.conversation().answers[0].attempts[0].response, answer);
    }

    #[tokio::test]
    async fn gives_up_after_max_waits() {
        let (options, events) = fast(2);

        let (result, transport) = run(
            vec![
                Err(TransportError::Http { status: 429 }),
                Err(TransportError::Http { status: 429 }),
                Err(TransportError::Http { status: 429 }),
            ],
            &options,
        )
        .await;

        assert!(matches!(result, Err(ApplicationError::AgentUnavailable { waits: 2, .. })));
        assert_eq!(waits(&events), 2);
        assert_eq!(transport.received().len(), 3);
    }

    #[tokio::test]
    async fn real_errors_fail_immediately_without_waiting() {
        let (options, events) = fast(5);

        let (result, _) = run(vec![Err(TransportError::Http { status: 404 })], &options).await;

        assert!(matches!(
            result,
            Err(ApplicationError::Transport(TransportError::Http { status: 404 }))
        ));
        assert_eq!(waits(&events), 0);
    }

    #[tokio::test]
    async fn progress_reports_question_numbers() {
        let (options, events) = fast(5);

        run(vec![Ok("Answer".into())], &options).await.0.unwrap();

        let events = events.lock().unwrap();
        assert!(matches!(&events[0], Progress::Asking { number: 1, total: 1, attempt: 1, .. }));
    }
}
