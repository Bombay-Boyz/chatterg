//! A run must stop cleanly when asked, obey what the server says about waiting,
//! and keep to a steady pace when told to.

use std::{
    collections::VecDeque,
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};

use async_trait::async_trait;
use chatterg::{
    application::{ApplicationError, Progress, RunOptions, run_with},
    cancel::Cancel,
    domain::{Questionnaire, State},
    storage::{Store, sqlite::SqliteStore},
    transport::{Capabilities, Message, Protocol, Response, Transport, TransportError},
};
use url::Url;

type Hook = Box<dyn Fn(usize) + Send + Sync>;

/// Replays outcomes, tells a hook about every send, and remembers when each happened.
struct Bot {
    outcomes: Mutex<VecDeque<Result<String, TransportError>>>,
    on_send: Hook,
    sent: Mutex<Vec<Instant>>,
}

impl Bot {
    fn new(outcomes: Vec<Result<&str, TransportError>>) -> Self {
        Self::hooked(outcomes, Box::new(|_| {}))
    }

    fn hooked(outcomes: Vec<Result<&str, TransportError>>, on_send: Hook) -> Self {
        Self {
            outcomes: Mutex::new(outcomes.into_iter().map(|o| o.map(str::to_owned)).collect()),
            on_send,
            sent: Mutex::new(Vec::new()),
        }
    }

    fn sends(&self) -> Vec<Instant> {
        self.sent.lock().unwrap().clone()
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
        let index = {
            let mut sent = self.sent.lock().unwrap();
            sent.push(Instant::now());
            sent.len() - 1
        };

        (self.on_send)(index);

        self.outcomes
            .lock()
            .unwrap()
            .pop_front()
            .expect("script exhausted")
            .map(|text| Response { text })
    }
}

fn questions(count: usize) -> Questionnaire {
    let body: String = (1..=count)
        .map(|n| {
            format!("  - {{id: q{n}, question: \"Question {n}?\", required: true, type: text}}\n")
        })
        .collect();

    serde_yaml::from_str(&format!("questions:\n{body}")).unwrap()
}

fn store() -> Arc<SqliteStore> {
    Arc::new(SqliteStore::memory().unwrap())
}

fn fast() -> RunOptions {
    RunOptions { cooldown: Duration::from_millis(5), ..RunOptions::default() }
}

fn waiting_log() -> (Arc<Mutex<Vec<Duration>>>, chatterg::application::ProgressCallback) {
    let log = Arc::new(Mutex::new(Vec::new()));
    let sink = Arc::clone(&log);

    let callback: chatterg::application::ProgressCallback = Arc::new(move |progress: &Progress| {
        if let Progress::Waiting { cooldown, .. } = progress {
            sink.lock().unwrap().push(*cooldown);
        }
    });

    (log, callback)
}

fn cancel_after(cancel: &Cancel, millis: u64) {
    let cancel = cancel.clone();

    tokio::spawn(async move {
        tokio::time::sleep(Duration::from_millis(millis)).await;
        cancel.cancel();
    });
}

// ---- stopping ------------------------------------------------------------------------

#[tokio::test]
async fn a_run_cancelled_before_it_starts_asks_nothing() {
    let cancel = Cancel::default();
    cancel.cancel();
    let options = RunOptions { cancel, ..fast() };
    let bot = Bot::new(vec![]);
    let store = store();

    let result =
        run_with(questions(2), "http://mock.local", &bot, Arc::clone(&store), &options).await;

    assert!(matches!(result, Err(ApplicationError::Interrupted)));
    assert!(bot.sends().is_empty());
    assert!(store.load().await.unwrap().is_none_or(|saved| saved.answers.is_empty()));
}

#[tokio::test]
async fn the_question_being_asked_is_finished_and_saved_before_stopping() {
    let cancel = Cancel::default();
    let trigger = cancel.clone();
    let bot = Bot::hooked(
        vec![Ok("A1"), Ok("A2")],
        Box::new(move |index| {
            if index == 0 {
                trigger.cancel(); // Ctrl+C arrives while question 1 is in flight
            }
        }),
    );
    let store = store();

    let result = run_with(
        questions(2),
        "http://mock.local",
        &bot,
        Arc::clone(&store),
        &RunOptions { cancel, ..fast() },
    )
    .await;

    assert!(matches!(result, Err(ApplicationError::Interrupted)));
    assert_eq!(bot.sends().len(), 1, "question 2 must not be started");

    let saved = store.load().await.unwrap().unwrap();
    assert_eq!(saved.position, 1);
    assert_eq!(
        saved.answers[0].attempts[0].response, "A1",
        "the in-flight answer must not be lost"
    );
}

#[tokio::test]
async fn after_a_stop_the_same_run_carries_on_and_finishes() {
    let store = store();

    let cancel = Cancel::default();
    let trigger = cancel.clone();
    let first = Bot::hooked(vec![Ok("A1")], Box::new(move |_| trigger.cancel()));
    run_with(
        questions(2),
        "http://mock.local",
        &first,
        Arc::clone(&store),
        &RunOptions { cancel, ..fast() },
    )
    .await
    .unwrap_err();

    let second = Bot::new(vec![Ok("A2")]);
    let engine = run_with(questions(2), "http://mock.local", &second, Arc::clone(&store), &fast())
        .await
        .unwrap();

    assert_eq!(engine.conversation().state, State::Complete);
    assert_eq!(engine.conversation().answers.len(), 2);
    assert_eq!(engine.conversation().messages_sent, 2);
    assert_eq!(second.sends().len(), 1, "only the remaining question is asked");
}

#[tokio::test]
async fn a_stop_during_the_pause_between_questions_is_immediate() {
    let cancel = Cancel::default();
    let options = RunOptions { delay: Duration::from_secs(30), cancel: cancel.clone(), ..fast() };
    let bot = Bot::new(vec![Ok("A1"), Ok("A2")]);
    let store = store();
    cancel_after(&cancel, 100);

    let started = Instant::now();
    let result =
        run_with(questions(2), "http://mock.local", &bot, Arc::clone(&store), &options).await;

    assert!(matches!(result, Err(ApplicationError::Interrupted)));
    assert!(started.elapsed() < Duration::from_secs(3), "took {:?}", started.elapsed());
    assert_eq!(store.load().await.unwrap().unwrap().position, 1);
}

#[tokio::test]
async fn a_stop_during_a_cooldown_is_immediate_and_keeps_the_log() {
    let cancel = Cancel::default();
    let options = RunOptions {
        cooldown: Duration::from_secs(30),
        cancel: cancel.clone(),
        ..RunOptions::default()
    };
    let bot = Bot::new(vec![Err(TransportError::Http { status: 503 })]);
    let store = store();
    cancel_after(&cancel, 100);

    let started = Instant::now();
    let result =
        run_with(questions(1), "http://mock.local", &bot, Arc::clone(&store), &options).await;

    assert!(matches!(result, Err(ApplicationError::Interrupted)));
    assert!(started.elapsed() < Duration::from_secs(3), "took {:?}", started.elapsed());

    let saved = store.load().await.unwrap().unwrap();
    assert!(saved.answers.is_empty());
    assert_eq!(saved.cooldowns.len(), 1, "the wait that was interrupted is still logged");
}

#[tokio::test]
async fn no_retry_is_started_once_a_stop_was_asked_for() {
    let questionnaire: Questionnaire = serde_yaml::from_str(
        "questions:\n  - {id: a, question: \"A?\", required: true, type: text, max_followups: 2, reject_if_contains: [\"meeting\"]}\n",
    )
    .unwrap();
    let cancel = Cancel::default();
    let trigger = cancel.clone();
    let bot = Bot::hooked(
        vec![Ok("let's schedule a meeting"), Ok("Acme")],
        Box::new(move |_| trigger.cancel()),
    );
    let store = store();

    let result = run_with(
        questionnaire,
        "http://mock.local",
        &bot,
        Arc::clone(&store),
        &RunOptions { cancel, ..fast() },
    )
    .await;

    assert!(matches!(result, Err(ApplicationError::Interrupted)));
    assert_eq!(bot.sends().len(), 1);

    let saved = store.load().await.unwrap().unwrap();
    assert_eq!(saved.position, 0);
    assert_eq!(
        saved.answers[0].attempts.len(),
        1,
        "the dodgy reply is recorded, the retry is left for next time"
    );
}

// ---- what the server says about waiting ---------------------------------------------------

async fn one_wait(
    cooldown: Duration,
    max_wait: Duration,
    failure: TransportError,
) -> (Duration, Duration) {
    let (log, callback) = waiting_log();
    let options =
        RunOptions { cooldown, max_wait, on_progress: Some(callback), ..RunOptions::default() };
    let bot = Bot::new(vec![Err(failure), Ok("A1")]);

    let started = Instant::now();
    run_with(questions(1), "http://mock.local", &bot, store(), &options).await.unwrap();
    let elapsed = started.elapsed();

    let waited = log.lock().unwrap()[0];
    (waited, elapsed)
}

fn retry_later(millis: u64) -> TransportError {
    TransportError::RetryLater { status: 429, retry_after: Duration::from_millis(millis) }
}

#[tokio::test]
async fn a_longer_wait_asked_for_by_the_server_wins() {
    let (waited, elapsed) =
        one_wait(Duration::from_millis(10), Duration::from_secs(60), retry_later(150)).await;

    assert_eq!(waited, Duration::from_millis(150));
    assert!(elapsed >= Duration::from_millis(140), "{elapsed:?}");
}

#[tokio::test]
async fn a_shorter_wait_asked_for_by_the_server_does_not_cut_our_cooldown() {
    let (waited, elapsed) =
        one_wait(Duration::from_millis(100), Duration::from_secs(60), retry_later(10)).await;

    assert_eq!(waited, Duration::from_millis(100));
    assert!(elapsed >= Duration::from_millis(95), "{elapsed:?}");
}

#[tokio::test]
async fn a_huge_wait_asked_for_by_the_server_is_capped() {
    let (waited, elapsed) = one_wait(
        Duration::from_millis(10),
        Duration::from_millis(120),
        TransportError::RetryLater { status: 429, retry_after: Duration::from_secs(36_000) },
    )
    .await;

    assert_eq!(waited, Duration::from_millis(120));
    assert!(elapsed < Duration::from_secs(5), "{elapsed:?}");
}

#[tokio::test]
async fn our_own_cooldown_is_never_cut_by_the_cap() {
    let (waited, _) = one_wait(
        Duration::from_millis(200),
        Duration::from_millis(50),
        TransportError::Http { status: 503 },
    )
    .await;

    assert_eq!(waited, Duration::from_millis(200));
}

#[tokio::test]
async fn the_cooldown_log_records_the_wait_that_was_actually_used() {
    let options = RunOptions {
        cooldown: Duration::from_millis(5),
        max_wait: Duration::from_secs(60),
        ..RunOptions::default()
    };
    let bot = Bot::new(vec![
        Err(TransportError::RetryLater { status: 429, retry_after: Duration::from_millis(1500) }),
        Ok("A1"),
    ]);

    let engine =
        run_with(questions(1), "http://mock.local", &bot, store(), &options).await.unwrap();

    let log = &engine.conversation().cooldowns;
    assert_eq!(log[0].seconds, 1);
    assert!(log[0].reason.contains("asks us to wait 1s"), "{}", log[0].reason);
}

// ---- keeping a steady pace --------------------------------------------------------------------

fn gaps(sends: &[Instant]) -> Vec<Duration> {
    sends.windows(2).map(|pair| pair[1] - pair[0]).collect()
}

#[tokio::test]
async fn requests_are_spaced_at_least_the_interval_apart() {
    let options = RunOptions { min_interval: Some(Duration::from_millis(80)), ..fast() };
    let bot = Bot::new(vec![Ok("A"), Ok("B"), Ok("C")]);

    run_with(questions(3), "http://mock.local", &bot, store(), &options).await.unwrap();

    let gaps = gaps(&bot.sends());
    assert_eq!(gaps.len(), 2);
    assert!(gaps.iter().all(|gap| *gap >= Duration::from_millis(75)), "{gaps:?}");
}

#[tokio::test]
async fn the_first_request_and_the_discovery_are_not_delayed() {
    let options = RunOptions { min_interval: Some(Duration::from_secs(30)), ..fast() };
    let bot = Bot::new(vec![Ok("A")]);

    let started = Instant::now();
    run_with(questions(1), "http://mock.local", &bot, store(), &options).await.unwrap();

    assert!(started.elapsed() < Duration::from_secs(2), "{:?}", started.elapsed());
}

#[tokio::test]
async fn a_retry_after_a_cooldown_keeps_to_the_pace_too() {
    let options = RunOptions {
        cooldown: Duration::ZERO,
        min_interval: Some(Duration::from_millis(100)),
        ..RunOptions::default()
    };
    let bot = Bot::new(vec![Err(TransportError::Http { status: 503 }), Ok("A")]);

    run_with(questions(1), "http://mock.local", &bot, store(), &options).await.unwrap();

    let gaps = gaps(&bot.sends());
    assert!(gaps[0] >= Duration::from_millis(95), "{gaps:?}");
}

#[tokio::test]
async fn the_pace_continues_across_a_stop_and_a_restart() {
    let store = store();

    // session one: answers question 1, then is stopped
    let cancel = Cancel::default();
    let trigger = cancel.clone();
    let first = Bot::hooked(vec![Ok("A1")], Box::new(move |_| trigger.cancel()));
    let options = RunOptions { min_interval: Some(Duration::from_millis(250)), cancel, ..fast() };
    run_with(questions(2), "http://mock.local", &first, Arc::clone(&store), &options)
        .await
        .unwrap_err();

    // session two starts straight away: it must not fire at once
    let second = Bot::new(vec![Ok("A2")]);
    let options = RunOptions { min_interval: Some(Duration::from_millis(250)), ..fast() };
    let started = Instant::now();
    run_with(questions(2), "http://mock.local", &second, Arc::clone(&store), &options)
        .await
        .unwrap();

    let waited = second.sends()[0] - started;
    assert!(
        waited >= Duration::from_millis(180),
        "first request of the new session came after {waited:?}"
    );
}

#[tokio::test]
async fn a_stop_ends_a_long_wait_for_the_next_slot() {
    let cancel = Cancel::default();
    let options = RunOptions {
        min_interval: Some(Duration::from_secs(30)),
        cancel: cancel.clone(),
        ..fast()
    };
    let bot = Bot::new(vec![Ok("A1"), Ok("A2")]);
    let store = store();
    cancel_after(&cancel, 150);

    let started = Instant::now();
    let result =
        run_with(questions(2), "http://mock.local", &bot, Arc::clone(&store), &options).await;

    assert!(matches!(result, Err(ApplicationError::Interrupted)));
    assert!(started.elapsed() < Duration::from_secs(3), "{:?}", started.elapsed());
    assert_eq!(store.load().await.unwrap().unwrap().position, 1);
}

#[tokio::test]
async fn without_a_rate_nothing_is_slowed_down() {
    let bot = Bot::new(vec![Ok("A"), Ok("B"), Ok("C")]);

    let started = Instant::now();
    run_with(questions(3), "http://mock.local", &bot, store(), &fast()).await.unwrap();

    assert!(started.elapsed() < Duration::from_millis(500));
}
