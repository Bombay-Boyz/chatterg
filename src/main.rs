use std::{error::Error, path::PathBuf, process::ExitCode, sync::Arc, time::Duration};

use clap::Parser;

use chatterg::{
    application::{self, Progress, RunOptions},
    domain::{QuestionDefaults, Questionnaire},
    output::human,
    storage::sqlite::SqliteStore,
    transport::a2a::A2aTransport,
};

#[derive(Debug, Parser)]
#[command(name = "chatterg")]
#[command(about = "Deterministic bot-to-bot questionnaire client")]
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

async fn run(cli: Cli) -> Result<(), Box<dyn Error>> {
    let mut defaults =
        QuestionDefaults { max_followups: cli.retries, ..QuestionDefaults::default() };
    if !cli.reject_phrases.is_empty() {
        defaults.reject_if_contains = cli.reject_phrases.clone();
    }

    let questionnaire = Questionnaire::from_path_with(&cli.questions, &defaults)?;
    let store = Arc::new(SqliteStore::open(&cli.store)?);
    let transport = A2aTransport::with_timeout(Duration::from_secs(cli.timeout));

    let mut options = RunOptions {
        cooldown: Duration::from_secs(cli.cooldown),
        max_waits: cli.max_waits,
        delay: Duration::from_secs(cli.delay),
        on_progress: Some(Arc::new(report)),
        ..RunOptions::default()
    };
    if !cli.rate_limit_phrases.is_empty() {
        options.rate_limit_phrases = cli.rate_limit_phrases.clone();
    }

    let engine =
        application::run_with(questionnaire, &cli.target, &transport, store, &options).await?;

    print!("{}", human::render(engine.conversation()));

    Ok(())
}

#[tokio::main]
async fn main() -> ExitCode {
    let cli = Cli::parse();

    match run(cli).await {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("error: {error}");

            let mut source = error.source();
            while let Some(cause) = source {
                eprintln!("  caused by: {cause}");
                source = cause.source();
            }

            ExitCode::FAILURE
        }
    }
}
