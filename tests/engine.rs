use chatterg::domain::{Engine, EngineState, FailurePolicy, QuestionId, Submission};

fn questionnaire() -> chatterg::domain::Questionnaire {
    serde_yaml::from_str(
        r#"
questions:
  - id: name
    question: "What is your name?"
    required: true
    type: string
    max_followups: 2
    on_failure: abort

  - id: age
    question: "What is your age?"
    required: true
    type: integer
    max_followups: 2
    on_failure: abort
"#,
    )
    .unwrap()
}

fn policy_questionnaire(policy: &str) -> chatterg::domain::Questionnaire {
    serde_yaml::from_str(&format!(
        r#"
questions:
  - id: name
    question: "What is your name?"
    required: true
    type: string
    max_followups: 0
    on_failure: {policy}

  - id: age
    question: "What is your age?"
    required: true
    type: integer
    max_followups: 0
    on_failure: abort
"#
    ))
    .unwrap()
}

#[test]
fn engine_starts_with_first_question() {
    let engine = Engine::new(questionnaire());

    match engine.start() {
        EngineState::Asking(question) => {
            assert_eq!(question.id.as_str(), "name");
        }
        _ => panic!("expected first question"),
    }
}

#[test]
fn accepted_answer_advances() {
    let engine = Engine::new(questionnaire());

    let Submission::Next(engine, question) = engine.submit("Sn".into()) else {
        panic!("expected next question");
    };

    assert_eq!(question.id.as_str(), "age");
    assert_eq!(engine.conversation().answer(&QuestionId::new("name")).unwrap().attempts.len(), 1);
}

#[test]
fn invalid_answer_causes_retry() {
    let engine = Engine::new(questionnaire());

    let Submission::Retry(engine, question) = engine.submit("".into()) else {
        panic!("expected retry");
    };

    assert_eq!(question.id.as_str(), "name");
    assert_eq!(engine.conversation().answer(&QuestionId::new("name")).unwrap().attempts.len(), 1);
}

#[test]
fn abort_policy_aborts() {
    let engine = Engine::new(policy_questionnaire("abort"));

    let Submission::Aborted(_) = engine.submit("".into()) else {
        panic!("expected abort");
    };
}

#[test]
fn skip_policy_advances() {
    let engine = Engine::new(policy_questionnaire("skip"));

    let Submission::Next(_, question) = engine.submit("".into()) else {
        panic!("expected next question");
    };

    assert_eq!(question.id.as_str(), "age");
}

#[test]
fn continue_policy_advances() {
    let engine = Engine::new(policy_questionnaire("continue"));

    let Submission::Next(_, question) = engine.submit("".into()) else {
        panic!("expected next question");
    };

    assert_eq!(question.id.as_str(), "age");
}

#[test]
fn unknown_policy_records_unknown() {
    let engine = Engine::new(policy_questionnaire("unknown"));

    let Submission::Unknown(_) = engine.submit("".into()) else {
        panic!("expected unknown");
    };
}

#[test]
fn last_accepted_answer_completes() {
    let engine = Engine::new(questionnaire());

    let Submission::Next(engine, _) = engine.submit("Sn".into()) else {
        panic!("expected second question");
    };

    let Submission::Complete(_) = engine.submit("42".into()) else {
        panic!("expected completion");
    };
}

#[test]
fn failure_policy_defaults_to_abort() {
    let questionnaire: chatterg::domain::Questionnaire = serde_yaml::from_str(
        r#"
questions:
  - id: name
    question: "What is your name?"
    required: true
    type: string
"#,
    )
    .unwrap();

    assert_eq!(questionnaire.questions[0].on_failure, FailurePolicy::Abort);
}

// ---- position / resume -------------------------------------------------

use chatterg::domain::{Conversation, DomainError, State};

#[test]
fn fresh_conversation_starts_at_position_zero() {
    let engine = Engine::new(questionnaire());
    assert_eq!(engine.position(), 0);
    assert_eq!(engine.conversation().position, 0);
    assert_eq!(engine.conversation().messages_sent, 0);
}

#[test]
fn answering_advances_position_and_message_count() {
    let engine = Engine::new(questionnaire());

    let Submission::Next(engine, _) = engine.submit("Sn".into()) else {
        panic!("expected next question");
    };

    assert_eq!(engine.position(), 1);
    assert_eq!(engine.conversation().messages_sent, 1);
}

#[test]
fn retry_keeps_position_but_counts_message() {
    let Submission::Retry(engine, _) = Engine::new(questionnaire()).submit("".into()) else {
        panic!("expected retry");
    };

    assert_eq!(engine.position(), 0);
    assert_eq!(engine.conversation().messages_sent, 1);
}

#[test]
fn position_is_not_answer_count_when_question_is_retried() {
    let Submission::Retry(engine, _) = Engine::new(questionnaire()).submit("".into()) else {
        panic!("expected retry");
    };
    let Submission::Retry(engine, _) = engine.submit("".into()) else {
        panic!("expected retry");
    };

    // two attempts recorded for one answer record, still on question 0
    assert_eq!(engine.position(), 0);
    assert_eq!(engine.conversation().answers.len(), 1);
}

#[test]
fn resume_continues_at_persisted_position() {
    let Submission::Next(engine, _) = Engine::new(questionnaire()).submit("Sn".into()) else {
        panic!("expected next question");
    };

    let json = serde_json::to_string(engine.conversation()).unwrap();
    let stored: Conversation = serde_json::from_str(&json).unwrap();
    assert_eq!(stored.position, 1);

    let resumed = Engine::resume(questionnaire(), stored).unwrap();

    match resumed.start() {
        EngineState::Asking(question) => assert_eq!(question.id.as_str(), "age"),
        other => panic!("expected age question, got {other:?}"),
    }
}

#[test]
fn completed_conversation_stays_completed_after_reload() {
    let Submission::Next(engine, _) = Engine::new(questionnaire()).submit("Sn".into()) else {
        panic!("expected next");
    };
    let Submission::Complete(engine) = engine.submit("42".into()) else {
        panic!("expected completion");
    };

    assert_eq!(engine.conversation().state, State::Complete);

    let json = serde_json::to_string(engine.conversation()).unwrap();
    let resumed = Engine::resume(questionnaire(), serde_json::from_str(&json).unwrap()).unwrap();

    assert_eq!(resumed.start(), EngineState::Complete);
    assert_eq!(resumed.position(), 2);
}

#[test]
fn aborted_conversation_stays_aborted_after_reload() {
    let Submission::Aborted(engine) = Engine::new(policy_questionnaire("abort")).submit("".into())
    else {
        panic!("expected abort");
    };

    let json = serde_json::to_string(engine.conversation()).unwrap();
    let resumed =
        Engine::resume(policy_questionnaire("abort"), serde_json::from_str(&json).unwrap())
            .unwrap();

    assert_eq!(resumed.start(), EngineState::Aborted);
}

#[test]
fn resume_rejects_position_past_end() {
    let mut conversation = Conversation::new();
    conversation.position = 99;

    assert!(matches!(
        Engine::resume(questionnaire(), conversation),
        Err(DomainError::InvalidPosition { position: 99, len: 2 })
    ));
}
