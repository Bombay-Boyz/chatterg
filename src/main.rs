use std::{error::Error, path::PathBuf, process::ExitCode, sync::Arc};

use clap::Parser;

use chatterg::{
    application, domain::Questionnaire, output::human, storage::sqlite::SqliteStore,
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

    /// Questionnaire YAML file.
    #[arg(value_name = "QUESTIONS_YAML")]
    questions: PathBuf,

    /// SQLite database path. Re-running with the same path resumes the questionnaire.
    #[arg(long, value_name = "SQLITE_PATH", default_value = "chatterg.db")]
    store: PathBuf,
}

async fn run(cli: Cli) -> Result<(), Box<dyn Error>> {
    let questionnaire = Questionnaire::from_path(&cli.questions)?;
    let store = Arc::new(SqliteStore::open(&cli.store)?);
    let transport = A2aTransport::new();

    let engine = application::run(questionnaire, &cli.target, &transport, store).await?;

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
