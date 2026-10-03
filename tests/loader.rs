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
