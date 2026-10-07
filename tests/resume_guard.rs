//! A run must not silently continue against a questions file that changed.

use std::sync::Arc;

use chatterg::{
    application::{ApplicationError, RunOptions, run_with},
    domain::{DomainError, Questionnaire, State},
    storage::{Store, sqlite::SqliteStore},
    transport::mock::MockTransport,
};

fn q(body: &str) -> Questionnaire {
    serde_yaml::from_str(&format!("questions:\n{body}")).unwrap()
}

const A: &str = "  - {id: a, question: \"A?\", required: true, type: text}\n";
const B: &str = "  - {id: b, question: \"B?\", required: true, type: text}\n";
const C: &str = "  - {id: c, question: \"C?\", required: true, type: text}\n";
const D: &str = "  - {id: d, question: \"D?\", required: true, type: text}\n";

fn abc() -> Questionnaire {
    q(&format!("{A}{B}{C}"))
}

async fn resume(
    questionnaire: Questionnaire,
    store: &Arc<SqliteStore>,
    answers: &[&str],
    force: bool,
) -> Result<chatterg::domain::Engine, ApplicationError> {
    let options = RunOptions { force_resume: force, ..RunOptions::default() };

    run_with(
        questionnaire,
        "http://mock.local",
        &MockTransport::new(answers.iter().copied()),
        Arc::clone(store),
        &options,
    )
    .await
}

/// A store holding a run that answered `a` and stopped before `b`.
async fn stopped_after_a() -> Arc<SqliteStore> {
    let store = Arc::new(SqliteStore::memory().unwrap());

    // The mock runs out of answers when `b` is asked, which stops the run.
    assert!(resume(abc(), &store, &["A1"], false).await.is_err());

    let saved = store.load().await.unwrap().unwrap();
    assert_eq!(saved.position, 1);
    store
}

#[tokio::test]
async fn unchanged_file_resumes() {
    let store = stopped_after_a().await;

    let engine = resume(abc(), &store, &["B1", "C1"], false).await.unwrap();

    assert_eq!(engine.conversation().state, State::Complete);
    assert_eq!(engine.conversation().answers.len(), 3);
}

#[tokio::test]
async fn appended_question_is_refused_without_force_and_named_in_the_error() {
    let store = stopped_after_a().await;

    let error = resume(q(&format!("{A}{B}{C}{D}")), &store, &["B1", "C1", "D1"], false)
        .await
        .err()
        .unwrap();

    match error {
        ApplicationError::Domain(DomainError::QuestionnaireChanged { summary }) => {
            assert!(summary.contains("1 added (d)"), "summary was: {summary}");
        }
        other => panic!("expected QuestionnaireChanged, got {other:?}"),
    }
}

#[tokio::test]
async fn appended_question_continues_with_force_and_the_new_list_is_remembered() {
    let store = stopped_after_a().await;
    let longer = q(&format!("{A}{B}{C}{D}"));

    let engine = resume(longer.clone(), &store, &["B1", "C1", "D1"], true).await.unwrap();

    assert_eq!(engine.conversation().answers.len(), 4);
    assert_eq!(
        engine.conversation().questionnaire_fingerprint.as_deref(),
        Some(longer.fingerprint().as_str())
    );
}

#[tokio::test]
async fn editing_a_question_not_yet_asked_is_allowed_with_force() {
    let store = stopped_after_a().await;
    let edited = q(&format!(
        "{A}{B}  - {{id: c, question: \"C, reworded?\", required: true, type: text}}\n"
    ));

    let engine = resume(edited, &store, &["B1", "C1"], true).await.unwrap();

    assert_eq!(engine.conversation().answers[2].attempts[0].request, "C, reworded?");
}

#[tokio::test]
async fn editing_an_answered_question_cannot_be_forced() {
    let store = stopped_after_a().await;
    let edited = q(&format!(
        "  - {{id: a, question: \"A, reworded?\", required: true, type: text}}\n{B}{C}"
    ));

    let error = resume(edited, &store, &["B1", "C1"], true).await.err().unwrap();

    assert!(matches!(
        error,
        ApplicationError::Domain(DomainError::QuestionnaireChangedUnsafe { .. })
    ));
}

#[tokio::test]
async fn editing_the_question_in_progress_cannot_be_forced() {
    let store = stopped_after_a().await;
    let edited = q(&format!(
        "{A}  - {{id: b, question: \"B, reworded?\", required: true, type: text}}\n{C}"
    ));

    let error = resume(edited, &store, &["B1", "C1"], true).await.err().unwrap();

    assert!(matches!(
        error,
        ApplicationError::Domain(DomainError::QuestionnaireChangedUnsafe { .. })
    ));
}

#[tokio::test]
async fn reordering_cannot_be_forced() {
    let store = stopped_after_a().await;

    let error = resume(q(&format!("{B}{A}{C}")), &store, &["B1", "C1"], true).await.err().unwrap();

    match error {
        ApplicationError::Domain(DomainError::QuestionnaireChangedUnsafe { summary }) => {
            assert!(summary.contains("order changed"), "summary was: {summary}");
        }
        other => panic!("expected QuestionnaireChangedUnsafe, got {other:?}"),
    }
}

#[tokio::test]
async fn removing_an_answered_question_cannot_be_forced() {
    let store = stopped_after_a().await;

    let error = resume(q(&format!("{B}{C}")), &store, &["B1", "C1"], true).await.err().unwrap();

    assert!(matches!(
        error,
        ApplicationError::Domain(DomainError::QuestionnaireChangedUnsafe { .. })
    ));
}

#[tokio::test]
async fn changing_only_retry_settings_is_not_a_change() {
    let store = stopped_after_a().await;
    let tweaked = q(&format!(
        "{A}  - {{id: b, question: \"B?\", required: true, type: text, max_followups: 5}}\n{C}"
    ));

    assert!(resume(tweaked, &store, &["B1", "C1"], false).await.is_ok());
}

#[tokio::test]
async fn runs_saved_before_fingerprints_existed_adopt_the_current_questions() {
    let store = stopped_after_a().await;

    let mut old = store.load().await.unwrap().unwrap();
    old.questionnaire_fingerprint = None;
    old.questionnaire_snapshot.clear();
    store.save(&old).await.unwrap();

    let engine = resume(abc(), &store, &["B1", "C1"], false).await.unwrap();

    assert_eq!(
        engine.conversation().questionnaire_fingerprint.as_deref(),
        Some(abc().fingerprint().as_str())
    );
}

#[test]
fn fingerprint_is_stable_and_sensitive_to_content() {
    assert_eq!(abc().fingerprint(), abc().fingerprint());
    assert_eq!(abc().fingerprint().len(), 64);
    assert_ne!(abc().fingerprint(), q(&format!("{A}{B}")).fingerprint());
    assert_ne!(abc().fingerprint(), q(&format!("{B}{A}{C}")).fingerprint());
}
