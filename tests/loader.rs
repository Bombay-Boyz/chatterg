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

// ---- sections ------------------------------------------------------------------

fn sections_of(questionnaire: &Questionnaire) -> Vec<Option<&str>> {
    questionnaire.questions.iter().map(|q| q.section.as_deref()).collect()
}

#[test]
fn text_file_headings_start_sections() {
    let dir = tempfile::tempdir().unwrap();
    let path = write(
        &dir,
        "q.txt",
        "# a normal comment\nBefore any heading?\n\n## Basics\nOne?\nTwo?\n\n##   Nxtbrane  \nThree?\n\n##\nAfter the sections?\n",
    );

    let questionnaire = Questionnaire::from_path(&path).unwrap();

    assert_eq!(
        sections_of(&questionnaire),
        [None, Some("Basics"), Some("Basics"), Some("Nxtbrane"), None]
    );
    // ids keep counting straight through headings
    assert_eq!(questionnaire.questions[4].id.as_str(), "q005");
}

#[test]
fn a_single_hash_is_still_just_a_comment() {
    let dir = tempfile::tempdir().unwrap();
    let path = write(&dir, "q.txt", "# Basics\nOne?\n");

    let questionnaire = Questionnaire::from_path(&path).unwrap();

    assert_eq!(questionnaire.questions.len(), 1);
    assert_eq!(sections_of(&questionnaire), [None]);
}

#[test]
fn a_file_with_only_headings_is_empty() {
    let dir = tempfile::tempdir().unwrap();
    let path = write(&dir, "q.txt", "## Basics\n## More\n");

    assert!(matches!(Questionnaire::from_path(&path), Err(DomainError::EmptyQuestionnaire)));
}

#[test]
fn yaml_questions_can_name_their_section() {
    let dir = tempfile::tempdir().unwrap();
    let path = write(
        &dir,
        "q.yaml",
        "questions:\n  - {id: a, question: A?, required: true, type: text, section: Basics}\n  - \"Plain?\"\n",
    );

    let questionnaire = Questionnaire::from_path(&path).unwrap();

    assert_eq!(sections_of(&questionnaire), [Some("Basics"), None]);
}

#[test]
fn yaml_sections_list_groups_bare_strings_and_full_questions() {
    let dir = tempfile::tempdir().unwrap();
    let path = write(
        &dir,
        "q.yaml",
        "sections:\n  - name: Basics\n    questions:\n      - \"One?\"\n      - {id: size, question: How many?, required: true, type: integer}\n      - {id: own, question: Own?, required: true, type: text, section: Override}\n  - name: Nxtbrane\n    questions:\n      - \"Two?\"\n",
    );

    let questionnaire = Questionnaire::from_path(&path).unwrap();

    assert_eq!(
        sections_of(&questionnaire),
        [Some("Basics"), Some("Basics"), Some("Override"), Some("Nxtbrane")]
    );
    assert_eq!(questionnaire.questions[0].id.as_str(), "q001");
    assert_eq!(questionnaire.questions[1].id.as_str(), "size");
    assert_eq!(questionnaire.questions[3].id.as_str(), "q004");
}

#[test]
fn questions_and_sections_cannot_be_mixed_at_the_top_level() {
    let dir = tempfile::tempdir().unwrap();
    let path = write(
        &dir,
        "q.yaml",
        "questions: [\"A?\"]\nsections:\n  - {name: S, questions: [\"B?\"]}\n",
    );

    let error = Questionnaire::from_path(&path).unwrap_err();

    assert!(matches!(error, DomainError::QuestionnaireParse { .. }));
    assert!(error.to_string().contains("either `questions:` or `sections:`"), "{error}");
}

#[test]
fn a_section_needs_a_name_and_a_question_list() {
    let dir = tempfile::tempdir().unwrap();

    let nameless = write(&dir, "a.yaml", "sections:\n  - questions: [\"A?\"]\n");
    let error = Questionnaire::from_path(&nameless).unwrap_err();
    assert!(error.to_string().contains("section #1 needs a non-empty `name`"), "{error}");

    let not_a_list = write(&dir, "b.yaml", "sections:\n  - {name: S, questions: nope}\n");
    let error = Questionnaire::from_path(&not_a_list).unwrap_err();
    assert!(error.to_string().contains("must be a list"), "{error}");

    let not_a_map = write(&dir, "c.yaml", "sections:\n  - just text\n");
    assert!(Questionnaire::from_path(&not_a_map).is_err());
}

#[test]
fn an_empty_section_is_allowed_as_long_as_some_question_exists() {
    let dir = tempfile::tempdir().unwrap();
    let path = write(
        &dir,
        "q.yaml",
        "sections:\n  - {name: Empty}\n  - {name: Full, questions: [\"A?\"]}\n",
    );

    assert_eq!(Questionnaire::from_path(&path).unwrap().questions.len(), 1);
}
