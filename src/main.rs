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
    /// Target agent URL.
    target: String,

    /// Questionnaire file.
    questions: String,
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let cli = Cli::parse();

    let questionnaire = Questionnaire::from_path(&cli.questions)?;
    let engine = Engine::new(questionnaire);

    let transport = A2aTransport::new();
    let store = Arc::new(SqliteStore::open("chatterg.db")?);

    let engine = application::run(engine, &cli.target, &transport, store).await?;

    print!("{}", human::render(engine.conversation()));

    Ok(())
}
