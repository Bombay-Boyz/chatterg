use std::{
    future::Future,
    sync::{Arc, Mutex, PoisonError},
    time::Duration,
};

use chrono::Utc;
use thiserror::Error;
use url::Url;

use crate::{
    cancel::Cancel,
    domain::{
        AgentInfo, Conversation, CooldownEvent, DomainError, Engine, EngineState, Question,
        Questionnaire, Submission, Timestamp, Timing,
    },
    storage::{StorageError, Store},
    transport::{Message, Transport, TransportError},
};

/// Replies shorter than this may be a rate-limit notice; longer ones are real
/// answers (which can legitimately mention e.g. a "rate-limiting step").
const RATE_LIMIT_NOTICE_MAX_LEN: usize = 400;

/// The longest pause the outage back-off grows to.
const OUTAGE_PAUSE_CAP: Duration = Duration::from_secs(600);

/// How many times saving progress is tried before the run stops.
const SAVE_ATTEMPTS: usize = 3;

pub const DEFAULT_RATE_LIMIT_PHRASES: &[&str] =
    &["rate limit", "rate-limit", "too many requests", "try again later", "slow down"];

#[derive(Debug, Error)]
pub enum ApplicationError {
    #[error("invalid target URL: {0}")]
    InvalidTarget(#[from] url::ParseError),

    #[error(transparent)]
    Domain(#[from] DomainError),

    #[error(transparent)]
    Transport(#[from] TransportError),

    #[error(transparent)]
    Storage(#[from] StorageError),

    #[error("agent still unavailable after {waits} cooldowns; last problem: {last}")]
    AgentUnavailable { waits: usize, last: String },

    /// The user asked to stop (Ctrl+C). Everything answered so far is saved.
    #[error("stopped at the user's request")]
    Interrupted,

    #[error(
        "could not save your progress after {attempts} tries: {source}. Free some disk space or \
         check that the folder can be written to, then run the same command again. Everything \
         saved earlier is safe."
    )]
    CouldNotSave {
        attempts: usize,
        #[source]
        source: StorageError,
    },
}

/// Something worth telling a human who is watching the run.
#[derive(Debug, Clone)]
pub enum Progress {
    Asking {
        number: usize,
        total: usize,
        id: String,
        attempt: usize,
    },
    Waiting {
        reason: String,
        wait: usize,
        max_waits: usize,
        cooldown: Duration,
    },
    /// A request finally went through after `waits` pauses totalling `paused`.
    Resumed {
        waits: usize,
        paused: Duration,
    },
    /// Something went wrong that did not stop the run (for example a backup that
    /// could not be written).
    Warning(String),
}

pub type ProgressCallback = Arc<dyn Fn(&Progress) + Send + Sync>;

/// Source of "now". Injectable so tests get deterministic timestamps.
pub type Clock = Arc<dyn Fn() -> Timestamp + Send + Sync>;

#[derive(Clone)]
pub struct RunOptions {
    /// Pause when the agent stops answering (rate limit, outage, timeout).
    pub cooldown: Duration,
    /// Consecutive cooldowns tolerated for one message before giving up.
    pub max_waits: usize,
    /// Pause between questions, to avoid hitting rate limits in the first place.
    pub delay: Duration,
    /// Short replies containing one of these (case-insensitive) are treated as
    /// "rate limited", not as an answer.
    pub rate_limit_phrases: Vec<String>,
    pub on_progress: Option<ProgressCallback>,

    /// Continue a run even though the questions file changed, provided every
    /// question already asked is unchanged.
    pub force_resume: bool,

    pub clock: Clock,

    /// The longest pause the *server* may ask for (`Retry-After` and friends). A
    /// `cooldown` you set yourself is never shortened by this.
    pub max_wait: Duration,

    /// The least time to leave between requests, so a run stays under a known
    /// limit instead of hitting it. Set from `--rate "30/hour"`.
    pub min_interval: Option<Duration>,

    /// Set this to stop the run cleanly: the question being asked is finished and
    /// saved, then the run returns `ApplicationError::Interrupted`.
    pub cancel: Cancel,

    /// How long to wait between tries when saving progress fails (disk full, a
    /// network drive that blinked). Saving is tried three times before giving up.
    pub save_retry_pause: Duration,
}

impl Default for RunOptions {
    fn default() -> Self {
        Self {
            cooldown: Duration::from_secs(120),
            max_waits: 30,
            delay: Duration::ZERO,
            rate_limit_phrases: DEFAULT_RATE_LIMIT_PHRASES
                .iter()
                .map(|p| (*p).to_owned())
                .collect(),
            on_progress: None,
            force_resume: false,
            clock: Arc::new(Utc::now),
            max_wait: Duration::from_secs(3600),
            min_interval: None,
            cancel: Cancel::default(),
            save_retry_pause: Duration::from_secs(1),
        }
    }
}

impl RunOptions {
    fn now(&self) -> Timestamp {
        (self.clock)()
    }

    fn notify(&self, progress: Progress) {
        if let Some(callback) = &self.on_progress {
            callback(&progress);
        }
    }

    fn looks_rate_limited(&self, text: &str) -> bool {
        if text.len() > RATE_LIMIT_NOTICE_MAX_LEN {
            return false;
        }

        let lowered = text.to_lowercase();
        self.rate_limit_phrases.iter().any(|phrase| lowered.contains(&phrase.to_lowercase()))
    }

    /// How long to pause after a failure: our own cooldown, or longer if the
    /// server asked for longer (but never more than `max_wait` for a server's wish).
    ///
    /// While the connection itself keeps failing (`outage_streak` failures in a row)
    /// the pause doubles each time, up to ten minutes, so a long outage is not
    /// hammered every two minutes.
    pub fn pause_for(&self, server_hint: Option<Duration>, outage_streak: usize) -> Duration {
        let base = if outage_streak > 1 {
            let doublings = u32::try_from(outage_streak - 1).unwrap_or(u32::MAX).min(10);

            self.cooldown.saturating_mul(1 << doublings).min(self.cooldown.max(OUTAGE_PAUSE_CAP))
        } else {
            self.cooldown
        };

        let wanted = base.max(server_hint.unwrap_or_default());

        wanted.min(self.max_wait.max(base))
    }

    fn is_transient(&self, error: &TransportError) -> bool {
        error.is_transient()
            || matches!(error, TransportError::Protocol { message, .. } if self.looks_rate_limited(message))
    }
}

/// Saves progress, trying again a couple of times if the disk hiccups. Warnings the
/// store collected (a backup that failed, say) are passed on to the user.
async fn persist<S: Store>(
    store: &S,
    conversation: &Conversation,
    options: &RunOptions,
) -> Result<(), ApplicationError> {
    let mut attempt = 1;

    loop {
        match store.save(conversation).await {
            Ok(()) => {
                for warning in store.take_warnings() {
                    options.notify(Progress::Warning(warning));
                }
                return Ok(());
            }
            Err(source) if attempt >= SAVE_ATTEMPTS => {
                return Err(ApplicationError::CouldNotSave { attempts: attempt, source });
            }
            Err(_) => {
                attempt += 1;
                pause(options, options.save_retry_pause).await?;
            }
        }
    }
}

/// Turns a rate such as `30/hour` into the gap to leave between requests (here
/// two minutes). Units: second, minute, hour, day (and their short forms).
pub fn parse_rate(text: &str) -> Result<Duration, String> {
    let bad = || format!("expected a rate like 10/minute, 30/hour or 100/day, not \"{text}\"");

    let (count, unit) = text.split_once('/').ok_or_else(bad)?;
    let count: u32 = count.trim().parse().map_err(|_| bad())?;

    if count == 0 {
        return Err(bad());
    }

    let seconds: u64 = match unit.trim().to_ascii_lowercase().as_str() {
        "s" | "sec" | "second" | "seconds" => 1,
        "m" | "min" | "minute" | "minutes" => 60,
        "h" | "hr" | "hour" | "hours" => 3600,
        "d" | "day" | "days" => 86_400,
        _ => return Err(bad()),
    };

    Ok(Duration::from_secs(seconds) / count)
}

/// Sleeps, but wakes at once if the user asks to stop.
async fn pause(options: &RunOptions, duration: Duration) -> Result<(), ApplicationError> {
    tokio::select! {
        biased;
        () = options.cancel.cancelled() => Err(ApplicationError::Interrupted),
        () = tokio::time::sleep(duration) => Ok(()),
    }
}

/// Keeps requests at least `interval` apart.
struct Pacer {
    interval: Duration,
    next: Mutex<tokio::time::Instant>,
}

impl Pacer {
    /// `last_sent` is when the previous session sent its last request, so a run that
    /// was stopped a moment ago does not start again straight away.
    fn new(options: &RunOptions, last_sent: Option<Timestamp>) -> Option<Self> {
        let interval = options.min_interval?;

        let owed = last_sent
            .and_then(|sent| {
                let due = sent + chrono::Duration::from_std(interval).ok()?;
                (due - options.now()).to_std().ok()
            })
            .unwrap_or(Duration::ZERO);

        Some(Self { interval, next: Mutex::new(tokio::time::Instant::now() + owed) })
    }

    async fn wait_turn(&self, options: &RunOptions) -> Result<(), ApplicationError> {
        let wait = {
            let mut next = self.next.lock().unwrap_or_else(PoisonError::into_inner);
            let now = tokio::time::Instant::now();
            let slot = (*next).max(now);

            *next = slot + self.interval;
            slot - now
        };

        pause(options, wait).await
    }
}

/// Runs (or resumes) the questionnaire with default options.
pub async fn run<T, S>(
    questionnaire: Questionnaire,
    target: &str,
    transport: &T,
    store: Arc<S>,
) -> Result<Engine, ApplicationError>
where
    T: Transport,
    S: Store + 'static,
{
    run_with(questionnaire, target, transport, store, &RunOptions::default()).await
}

/// Runs (or resumes) the questionnaire.
///
/// If the store already holds a conversation the engine is rebuilt from it and
/// continues from the persisted position. When the agent stops answering the
/// run pauses for `options.cooldown` and re-sends the same message, up to
/// `options.max_waits` times in a row.
pub async fn run_with<T, S>(
    questionnaire: Questionnaire,
    target: &str,
    transport: &T,
    store: Arc<S>,
    options: &RunOptions,
) -> Result<Engine, ApplicationError>
where
    T: Transport,
    S: Store + 'static,
{
    let target = Url::parse(target)?;

    let mut engine = match store.load().await? {
        Some(conversation) if options.force_resume => {
            Engine::resume_forced(questionnaire, conversation)?
        }
        Some(conversation) => Engine::resume(questionnaire, conversation)?,
        None => Engine::new(questionnaire),
    };

    if !matches!(engine.start(), EngineState::Asking(_)) {
        return Ok(engine);
    }

    engine.begin(options.now());

    let last_sent = engine
        .conversation()
        .answers
        .iter()
        .flat_map(|record| record.attempts.iter())
        .filter_map(|attempt| attempt.sent_at)
        .max();
    let pacer = Pacer::new(options, last_sent);

    let mut events = Vec::new();
    let discovered =
        with_cooldown(options, None, &mut events, || transport.discover(&target), |_| None).await;
    engine.record_cooldowns(std::mem::take(&mut events));

    let capabilities = match discovered {
        Ok((capabilities, _)) => capabilities,
        Err(error) => {
            persist(store.as_ref(), engine.conversation(), options).await?;
            return Err(error);
        }
    };

    let endpoint = capabilities.endpoint.clone();
    engine.set_agent(AgentInfo {
        name: capabilities.agent_name.clone(),
        target: target.to_string(),
        endpoint: endpoint.to_string(),
        protocol: capabilities.protocols.first().map_or("unknown", |p| p.as_str()).to_owned(),
    });

    let total = engine.total_questions();

    loop {
        let question = match engine.start() {
            EngineState::Asking(question) => question,
            EngineState::Ready | EngineState::Complete | EngineState::Aborted => {
                let engine = engine.finish(options.now());
                persist(store.as_ref(), engine.conversation(), options).await?;
                return Ok(engine);
            }
        };

        if options.cancel.is_cancelled() {
            persist(store.as_ref(), engine.conversation(), options).await?;
            return Err(ApplicationError::Interrupted);
        }

        let attempt =
            engine.conversation().answer(&question.id).map_or(0, |record| record.attempts.len())
                + 1;

        options.notify(Progress::Asking {
            number: engine.position() + 1,
            total,
            id: question.id.as_str().to_owned(),
            attempt,
        });

        let message = Message {
            id: format!("chatterg-{}", engine.conversation().next_message_number()),
            text: next_text(&engine, &question),
        };

        // The same message (and id) is re-sent after a cooldown: it was never answered.
        let sent = with_cooldown(
            options,
            pacer.as_ref(),
            &mut events,
            || transport.send(&endpoint, message.clone()),
            |response| {
                options.looks_rate_limited(&response.text).then(|| {
                    format!("agent replied with a rate-limit notice: {}", response.text.trim())
                })
            },
        )
        .await;

        engine.record_cooldowns(std::mem::take(&mut events));

        let (response, timing) = match sent {
            Ok(done) => done,
            Err(error) => {
                // Keep the cooldown log even though the run is giving up.
                persist(store.as_ref(), engine.conversation(), options).await?;
                return Err(error);
            }
        };

        match engine.submit_timed(response.text, timing) {
            Submission::Next(next, _) | Submission::Retry(next, _) => {
                persist(store.as_ref(), next.conversation(), options).await?;
                engine = next;

                if !options.delay.is_zero() {
                    pause(options, options.delay).await?;
                }
            }

            Submission::Aborted(next) | Submission::Unknown(next) | Submission::Complete(next) => {
                let next = next.finish(options.now());
                persist(store.as_ref(), next.conversation(), options).await?;
                return Ok(next);
            }
        }
    }
}

/// Calls `operation` until it yields a usable result, returning it with the
/// times just before the call and just after it. Transient failures and replies
/// flagged by `rejected` cause a cooldown (logged in `events`) and another try;
/// anything else is returned immediately.
async fn with_cooldown<T, F, Fut>(
    options: &RunOptions,
    pacer: Option<&Pacer>,
    events: &mut Vec<CooldownEvent>,
    mut operation: F,
    rejected: impl Fn(&T) -> Option<String>,
) -> Result<(T, Timing), ApplicationError>
where
    F: FnMut() -> Fut,
    Fut: Future<Output = Result<T, TransportError>>,
{
    let mut waits = 0;
    let mut outage_streak = 0;
    let mut paused = Duration::ZERO;

    loop {
        if options.cancel.is_cancelled() {
            return Err(ApplicationError::Interrupted);
        }
        if let Some(pacer) = pacer {
            pacer.wait_turn(options).await?;
        }

        let sent_at = options.now();
        let outcome = operation().await;
        let received_at = options.now();

        let (reason, server_hint) = match outcome {
            Ok(value) => match rejected(&value) {
                None => {
                    if waits > 0 {
                        options.notify(Progress::Resumed { waits, paused });
                    }
                    return Ok((value, Timing::new(sent_at, received_at)));
                }
                Some(reason) => {
                    outage_streak = 0;
                    (reason, None)
                }
            },
            Err(error) if options.is_transient(&error) => {
                outage_streak = if error.is_outage() { outage_streak + 1 } else { 0 };

                let hint = match &error {
                    TransportError::RetryLater { retry_after, .. } => Some(*retry_after),
                    _ => None,
                };
                (error.to_string(), hint)
            }
            Err(error) => return Err(error.into()),
        };

        if waits >= options.max_waits {
            return Err(ApplicationError::AgentUnavailable { waits, last: reason });
        }

        waits += 1;
        let pause_length = options.pause_for(server_hint, outage_streak);
        paused += pause_length;
        events.push(CooldownEvent {
            at: received_at,
            reason: reason.clone(),
            seconds: pause_length.as_secs(),
        });
        options.notify(Progress::Waiting {
            reason,
            wait: waits,
            max_waits: options.max_waits,
            cooldown: pause_length,
        });

        pause(options, pause_length).await?;
    }
}

/// First attempt sends `question`; retries send `followup` when one is configured.
fn next_text(engine: &Engine, question: &Question) -> String {
    let retrying = engine
        .conversation()
        .answer(&question.id)
        .is_some_and(|answer| !answer.attempts.is_empty());

    match (&question.followup, retrying) {
        (Some(followup), true) => followup.clone(),
        _ => question.question.clone(),
    }
}
