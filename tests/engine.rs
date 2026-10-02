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

    let Submission::Complete = engine.submit("42".into()) else {
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
