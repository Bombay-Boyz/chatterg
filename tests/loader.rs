use chatterg::domain::{DomainError, Questionnaire};

fn write(dir: &tempfile::TempDir, name: &str, body: &str) -> std::path::PathBuf {
    let path = dir.path().join(name);
    std::fs::write(&path, body).unwrap();
    path
}

#[test]
fn missing_file_is_a_useful_error() {
    let error = Questionnaire::from_path("/definitely/not/here.yaml").unwrap_err();

    assert!(matches!(error, DomainError::QuestionnaireRead { .. }));
    assert!(error.to_string().contains("/definitely/not/here.yaml"));
}

#[test]
fn invalid_yaml_is_a_useful_error() {
    let dir = tempfile::tempdir().unwrap();
    let path = write(&dir, "bad.yaml", "questions: [ this is : not valid");

    let error = Questionnaire::from_path(&path).unwrap_err();

    assert!(matches!(error, DomainError::QuestionnaireParse { .. }));
    assert!(error.to_string().contains("bad.yaml"));
}

#[test]
fn empty_questionnaire_is_rejected() {
    let dir = tempfile::tempdir().unwrap();
    let path = write(&dir, "empty.yaml", "questions: []\n");

    assert!(matches!(Questionnaire::from_path(&path), Err(DomainError::EmptyQuestionnaire)));
}

#[test]
fn duplicate_ids_are_rejected() {
    let dir = tempfile::tempdir().unwrap();
    let path = write(
        &dir,
        "dup.yaml",
        "questions:\n  - {id: a, question: Q1, required: true, type: string}\n  - {id: a, question: Q2, required: true, type: string}\n",
    );

    assert!(matches!(
        Questionnaire::from_path(&path),
        Err(DomainError::DuplicateQuestionId(id)) if id == "a"
    ));
}

#[test]
fn valid_questionnaire_loads_deterministically() {
    let first = Questionnaire::from_path("questions.yaml").unwrap();
    let second = Questionnaire::from_path("questions.yaml").unwrap();

    assert_eq!(first, second);
    assert_eq!(first.questions.len(), 5);
    assert_eq!(first.questions[0].id.as_str(), "company_name");
}

// ---- plain-text / bare-string questionnaires ---------------------------

use chatterg::domain::{AnswerType, FailurePolicy, QuestionDefaults};

#[test]
fn txt_file_one_question_per_line() {
    let dir = tempfile::tempdir().unwrap();
    let path = write(
        &dir,
        "q.txt",
        "# a comment\nWhat is a zeolite?\n\n- \"What is a zeolite membrane's permeance?\"\n3. How is selectivity calculated?\n   \n'Why 1,000-hour tests?'\n",
    );

    let questionnaire = Questionnaire::from_path(&path).unwrap();
    let texts: Vec<_> = questionnaire.questions.iter().map(|q| q.question.as_str()).collect();

    assert_eq!(
        texts,
        [
            "What is a zeolite?",
            "What is a zeolite membrane's permeance?",
            "How is selectivity calculated?",
            "Why 1,000-hour tests?"
        ]
    );
    assert_eq!(questionnaire.questions[0].id.as_str(), "q001");
    assert_eq!(questionnaire.questions[3].id.as_str(), "q004");
}

#[test]
fn bare_strings_get_default_settings() {
    let dir = tempfile::tempdir().unwrap();
    let path = write(&dir, "q.yaml", "questions:\n  - \"What is Nxtbrane?\"\n");

    let question = &Questionnaire::from_path(&path).unwrap().questions[0];

    assert_eq!(question.answer_type, AnswerType::Text);
    assert!(question.required);
    assert_eq!(question.max_followups, 2);
    assert_eq!(question.on_failure, FailurePolicy::Continue);
    assert!(question.reject_if_contains.iter().any(|p| p == "meeting"));
    assert!(question.followup.as_deref().unwrap().ends_with("What is Nxtbrane?"));
}

#[test]
fn bare_top_level_list_is_accepted() {
    let dir = tempfile::tempdir().unwrap();
    let path = write(&dir, "q.yml", "- \"One?\"\n- \"Two?\"\n");

    assert_eq!(Questionnaire::from_path(&path).unwrap().questions.len(), 2);
}

#[test]
fn bare_strings_and_full_objects_can_be_mixed() {
    let dir = tempfile::tempdir().unwrap();
    let path = write(
        &dir,
        "q.yaml",
        "questions:\n  - \"Plain?\"\n  - {id: size, question: How many?, required: true, type: integer}\n",
    );

    let questions = Questionnaire::from_path(&path).unwrap().questions;

    assert_eq!(questions[0].id.as_str(), "q001");
    assert_eq!(questions[1].id.as_str(), "size");
    assert_eq!(questions[1].answer_type, AnswerType::Integer);
}

#[test]
fn custom_defaults_are_applied() {
    let dir = tempfile::tempdir().unwrap();
    let path = write(&dir, "q.txt", "Question?\n");
    let defaults = QuestionDefaults {
        max_followups: 5,
        reject_if_contains: vec!["no comment".into()],
        ..QuestionDefaults::default()
    };

    let question = &Questionnaire::from_path_with(&path, &defaults).unwrap().questions[0];

    assert_eq!(question.max_followups, 5);
    assert_eq!(question.reject_if_contains, ["no comment"]);
}

#[test]
fn invalid_full_question_names_its_position() {
    let dir = tempfile::tempdir().unwrap();
    let path = write(&dir, "q.yaml", "questions:\n  - \"ok?\"\n  - {id: x, question: no type}\n");

    let error = Questionnaire::from_path(&path).unwrap_err();

    assert!(matches!(error, DomainError::InvalidQuestion { index: 2, .. }));
    assert!(error.to_string().contains("#2"));
}

#[test]
fn empty_txt_file_is_rejected() {
    let dir = tempfile::tempdir().unwrap();
    let path = write(&dir, "q.txt", "# only a comment\n\n");

    assert!(matches!(Questionnaire::from_path(&path), Err(DomainError::EmptyQuestionnaire)));
}

#[test]
fn hundred_questions_get_unique_ids() {
    let dir = tempfile::tempdir().unwrap();
    let body: String = (1..=100).map(|n| format!("Question number {n}?\n")).collect();
    let path = write(&dir, "q.txt", &body);

    let questions = Questionnaire::from_path(&path).unwrap().questions;

    assert_eq!(questions.len(), 100);
    assert_eq!(questions[99].id.as_str(), "q100");
}
