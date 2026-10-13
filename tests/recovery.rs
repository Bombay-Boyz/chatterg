//! Going back: backups on demand, rollback to a snapshot, rewinding to a question.
//! None of these may ever lose work, and each can be undone.

use std::{path::PathBuf, sync::Arc};

use chatterg::{
    application::{RunOptions, run_with},
    domain::{Conversation, Engine, Questionnaire, State, Submission},
    recovery::{self, RecoveryError},
    storage::{
        StorageError, Store,
        backups::{self, Kind},
        sqlite::SqliteStore,
    },
    transport::mock::MockTransport,
};

fn questionnaire(total: usize) -> Questionnaire {
    let body: String = (1..=total)
        .map(|n| {
            format!("  - {{id: q{n}, question: \"Question {n}?\", required: true, type: text}}\n")
        })
        .collect();

    serde_yaml::from_str(&format!("questions:\n{body}")).unwrap()
}

/// A real conversation: `answered` of `total` questions finished through the engine.
fn conversation(total: usize, answered: usize) -> Conversation {
    let mut engine = Engine::new(questionnaire(total));

    for n in 1..=answered {
        engine = match engine.submit(format!("A{n}")) {
            Submission::Next(engine, _) | Submission::Complete(engine) => engine,
            other => panic!("unexpected {other:?}"),
        };
    }

    engine.conversation().clone()
}

async fn notebook(dir: &std::path::Path, conversation: &Conversation) -> PathBuf {
    let path = dir.join("c.db");
    let store = SqliteStore::open(&path).unwrap();
    store.save(conversation).await.unwrap();
    path
}

async fn load(path: &std::path::Path) -> Conversation {
    SqliteStore::open_read_only(path).unwrap().load().await.unwrap().unwrap()
}

// ---- backup on demand --------------------------------------------------------------------

#[tokio::test]
async fn a_backup_can_be_taken_while_a_run_holds_the_notebook() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("c.db");
    let running = SqliteStore::open(&path).unwrap();
    running.save(&conversation(6, 4)).await.unwrap();

    let snapshot = recovery::backup_now(&path).await.unwrap();

    assert_eq!(snapshot.kind, Kind::Manual);
    assert_eq!(snapshot.position, 4);
    assert!(snapshot.bytes > 0);
    assert_eq!(load(&snapshot.path).await.position, 4);
    assert_eq!(backups::list(&path).len(), 1);
}

#[tokio::test]
async fn an_empty_or_missing_notebook_has_nothing_to_back_up() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("c.db");

    assert!(matches!(
        recovery::backup_now(&path).await,
        Err(RecoveryError::Storage(StorageError::Open { .. }))
    ));

    drop(SqliteStore::open(&path).unwrap());
    assert!(matches!(recovery::backup_now(&path).await, Err(RecoveryError::NothingToBackUp(_))));
}

// ---- rollback ----------------------------------------------------------------------------------

/// A notebook now at 5 of 6, with backups taken at 2 and 4.
async fn run_with_backups(dir: &std::path::Path) -> PathBuf {
    let path = dir.join("c.db");
    {
        let store = SqliteStore::open(&path).unwrap();
        for answered in [2, 4] {
            store.save(&conversation(6, answered)).await.unwrap();
            recovery::backup_now(&path).await.unwrap();
        }
        store.save(&conversation(6, 5)).await.unwrap();
    }
    path
}

#[tokio::test]
async fn rollback_restores_the_newest_ordinary_backup_and_keeps_what_was_there() {
    let dir = tempfile::tempdir().unwrap();
    let path = run_with_backups(dir.path()).await;

    let done = recovery::rollback(&path, None).await.unwrap();

    assert_eq!(done.position, 4);
    assert_eq!(done.total, 6);
    assert_eq!(done.restored_from.position, 4);
    assert_eq!(load(&path).await, conversation(6, 4));

    // what was in the notebook before is safe in a safety copy
    let safety = done.safety.expect("a safety copy must be made");
    assert_eq!(load(&safety).await.position, 5);
    assert!(backups::list(&path).iter().any(|s| s.kind == Kind::Safety));
}

#[tokio::test]
async fn a_backup_can_be_chosen_by_its_number_in_the_list() {
    let dir = tempfile::tempdir().unwrap();
    let path = run_with_backups(dir.path()).await;

    // newest first: 1 = the position-4 backup, 2 = the position-2 backup
    let done = recovery::rollback(&path, Some(2)).await.unwrap();

    assert_eq!(done.position, 2);
    assert_eq!(load(&path).await.position, 2);
}

#[tokio::test]
async fn a_rollback_can_itself_be_undone_through_its_safety_copy() {
    let dir = tempfile::tempdir().unwrap();
    let path = run_with_backups(dir.path()).await;

    recovery::rollback(&path, None).await.unwrap(); // 5 -> 4, safety copy of 5

    let list = backups::list(&path);
    let number = list.iter().position(|s| s.kind == Kind::Safety).unwrap() + 1;
    recovery::rollback(&path, Some(number)).await.unwrap();

    assert_eq!(load(&path).await.position, 5);
}

#[tokio::test]
async fn rollback_without_backups_or_with_a_wrong_number_changes_nothing() {
    let dir = tempfile::tempdir().unwrap();
    let path = notebook(dir.path(), &conversation(6, 3)).await;

    assert!(matches!(recovery::rollback(&path, None).await, Err(RecoveryError::NoBackups { .. })));

    recovery::backup_now(&path).await.unwrap();
    for number in [0, 2, 99] {
        assert!(
            matches!(
                recovery::rollback(&path, Some(number)).await,
                Err(RecoveryError::NoSuchBackup { count: 1, .. })
            ),
            "{number}"
        );
    }

    assert_eq!(load(&path).await, conversation(6, 3));
    assert_eq!(backups::list(&path).len(), 1, "no safety copy for a rollback that did nothing");
}

#[tokio::test]
async fn rollback_is_refused_while_another_chatterg_is_using_the_notebook() {
    let dir = tempfile::tempdir().unwrap();
    let path = run_with_backups(dir.path()).await;
    let _running = SqliteStore::open(&path).unwrap();

    let result = recovery::rollback(&path, None).await;

    assert!(
        matches!(result, Err(RecoveryError::Storage(StorageError::Locked { .. }))),
        "{result:?}"
    );
    assert_eq!(load(&path).await.position, 5, "nothing may be touched");
}

#[tokio::test]
async fn a_damaged_notebook_is_moved_aside_and_the_backup_restored() {
    let dir = tempfile::tempdir().unwrap();
    let path = run_with_backups(dir.path()).await;
    std::fs::write(&path, "garbage, long enough to not be mistaken for a database header at all")
        .unwrap();
    let _ = std::fs::remove_file(dir.path().join("c.db-wal"));
    let _ = std::fs::remove_file(dir.path().join("c.db-shm"));

    let done = recovery::rollback(&path, None).await.unwrap();

    let aside = done.moved_aside.expect("the damaged file must be kept");
    assert!(std::fs::read_to_string(&aside).unwrap().starts_with("garbage"));
    assert!(done.safety.is_none(), "there was nothing readable to copy");
    assert_eq!(load(&path).await.position, 4);
}

#[tokio::test]
async fn an_empty_backup_is_refused_before_anything_is_touched() {
    let dir = tempfile::tempdir().unwrap();
    let path = notebook(dir.path(), &conversation(6, 3)).await;

    // a snapshot of a notebook that holds no run
    let empty = dir.path().join("empty.db");
    drop(SqliteStore::open(&empty).unwrap());
    let target = backups::unused_path(&path, Kind::Manual, chrono::Utc::now(), 0);
    std::fs::create_dir_all(target.parent().unwrap()).unwrap();
    SqliteStore::open_read_only(&empty).unwrap().snapshot_to(&target).unwrap();

    assert!(matches!(
        recovery::rollback(&path, None).await,
        Err(RecoveryError::EmptyBackup { .. })
    ));
    assert_eq!(load(&path).await.position, 3);
}

// ---- rewind --------------------------------------------------------------------------------------

#[tokio::test]
async fn rewind_forgets_that_question_and_everything_after_it() {
    let dir = tempfile::tempdir().unwrap();
    let path = notebook(dir.path(), &conversation(6, 5)).await;

    let done = recovery::rewind(&path, 3).await.unwrap();

    assert_eq!((done.from_position, done.to_question, done.dropped), (5, 3, 3));

    let after = load(&path).await;
    assert_eq!(after.position, 2);
    assert_eq!(after.answers.len(), 2);
    assert_eq!(after.answers[1].attempts[0].response, "A2");
    assert_eq!(after.messages_sent, 2);
    assert_eq!(after.state, State::Ready);
    assert_eq!(load(&done.safety).await.position, 5, "the old state is kept");
}

#[tokio::test]
async fn rewinding_a_finished_run_makes_it_runnable_again() {
    let dir = tempfile::tempdir().unwrap();
    let finished = conversation(4, 4);
    assert_eq!(finished.state, State::Complete);
    let path = notebook(dir.path(), &finished).await;

    recovery::rewind(&path, 4).await.unwrap();

    let after = load(&path).await;
    assert_eq!((after.position, after.state.clone(), after.finished_at), (3, State::Ready, None));
}

#[tokio::test]
async fn rewinding_to_question_one_starts_the_run_over() {
    let dir = tempfile::tempdir().unwrap();
    let path = notebook(dir.path(), &conversation(6, 3)).await;

    recovery::rewind(&path, 1).await.unwrap();

    let after = load(&path).await;
    assert_eq!((after.position, after.answers.len(), after.messages_sent), (0, 0, 0));
}

#[tokio::test]
async fn impossible_rewinds_are_refused_and_change_nothing() {
    let dir = tempfile::tempdir().unwrap();
    let path = notebook(dir.path(), &conversation(6, 3)).await;

    for question in [0, 4, 9] {
        let result = recovery::rewind(&path, question).await;
        assert!(
            matches!(result, Err(RecoveryError::CannotRewind { position: 3, next: 4, .. })),
            "{question}: {result:?}"
        );
    }

    assert_eq!(load(&path).await, conversation(6, 3));
    assert!(backups::list(&path).is_empty(), "a refused rewind makes no safety copy");
}

#[tokio::test]
async fn there_is_nothing_to_rewind_in_an_empty_or_unstarted_notebook() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("c.db");
    drop(SqliteStore::open(&path).unwrap());
    assert!(matches!(recovery::rewind(&path, 1).await, Err(RecoveryError::NothingToRewind(_))));

    let unstarted =
        notebook(tempfile::tempdir().unwrap().keep().as_path(), &conversation(6, 0)).await;
    assert!(matches!(
        recovery::rewind(&unstarted, 1).await,
        Err(RecoveryError::NothingToRewind(_))
    ));
}

#[tokio::test]
async fn rewind_is_refused_while_a_run_is_using_the_notebook() {
    let dir = tempfile::tempdir().unwrap();
    let path = notebook(dir.path(), &conversation(6, 3)).await;
    let _running = SqliteStore::open(&path).unwrap();

    assert!(matches!(
        recovery::rewind(&path, 2).await,
        Err(RecoveryError::Storage(StorageError::Locked { .. }))
    ));
    assert_eq!(load(&path).await.position, 3);
}

#[tokio::test]
async fn after_a_rewind_the_run_asks_those_questions_again_and_finishes() {
    let dir = tempfile::tempdir().unwrap();
    let path = notebook(dir.path(), &conversation(5, 4)).await;

    recovery::rewind(&path, 3).await.unwrap();

    let transport = MockTransport::new(["N3", "N4", "N5"]);
    let store = Arc::new(SqliteStore::open(&path).unwrap());
    let engine =
        run_with(questionnaire(5), "http://mock.local", &transport, store, &RunOptions::default())
            .await
            .unwrap();

    let answers: Vec<_> =
        engine.conversation().answers.iter().map(|a| a.attempts[0].response.as_str()).collect();
    assert_eq!(answers, ["A1", "A2", "N3", "N4", "N5"]);

    let sent = transport.sent();
    assert_eq!(
        sent.iter().map(|m| m.text.as_str()).collect::<Vec<_>>(),
        ["Question 3?", "Question 4?", "Question 5?"]
    );
    assert_eq!(
        sent.iter().map(|m| m.id.as_str()).collect::<Vec<_>>(),
        ["chatterg-3", "chatterg-4", "chatterg-5"]
    );
    assert_eq!(engine.conversation().state, State::Complete);
}

// ---- the conversation method itself --------------------------------------------------------------

#[test]
fn rewind_to_keeps_earlier_answers_cooldowns_and_the_question_list() {
    let mut conversation = conversation(6, 5);
    conversation.cooldowns.push(chatterg::domain::CooldownEvent {
        at: chrono::Utc::now(),
        reason: "429".into(),
        seconds: 5,
    });
    let snapshot = conversation.questionnaire_snapshot.clone();

    conversation.rewind_to(2);

    assert_eq!(conversation.answers.len(), 2);
    assert_eq!(conversation.cooldowns.len(), 1);
    assert_eq!(conversation.questionnaire_snapshot, snapshot);
    assert_eq!(conversation.next_message_number(), 3);
}
