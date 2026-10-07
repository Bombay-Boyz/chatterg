use std::{
    error::Error,
    path::{Path, PathBuf},
    process::ExitCode,
    sync::Arc,
    time::Duration,
};

use clap::Parser;

use chatterg::{
    application::{self, ApplicationError, Progress, RunOptions},
    domain::{EngineState, QuestionDefaults, Questionnaire},
    output::human,
    storage::sqlite::SqliteStore,
    transport::a2a::A2aTransport,
};

#[derive(Debug, Parser)]
#[command(name = "chatterg")]
#[command(about = "Deterministic bot-to-bot questionnaire client")]
#[command(after_help = "\
EXIT CODES:
  0  every question was processed (some answers may still be marked rejected)
  1  error (bad arguments or file, network, protocol, storage)
  2  run ended early because a question failed with on_failure abort/unknown
  3  gave up: the agent stayed unavailable for --max-waits cooldowns in a row
")]
struct Cli {
    /// Target A2A agent URL (the agent card is fetched from
    /// `<AGENT_URL>/.well-known/agent-card.json`).
    #[arg(value_name = "AGENT_URL")]
    target: String,

    /// Questions file. `.yaml`/`.yml`: a list of questions (plain strings or full
    /// objects). Any other extension (e.g. `.txt`): one question per line.
    #[arg(value_name = "QUESTIONS_FILE")]
    questions: PathBuf,

    /// Extra retries per plain-text question when the answer is evasive/rejected
    /// (total asks = RETRIES + 1). Does not override fully specified questions.
    #[arg(long, value_name = "RETRIES", default_value_t = 2)]
    retries: usize,

    /// Phrase that marks an answer as evasive (case-insensitive). Repeatable;
    /// replaces the built-in list (meeting, schedule a call, calendly, ...).
    #[arg(long = "reject-phrase", value_name = "PHRASE")]
    reject_phrases: Vec<String>,

    /// Seconds to pause when the agent stops answering (rate limit, outage,
    /// timeout) before re-sending the same question.
    #[arg(long, value_name = "SECONDS", default_value_t = 120)]
    cooldown: u64,

    /// Give up after this many consecutive cooldowns for one message.
    #[arg(long, value_name = "N", default_value_t = 30)]
    max_waits: usize,

    /// Seconds to pause between questions, to stay under rate limits.
    #[arg(long, value_name = "SECONDS", default_value_t = 0)]
    delay: u64,

    /// Seconds to wait for each agent reply before treating it as unavailable.
    #[arg(long, value_name = "SECONDS", default_value_t = 120)]
    timeout: u64,

    /// Short reply text (case-insensitive) meaning "rate limited". Repeatable;
    /// replaces the built-in list (rate limit, too many requests, ...).
    #[arg(long = "rate-limit-phrase", value_name = "PHRASE")]
    rate_limit_phrases: Vec<String>,

    /// SQLite database path. Re-running with the same path resumes the questionnaire.
    #[arg(long, value_name = "SQLITE_PATH", default_value = "chatterg.db")]
    store: PathBuf,

    /// Continue a run even though the questions file was edited, as long as every
    /// question already asked is unchanged (edits to later questions are fine).
    #[arg(long, conflicts_with = "restart")]
    force_resume: bool,

    /// Archive the stored run to `<STORE>.<timestamp>.bak` and start again from the
    /// first question.
    #[arg(long)]
    restart: bool,
}

/// How a run that did not fail ended.
enum Outcome {
    Completed,
    EndedEarly,
}

const EXIT_ENDED_EARLY: u8 = 2;
const EXIT_AGENT_UNAVAILABLE: u8 = 3;

fn backup_path(store: &Path) -> PathBuf {
    let mut name = store.as_os_str().to_owned();
    name.push(format!(".{}.bak", chrono::Utc::now().format("%Y%m%dT%H%M%SZ")));
    PathBuf::from(name)
}

fn report(progress: &Progress) {
    match progress {
        Progress::Asking { number, total, id, attempt } => {
            let retry = if *attempt > 1 { format!(" (attempt {attempt})") } else { String::new() };
            eprintln!("[{number}/{total}] {id}{retry}");
        }
        Progress::Waiting { reason, wait, max_waits, cooldown } => {
            eprintln!(
                "  agent unavailable: {reason}\n  pausing {}s, then resuming (cooldown {wait}/{max_waits})",
                cooldown.as_secs()
            );
        }
    }
}

async fn run(cli: Cli) -> Result<Outcome, Box<dyn Error>> {
    let mut defaults =
        QuestionDefaults { max_followups: cli.retries, ..QuestionDefaults::default() };
    if !cli.reject_phrases.is_empty() {
        defaults.reject_if_contains = cli.reject_phrases.clone();
    }

    let questionnaire = Questionnaire::from_path_with(&cli.questions, &defaults)?;
    let store = Arc::new(SqliteStore::open(&cli.store)?);

    if cli.restart {
        let backup = backup_path(&cli.store);

        if store.archive_and_reset(&backup)? {
            eprintln!("archived the previous run to {}", backup.display());
        }
    }

    let transport = A2aTransport::with_timeout(Duration::from_secs(cli.timeout));

    let mut options = RunOptions {
        cooldown: Duration::from_secs(cli.cooldown),
        max_waits: cli.max_waits,
        delay: Duration::from_secs(cli.delay),
        force_resume: cli.force_resume,
        on_progress: Some(Arc::new(report)),
        ..RunOptions::default()
    };
    if !cli.rate_limit_phrases.is_empty() {
        options.rate_limit_phrases = cli.rate_limit_phrases.clone();
    }

    let engine =
        application::run_with(questionnaire, &cli.target, &transport, store, &options).await?;

    print!("{}", human::render(engine.conversation()));

    if engine.start() == EngineState::Aborted {
        eprintln!(
            "the run ended early: a question failed and its on_failure policy is abort/unknown"
        );
        return Ok(Outcome::EndedEarly);
    }

    Ok(Outcome::Completed)
}

#[tokio::main]
async fn main() -> ExitCode {
    let cli = Cli::parse();

    match run(cli).await {
        Ok(Outcome::Completed) => ExitCode::SUCCESS,
        Ok(Outcome::EndedEarly) => ExitCode::from(EXIT_ENDED_EARLY),
        Err(error) => {
            eprintln!("error: {error}");

            let mut source = error.source();
            while let Some(cause) = source {
                eprintln!("  caused by: {cause}");
                source = cause.source();
            }

            match error.downcast_ref::<ApplicationError>() {
                Some(ApplicationError::AgentUnavailable { .. }) => {
                    ExitCode::from(EXIT_AGENT_UNAVAILABLE)
                }
                _ => ExitCode::FAILURE,
            }
        }
    }
}
