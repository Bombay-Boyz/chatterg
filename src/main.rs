use clap::Parser;

use chatterg::domain::Conversation;

#[derive(Debug, Parser)]
#[command(name = "chatterg")]
#[command(about = "Deterministic bot-to-bot questionnaire client")]
struct Cli {
    /// Target agent URL.
    target: String,

    /// Questionnaire file.
    questions: String,
}

fn main() {
    let cli = Cli::parse();

    println!("ChatterG");
    println!("Target: {}", cli.target);
    println!("Questions: {}", cli.questions);

    let conversation = Conversation::new();

    println!("Initial state: {:?}", conversation.state);
}
