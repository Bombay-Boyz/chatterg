use std::{future::Future, sync::Arc, time::Duration};

use chrono::Utc;
use thiserror::Error;
use url::Url;

use crate::{
    domain::{
        AgentInfo, CooldownEvent, DomainError, Engine, EngineState, Question, Questionnaire,
        Submission, Timestamp, Timing,
    },
    storage::{StorageError, Store},
    transport::{Message, Transport, TransportError},
};

/// Replies shorter than this may be a rate-limit notice; longer ones are real
/// answers (which can legitimately mention e.g. a "rate-limiting step").
const RATE_LIMIT_NOTICE_MAX_LEN: usize = 400;

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
}

/// Something worth telling a human who is watching the run.
#[derive(Debug, Clone)]
pub enum Progress {
    Asking { number: usize, total: usize, id: String, attempt: usize },
    Waiting { reason: String, wait: usize, max_waits: usize, cooldown: Duration },
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

    fn is_transient(&self, error: &TransportError) -> bool {
        error.is_transient()
            || matches!(error, TransportError::Protocol { message, .. } if self.looks_rate_limited(message))
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

    let mut events = Vec::new();
    let discovered =
        with_cooldown(options, &mut events, || transport.discover(&target), |_| None).await;
    engine.record_cooldowns(std::mem::take(&mut events));

    let capabilities = match discovered {
        Ok((capabilities, _)) => capabilities,
        Err(error) => {
            store.save(engine.conversation()).await?;
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
                store.save(engine.conversation()).await?;
                return Ok(engine);
            }
        };

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
                store.save(engine.conversation()).await?;
                return Err(error);
            }
        };

        match engine.submit_timed(response.text, timing) {
            Submission::Next(next, _) | Submission::Retry(next, _) => {
                store.save(next.conversation()).await?;
                engine = next;

                if !options.delay.is_zero() {
                    tokio::time::sleep(options.delay).await;
                }
            }

            Submission::Aborted(next) | Submission::Unknown(next) | Submission::Complete(next) => {
                let next = next.finish(options.now());
                store.save(next.conversation()).await?;
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
    events: &mut Vec<CooldownEvent>,
    mut operation: F,
    rejected: impl Fn(&T) -> Option<String>,
) -> Result<(T, Timing), ApplicationError>
where
    F: FnMut() -> Fut,
    Fut: Future<Output = Result<T, TransportError>>,
{
    let mut waits = 0;

    loop {
        let sent_at = options.now();
        let outcome = operation().await;
        let received_at = options.now();

        let reason = match outcome {
            Ok(value) => match rejected(&value) {
                None => return Ok((value, Timing::new(sent_at, received_at))),
                Some(reason) => reason,
            },
            Err(error) if options.is_transient(&error) => error.to_string(),
            Err(error) => return Err(error.into()),
        };

        if waits >= options.max_waits {
            return Err(ApplicationError::AgentUnavailable { waits, last: reason });
        }

        waits += 1;
        events.push(CooldownEvent {
            at: received_at,
            reason: reason.clone(),
            seconds: options.cooldown.as_secs(),
        });
        options.notify(Progress::Waiting {
            reason,
            wait: waits,
            max_waits: options.max_waits,
            cooldown: options.cooldown,
        });

        tokio::time::sleep(options.cooldown).await;
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
