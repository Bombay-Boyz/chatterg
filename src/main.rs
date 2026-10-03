use std::sync::Arc;

use clap::Parser;

use chatterg::{
    application,
    domain::{Engine, Questionnaire},
    output::human,
    storage::sqlite::SqliteStore,
    transport::a2a::A2aTransport,
};

#[derive(Debug, Parser)]
#[command(name = "chatterg")]
#[command(about = "Deterministic bot-to-bot questionnaire client")]
struct Cli {
    /// Target A2A agent URL.
    target: String,

    /// Questionnaire YAML file.
    questions: String,

    /// SQLite database path.
    #[arg(long, default_value = "chatterg.db")]
    store: String,
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let cli = Cli::parse();

    let questionnaire = Questionnaire::from_path(&cli.questions)?;

    if questionnaire.is_empty() {
        return Err("questionnaire contains no questions".into());
    }

    let engine = Engine::new(questionnaire);
    let transport = A2aTransport::new();
    let store = Arc::new(SqliteStore::open(&cli.store)?);

    let engine = application::run(engine, &cli.target, &transport, Arc::clone(&store)).await?;

    print!("{}", human::render(engine.conversation()));

    Ok(())
}
