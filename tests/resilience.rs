//! A run must survive a flaky disk and a long internet outage without losing its
//! place or hammering the bot.

use std::{
    collections::VecDeque,
    sync::{
        Arc, Mutex,
        atomic::{AtomicUsize, Ordering},
    },
    time::{Duration, Instant},
};

use async_trait::async_trait;
use chatterg::{
    application::{ApplicationError, Progress, ProgressCallback, RunOptions, run_with},
    cancel::Cancel,
    domain::{Conversation, Questionnaire, State},
    storage::{StorageError, Store},
    transport::{Capabilities, Message, Protocol, Response, Transport, TransportError},
};
use url::Url;

fn questions(count: usize) -> Questionnaire {
    let body: String = (1..=count)
        .map(|n| {
            format!("  - {{id: q{n}, question: \"Question {n}?\", required: true, type: text}}\n")
        })
        .collect();

    serde_yaml::from_str(&format!("questions:\n{body}")).unwrap()
}

// ---- a store that can be told to fail ---------------------------------------------------

/// Keeps the conversation in memory. Saves number `fail_from..fail_until` (1-based) fail.
struct FlakyStore {
    saved: Mutex<Option<Conversation>>,
    attempts: AtomicUsize,
    fail_from: usize,
    fail_until: usize,
    warnings: Mutex<Vec<String>>,
}

impl FlakyStore {
    fn new(fail_from: usize, fail_until: usize) -> Self {
        Self {
            saved: Mutex::new(None),
            attempts: AtomicUsize::new(0),
            fail_from,
            fail_until,
            warnings: Mutex::new(Vec::new()),
        }
    }

    fn with_warning(self, text: &str) -> Self {
        self.warnings.lock().unwrap().push(text.to_owned());
        self
    }

    fn saved(&self) -> Option<Conversation> {
        self.saved.lock().unwrap().clone()
    }

    fn attempts(&self) -> usize {
        self.attempts.load(Ordering::SeqCst)
    }
}

#[async_trait]
impl Store for FlakyStore {
    async fn save(&self, conversation: &Conversation) -> Result<(), StorageError> {
        let number = self.attempts.fetch_add(1, Ordering::SeqCst) + 1;

        if number >= self.fail_from && number < self.fail_until {
            return Err(StorageError::Database(Box::new(std::io::Error::other("disk full"))));
        }

        *self.saved.lock().unwrap() = Some(conversation.clone());
        Ok(())
    }

    async fn load(&self) -> Result<Option<Conversation>, StorageError> {
        Ok(self.saved())
    }

    fn take_warnings(&self) -> Vec<String> {
        std::mem::take(&mut *self.warnings.lock().unwrap())
    }
}

// ---- a bot that replays outcomes -----------------------------------------------------------

struct Bot {
    outcomes: Mutex<VecDeque<Result<String, TransportError>>>,
}

impl Bot {
    fn new(outcomes: Vec<Result<&str, TransportError>>) -> Self {
        Self { outcomes: Mutex::new(outcomes.into_iter().map(|o| o.map(str::to_owned)).collect()) }
    }
}

#[async_trait]
impl Transport for Bot {
    async fn discover(&self, target: &Url) -> Result<Capabilities, TransportError> {
        Ok(Capabilities {
            protocols: vec![Protocol::A2a],
            endpoint: target.clone(),
            agent_name: None,
        })
    }

    async fn send(&self, _: &Url, _: Message) -> Result<Response, TransportError> {
        self.outcomes
            .lock()
            .unwrap()
            .pop_front()
            .expect("script exhausted")
            .map(|text| Response { text })
    }
}

fn quick() -> RunOptions {
    RunOptions {
        cooldown: Duration::from_millis(10),
        save_retry_pause: Duration::from_millis(5),
        ..RunOptions::default()
    }
}

fn events() -> (Arc<Mutex<Vec<Progress>>>, ProgressCallback) {
    let log = Arc::new(Mutex::new(Vec::new()));
    let sink = Arc::clone(&log);
    (log, Arc::new(move |progress: &Progress| sink.lock().unwrap().push(progress.clone())))
}

fn waits(log: &Mutex<Vec<Progress>>) -> Vec<Duration> {
    log.lock()
        .unwrap()
        .iter()
        .filter_map(|event| match event {
            Progress::Waiting { cooldown, .. } => Some(*cooldown),
            _ => None,
        })
        .collect()
}

// ---- saving progress ------------------------------------------------------------------------

#[tokio::test]
async fn a_save_that_fails_twice_and_then_works_does_not_stop_the_run() {
    let store = Arc::new(FlakyStore::new(2, 4)); // saves 2 and 3 fail; their retry succeeds
    let bot = Bot::new(vec![Ok("A1"), Ok("A2")]);

    let engine = run_with(questions(2), "http://mock.local", &bot, Arc::clone(&store), &quick())
        .await
        .unwrap();

    assert_eq!(engine.conversation().state, State::Complete);
    assert_eq!(store.saved().unwrap().position, 2);
    assert!(store.attempts() > 2);
}

#[tokio::test]
async fn a_disk_that_stays_broken_stops_the_run_with_a_clear_message_and_loses_nothing() {
    // the first two saves work (questions 1 and 2 are safe), then the disk fails for good
    let store = Arc::new(FlakyStore::new(3, usize::MAX));
    let bot = Bot::new(vec![Ok("A1"), Ok("A2"), Ok("A3")]);

    let error = run_with(questions(3), "http://mock.local", &bot, Arc::clone(&store), &quick())
        .await
        .err()
        .unwrap();

    match &error {
        ApplicationError::CouldNotSave { attempts: 3, .. } => {}
        other => panic!("expected CouldNotSave, got {other:?}"),
    }
    let message = error.to_string();
    assert!(message.contains("could not save your progress after 3 tries"), "{message}");
    assert!(message.contains("disk full"), "{message}");
    assert!(message.contains("run the same command again"), "{message}");

    assert_eq!(store.saved().unwrap().position, 2, "what was saved earlier is intact");
}

#[tokio::test]
async fn a_stop_during_the_wait_between_save_attempts_is_immediate() {
    let cancel = Cancel::default();
    let store = Arc::new(FlakyStore::new(1, usize::MAX));
    let options =
        RunOptions { save_retry_pause: Duration::from_secs(30), cancel: cancel.clone(), ..quick() };
    let bot = Bot::new(vec![Ok("A1")]);

    let stopper = cancel.clone();
    tokio::spawn(async move {
        tokio::time::sleep(Duration::from_millis(100)).await;
        stopper.cancel();
    });

    let started = Instant::now();
    let result = run_with(questions(1), "http://mock.local", &bot, store, &options).await;

    assert!(matches!(result, Err(ApplicationError::Interrupted)));
    assert!(started.elapsed() < Duration::from_secs(3));
}

#[tokio::test]
async fn warnings_from_the_store_reach_the_user() {
    let store = Arc::new(
        FlakyStore::new(usize::MAX, usize::MAX).with_warning("could not write a backup to /x"),
    );
    let (log, callback) = events();
    let options = RunOptions { on_progress: Some(callback), ..quick() };

    run_with(questions(1), "http://mock.local", &Bot::new(vec![Ok("A1")]), store, &options)
        .await
        .unwrap();

    let warnings: Vec<_> = log
        .lock()
        .unwrap()
        .iter()
        .filter_map(|event| match event {
            Progress::Warning(text) => Some(text.clone()),
            _ => None,
        })
        .collect();
    assert_eq!(warnings, ["could not write a backup to /x"]);
}

// ---- internet outages -------------------------------------------------------------------------

fn down() -> TransportError {
    TransportError::Network("connection refused".into())
}

fn memory() -> Arc<FlakyStore> {
    Arc::new(FlakyStore::new(usize::MAX, usize::MAX))
}

#[tokio::test]
async fn a_long_outage_is_waited_out_with_pauses_that_double() {
    let (log, callback) = events();
    let options = RunOptions { on_progress: Some(callback), ..quick() };
    let bot = Bot::new(vec![Err(down()), Err(down()), Err(down()), Err(down()), Ok("A1")]);

    run_with(questions(1), "http://mock.local", &bot, memory(), &options).await.unwrap();

    assert_eq!(
        waits(&log),
        [10, 20, 40, 80].map(Duration::from_millis),
        "each failed attempt in a row doubles the pause"
    );
}

#[tokio::test]
async fn the_user_is_told_when_things_are_back_to_normal() {
    let (log, callback) = events();
    let options = RunOptions { on_progress: Some(callback), ..quick() };
    let bot = Bot::new(vec![Err(down()), Err(down()), Ok("A1")]);

    run_with(questions(1), "http://mock.local", &bot, memory(), &options).await.unwrap();

    let resumed: Vec<_> = log
        .lock()
        .unwrap()
        .iter()
        .filter_map(|event| match event {
            Progress::Resumed { waits, paused } => Some((*waits, *paused)),
            _ => None,
        })
        .collect();
    assert_eq!(resumed, [(2, Duration::from_millis(30))]);
}

#[tokio::test]
async fn a_failure_that_is_not_an_outage_starts_the_doubling_over() {
    let (log, callback) = events();
    let options = RunOptions { on_progress: Some(callback), ..quick() };
    let bot = Bot::new(vec![
        Err(down()),
        Err(down()),
        Err(TransportError::Http { status: 503 }), // the bot is reachable again, just busy
        Err(down()),
        Ok("A1"),
    ]);

    run_with(questions(1), "http://mock.local", &bot, memory(), &options).await.unwrap();

    assert_eq!(waits(&log), [10, 20, 10, 10].map(Duration::from_millis));
}

#[tokio::test]
async fn the_outage_pause_never_shrinks_a_cooldown_the_user_chose() {
    let options = RunOptions { cooldown: Duration::from_secs(900), ..RunOptions::default() };

    for streak in [0, 1, 2, 5, 40] {
        assert_eq!(options.pause_for(None, streak), Duration::from_secs(900), "streak {streak}");
    }
}

#[test]
fn pause_lengths_follow_the_rules() {
    let options = RunOptions { cooldown: Duration::from_secs(120), ..RunOptions::default() };

    let secs = |streak| options.pause_for(None, streak).as_secs();

    assert_eq!(secs(0), 120);
    assert_eq!(secs(1), 120, "the first failure waits the normal cooldown");
    assert_eq!(secs(2), 240);
    assert_eq!(secs(3), 480);
    assert_eq!(secs(4), 600, "capped at ten minutes");
    assert_eq!(secs(12), 600);
    assert_eq!(secs(10_000), 600, "no overflow however long the outage");
}

#[test]
fn a_server_hint_still_wins_when_it_is_longer_and_is_capped_by_max_wait() {
    let options = RunOptions {
        cooldown: Duration::from_secs(10),
        max_wait: Duration::from_secs(300),
        ..RunOptions::default()
    };

    assert_eq!(options.pause_for(Some(Duration::from_secs(90)), 0).as_secs(), 90);
    assert_eq!(options.pause_for(Some(Duration::from_secs(5)), 0).as_secs(), 10);
    assert_eq!(options.pause_for(Some(Duration::from_secs(9999)), 0).as_secs(), 300);
}

#[tokio::test]
async fn the_run_gives_up_after_max_waits_even_in_an_outage() {
    let options = RunOptions { max_waits: 3, ..quick() };
    let bot = Bot::new(vec![Err(down()), Err(down()), Err(down()), Err(down())]);
    let store = memory();

    let error = run_with(questions(2), "http://mock.local", &bot, Arc::clone(&store), &options)
        .await
        .err()
        .unwrap();

    assert!(matches!(error, ApplicationError::AgentUnavailable { waits: 3, .. }), "{error:?}");
    assert!(store.saved().is_some(), "the progress made so far is saved");
}
