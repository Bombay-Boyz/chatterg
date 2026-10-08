use std::{
    error::Error,
    path::{Path, PathBuf},
    process::ExitCode,
    sync::Arc,
    time::Duration,
};

use clap::{Args, Parser, Subcommand, ValueEnum};

use chatterg::{
    application::{self, ApplicationError, Progress, RunOptions},
    domain::{EngineState, QuestionDefaults, Questionnaire},
    output::{
        human,
        report::{self, Format, Report},
    },
    storage::{Store, sqlite::SqliteStore},
    transport::a2a::A2aTransport,
};

#[derive(Debug, Parser)]
#[command(name = "chatterg")]
#[command(about = "Deterministic bot-to-bot questionnaire client")]
#[command(
    args_conflicts_with_subcommands = true,
    subcommand_negates_reqs = true,
    subcommand_precedence_over_arg = true
)]
#[command(after_help = "\
EXIT CODES:
  0  every question was processed (some answers may still be marked rejected)
  1  error (bad arguments or file, network, protocol, storage)
  2  run ended early because a question failed with on_failure abort/unknown
  3  gave up: the agent stayed unavailable for --max-waits cooldowns in a row
")]
struct Cli {
    #[command(subcommand)]
    command: Option<Command>,

    /// The normal run (`chatterg <AGENT_URL> <QUESTIONS_FILE>`). Arguments are
    /// optional here only so that `chatterg report ...` can be used instead;
    /// clap still insists on them when no subcommand is given.
    #[command(flatten)]
    run: Option<RunArgs>,
}

#[derive(Debug, Subcommand)]
enum Command {
    /// Write a report (Markdown, HTML, CSV or JSON) from a finished or partial run.
    Report(ReportArgs),
}

#[derive(Debug, Args)]
struct ReportArgs {
    /// The notebook to read (the --store used for the run). It is opened read-only,
    /// so this is safe while a run is in progress.
    #[arg(long, value_name = "SQLITE_PATH", default_value = "chatterg.db")]
    store: PathBuf,

    /// Output format. If left out it is guessed from the --out file extension
    /// (.md, .html, .csv, .json); otherwise Markdown.
    #[arg(long, value_enum)]
    format: Option<ReportFormat>,

    /// Write the report to this file instead of the screen.
    #[arg(long, value_name = "FILE")]
    out: Option<PathBuf>,

    /// Replace the --out file if it already exists.
    #[arg(long, requires = "out")]
    overwrite: bool,

    /// The questions file used for the run. Adds section headings and lists
    /// questions that were never asked. It must match the run's questions.
    #[arg(long, value_name = "QUESTIONS_FILE")]
    questions: Option<PathBuf>,
}

#[derive(Debug, Clone, Copy, ValueEnum)]
enum ReportFormat {
    Md,
    Html,
    Csv,
    Json,
}

impl From<ReportFormat> for Format {
    fn from(format: ReportFormat) -> Self {
        match format {
            ReportFormat::Md => Format::Markdown,
            ReportFormat::Html => Format::Html,
            ReportFormat::Csv => Format::Csv,
            ReportFormat::Json => Format::Json,
        }
    }
}

#[derive(Debug, Args)]
struct RunArgs {
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

    /// Write a report when the run finishes (every question processed). The format
    /// comes from the file ending: .md, .html, .csv or .json. Repeat for several
    /// formats. Missing folders are created.
    #[arg(long = "report", value_name = "FILE")]
    reports: Vec<PathBuf>,

    /// Allow --report to replace files that already exist.
    #[arg(long, requires = "reports")]
    overwrite: bool,
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

/// Writes `text` to `path`, creating missing folders. An existing file is only
/// replaced when `overwrite` is set; the check and the write are one atomic step.
fn write_file(path: &Path, text: &str, overwrite: bool) -> Result<(), String> {
    use std::io::Write;

    if let Some(parent) = path.parent().filter(|parent| !parent.as_os_str().is_empty()) {
        std::fs::create_dir_all(parent)
            .map_err(|error| format!("cannot create the folder {}: {error}", parent.display()))?;
    }

    let mut options = std::fs::OpenOptions::new();
    options.write(true);
    if overwrite {
        options.create(true).truncate(true);
    } else {
        options.create_new(true);
    }

    let mut file = options.open(path).map_err(|error| {
        if error.kind() == std::io::ErrorKind::AlreadyExists {
            format!("{} already exists; choose another name or add --overwrite", path.display())
        } else {
            format!("cannot write {}: {error}", path.display())
        }
    })?;

    file.write_all(text.as_bytes())
        .map_err(|error| format!("cannot write {}: {error}", path.display()))
}

/// True when both paths name the same existing file.
fn same_file(a: &Path, b: &Path) -> bool {
    match (std::fs::canonicalize(a), std::fs::canonicalize(b)) {
        (Ok(a), Ok(b)) => a == b,
        _ => false,
    }
}

/// Checks every `--report` before anything is asked, so a mistake costs seconds
/// instead of a finished run.
fn plan_reports(args: &RunArgs) -> Result<Vec<(PathBuf, Format)>, String> {
    let mut plan: Vec<(PathBuf, Format)> = Vec::new();

    for path in &args.reports {
        let format = Format::from_path(path).ok_or_else(|| {
            format!(
                "cannot tell the report format from {}; end the name with .md, .html, .csv or .json",
                path.display()
            )
        })?;

        if plan.iter().any(|(planned, _)| planned == path) {
            return Err(format!("--report {} was given twice", path.display()));
        }

        if path.is_dir() {
            return Err(format!("{} is a folder, not a file", path.display()));
        }

        if same_file(path, &args.questions) {
            return Err(format!("--report {} would overwrite your questions file", path.display()));
        }

        if same_file(path, &args.store) {
            return Err(format!("--report {} would overwrite your notebook", path.display()));
        }

        if path.exists() && !args.overwrite {
            return Err(format!(
                "the report file {} already exists; choose another name or add --overwrite",
                path.display()
            ));
        }

        plan.push((path.clone(), format));
    }

    Ok(plan)
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

async fn run(cli: RunArgs) -> Result<Outcome, Box<dyn Error>> {
    let planned_reports = plan_reports(&cli)?;

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
        application::run_with(questionnaire.clone(), &cli.target, &transport, store, &options)
            .await?;

    print!("{}", human::render(engine.conversation()));

    if engine.start() == EngineState::Aborted {
        eprintln!(
            "the run ended early: a question failed and its on_failure policy is abort/unknown"
        );
        return Ok(Outcome::EndedEarly);
    }

    // Every question has been processed: say so, and write the requested reports.
    let finished = Report::build(engine.conversation(), Some(&questionnaire));
    eprintln!("{}", finished.done_note());

    let mut failures = Vec::new();
    for (path, format) in &planned_reports {
        match write_file(path, &finished.render(*format), cli.overwrite) {
            Ok(()) => eprintln!("Report: {}", path.display()),
            Err(error) => failures.push(error),
        }
    }

    if !failures.is_empty() {
        return Err(format!(
            "the run finished and is saved in {}, but a report could not be written: {}. \
             You can make it later with: chatterg report --store {}",
            cli.store.display(),
            failures.join("; "),
            cli.store.display()
        )
        .into());
    }

    Ok(Outcome::Completed)
}

async fn make_report(args: ReportArgs) -> Result<Outcome, Box<dyn Error>> {
    let store = SqliteStore::open_read_only(&args.store)?;

    let conversation = store.load().await?.ok_or_else(|| {
        format!("the notebook {} does not contain a run yet", args.store.display())
    })?;

    let questionnaire = match &args.questions {
        Some(path) => {
            let questionnaire = Questionnaire::from_path(path)?;
            report::check_questions(&conversation, &questionnaire)?;
            Some(questionnaire)
        }
        None => None,
    };

    let format = args
        .format
        .map(Format::from)
        .or_else(|| args.out.as_deref().and_then(Format::from_path))
        .unwrap_or(Format::Markdown);

    let text = Report::build(&conversation, questionnaire.as_ref()).render(format);

    match &args.out {
        Some(path) => {
            write_file(path, &text, args.overwrite)?;
            eprintln!("wrote {}", path.display());
        }
        None => print!("{text}"),
    }

    Ok(Outcome::Completed)
}

#[tokio::main]
async fn main() -> ExitCode {
    let cli = Cli::parse();

    let result = match cli.command {
        Some(Command::Report(args)) => make_report(args).await,
        None => match cli.run {
            Some(args) => run(args).await,
            None => Err("missing arguments: expected <AGENT_URL> <QUESTIONS_FILE>".into()),
        },
    };

    match result {
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
