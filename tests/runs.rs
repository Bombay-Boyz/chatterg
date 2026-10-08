//! Putting a finished run away must never lose it: the notebook is only emptied
//! once every file has been written.

use std::path::Path;

use chatterg::{
    domain::{
        AgentInfo, AnswerRecord, Attempt, Conversation, QuestionId, Questionnaire, State,
        ValidationStatus,
    },
    runs::{self, RunsError},
    storage::{Store, sqlite::SqliteStore},
};
use chrono::{TimeZone, Utc};

fn when() -> chrono::DateTime<Utc> {
    Utc.with_ymd_and_hms(2026, 10, 8, 15, 30, 12).unwrap()
}

fn questionnaire() -> Questionnaire {
    serde_yaml::from_str(
        "questions:\n  - {id: a, question: \"What is A?\", required: true, type: text, section: Basics}\n",
    )
    .unwrap()
}

fn finished(target: &str) -> Conversation {
    Conversation {
        state: State::Complete,
        answers: vec![AnswerRecord {
            question_id: QuestionId::new("a"),
            attempts: vec![Attempt {
                number: 1,
                request: "What is A?".into(),
                response: "A is a letter.".into(),
                validation: ValidationStatus::Accepted,
                sent_at: Some(when()),
                received_at: Some(when()),
                latency_ms: Some(10),
            }],
        }],
        position: 1,
        messages_sent: 1,
        started_at: Some(when()),
        finished_at: Some(when()),
        agent: Some(AgentInfo {
            name: Some("Bot".into()),
            target: target.into(),
            endpoint: format!("{target}/rpc"),
            protocol: "a2a".into(),
        }),
        ..Conversation::default()
    }
}

fn names(folder: &Path) -> Vec<String> {
    let mut names: Vec<_> = std::fs::read_dir(folder)
        .unwrap()
        .filter_map(Result::ok)
        .map(|entry| entry.file_name().to_string_lossy().into_owned())
        .collect();
    names.sort();
    names
}

// ---- names ------------------------------------------------------------------------

#[test]
fn slugs_are_short_lowercase_and_dash_separated() {
    assert_eq!(runs::slug("flowmarket.social/nxtbrane"), "flowmarket-social-nxtbrane");
    assert_eq!(runs::slug("  Hello,   World!! "), "hello-world");
    assert_eq!(runs::slug("///"), "");
    assert_eq!(runs::slug(&"a".repeat(80)).len(), 50);
    assert!(!runs::slug(&format!("{}-b", "a".repeat(49))).ends_with('-'));
}

#[test]
fn the_folder_name_has_the_finish_time_and_the_bot() {
    let conversation = finished("https://flowmarket.social/nxtbrane");

    assert_eq!(
        runs::folder_name(&conversation, Utc::now()),
        "20261008-153012-flowmarket-social-nxtbrane"
    );
}

#[test]
fn a_run_with_no_finish_time_or_agent_still_gets_a_name() {
    let conversation = Conversation::default();

    assert_eq!(runs::folder_name(&conversation, when()), "20261008-153012-run");
}

// ---- archiving ----------------------------------------------------------------------

#[tokio::test]
async fn a_run_is_saved_and_the_notebook_is_emptied() {
    let dir = tempfile::tempdir().unwrap();
    let store = SqliteStore::open(dir.path().join("chatterg.db")).unwrap();
    let conversation = finished("https://flowmarket.social/nxtbrane");
    store.save(&conversation).await.unwrap();

    let questions = dir.path().join("questions.txt");
    std::fs::write(&questions, "## Basics\nWhat is A?\n").unwrap();
    let runs_dir = dir.path().join("runs");

    let saved = runs::archive_run(
        &store,
        &conversation,
        Some(&questionnaire()),
        Some(&questions),
        &runs_dir,
    )
    .unwrap();

    assert_eq!(saved.folder, runs_dir.join("20261008-153012-flowmarket-social-nxtbrane"));
    assert_eq!(
        saved.files,
        ["report.html", "report.md", "answers.csv", "questions.txt", "notebook.db"]
    );
    assert_eq!(
        names(&saved.folder),
        ["answers.csv", "notebook.db", "questions.txt", "report.html", "report.md"]
    );

    // the contents are real
    assert!(
        std::fs::read_to_string(saved.folder.join("report.html"))
            .unwrap()
            .contains("A is a letter.")
    );
    assert!(
        std::fs::read_to_string(saved.folder.join("report.md")).unwrap().contains("### Basics")
    );
    assert!(
        std::fs::read_to_string(saved.folder.join("answers.csv"))
            .unwrap()
            .contains("A is a letter.")
    );
    assert_eq!(
        std::fs::read_to_string(saved.folder.join("questions.txt")).unwrap(),
        "## Basics\nWhat is A?\n"
    );

    // the archived notebook still holds the whole run ...
    let archived = SqliteStore::open_read_only(saved.folder.join("notebook.db")).unwrap();
    assert_eq!(archived.load().await.unwrap().unwrap(), conversation);

    // ... and the working notebook is empty
    assert!(store.load().await.unwrap().is_none());
}

#[tokio::test]
async fn two_runs_in_the_same_second_get_separate_folders() {
    let dir = tempfile::tempdir().unwrap();
    let store = SqliteStore::open(dir.path().join("chatterg.db")).unwrap();
    let conversation = finished("https://example.com/bot");
    let runs_dir = dir.path().join("runs");
    let mut folders = Vec::new();

    for _ in 0..3 {
        store.save(&conversation).await.unwrap();
        folders
            .push(runs::archive_run(&store, &conversation, None, None, &runs_dir).unwrap().folder);
    }

    let names: Vec<_> =
        folders.iter().map(|f| f.file_name().unwrap().to_string_lossy().into_owned()).collect();
    assert_eq!(
        names,
        [
            "20261008-153012-example-com-bot",
            "20261008-153012-example-com-bot-2",
            "20261008-153012-example-com-bot-3"
        ]
    );
}

#[tokio::test]
async fn an_earlier_run_is_never_overwritten() {
    let dir = tempfile::tempdir().unwrap();
    let store = SqliteStore::open(dir.path().join("chatterg.db")).unwrap();
    let conversation = finished("https://example.com/bot");
    let runs_dir = dir.path().join("runs");

    store.save(&conversation).await.unwrap();
    let first = runs::archive_run(&store, &conversation, None, None, &runs_dir).unwrap().folder;
    std::fs::write(first.join("report.md"), "my own notes").unwrap();

    store.save(&conversation).await.unwrap();
    runs::archive_run(&store, &conversation, None, None, &runs_dir).unwrap();

    assert_eq!(std::fs::read_to_string(first.join("report.md")).unwrap(), "my own notes");
}

#[tokio::test]
async fn a_leftover_run_is_saved_without_the_questions_file() {
    let dir = tempfile::tempdir().unwrap();
    let store = SqliteStore::open(dir.path().join("chatterg.db")).unwrap();
    let conversation = finished("https://example.com/bot");
    store.save(&conversation).await.unwrap();

    let saved =
        runs::archive_run(&store, &conversation, None, None, &dir.path().join("runs")).unwrap();

    assert_eq!(saved.files, ["report.html", "report.md", "answers.csv", "notebook.db"]);
    assert!(store.load().await.unwrap().is_none());
}

#[tokio::test]
async fn an_empty_notebook_saves_the_report_but_has_nothing_to_archive() {
    let dir = tempfile::tempdir().unwrap();
    let store = SqliteStore::open(dir.path().join("chatterg.db")).unwrap();

    let saved = runs::archive_run(
        &store,
        &finished("https://example.com/bot"),
        None,
        None,
        &dir.path().join("runs"),
    )
    .unwrap();

    assert!(!saved.files.contains(&"notebook.db".to_owned()));
    assert!(!saved.folder.join("notebook.db").exists());
}

// ---- nothing is lost when something goes wrong ------------------------------------------

#[tokio::test]
async fn if_the_folder_cannot_be_made_the_notebook_keeps_the_run() {
    let dir = tempfile::tempdir().unwrap();
    let store = SqliteStore::open(dir.path().join("chatterg.db")).unwrap();
    let conversation = finished("https://example.com/bot");
    store.save(&conversation).await.unwrap();

    // a file where the runs folder should be
    let blocker = dir.path().join("runs");
    std::fs::write(&blocker, "x").unwrap();

    let result = runs::archive_run(&store, &conversation, None, None, &blocker);

    assert!(matches!(result, Err(RunsError::CreateFolder { .. })));
    assert_eq!(store.load().await.unwrap().unwrap(), conversation, "the run must still be there");
}

#[tokio::test]
async fn if_the_questions_cannot_be_copied_nothing_is_emptied_and_no_half_folder_is_left() {
    let dir = tempfile::tempdir().unwrap();
    let store = SqliteStore::open(dir.path().join("chatterg.db")).unwrap();
    let conversation = finished("https://example.com/bot");
    store.save(&conversation).await.unwrap();
    let runs_dir = dir.path().join("runs");

    let result = runs::archive_run(
        &store,
        &conversation,
        Some(&questionnaire()),
        Some(&dir.path().join("no-such-questions.txt")),
        &runs_dir,
    );

    assert!(matches!(result, Err(RunsError::WriteFile { .. })));
    assert_eq!(store.load().await.unwrap().unwrap(), conversation);
    assert!(
        std::fs::read_dir(&runs_dir).unwrap().next().is_none(),
        "no half-written folder may remain"
    );
}
