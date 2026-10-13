use std::{
    error::Error,
    path::{Path, PathBuf},
    process::ExitCode,
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};

use clap::{Args, Parser, Subcommand, ValueEnum};

use chatterg::{
    application::{self, ApplicationError, Progress, RunOptions},
    bank,
    cancel::Cancel,
    domain::{Engine, EngineState, QuestionDefaults, Questionnaire, State},
    output::{
        human,
        progress::{Eta, approximately},
        report::{self, Format, Report},
    },
    recovery, runs,
    storage::{
        StorageError, Store,
        backups::{self, BackupPolicy},
        sqlite::SqliteStore,
    },
    transport::a2a::A2aTransport,
};

/// Screen output that can never panic. A closed terminal or a broken pipe
/// (`chatterg list | head`) must not crash a run that is half way through.
macro_rules! say {
    ($($arg:tt)*) => {{
        use std::io::Write as _;
        let _ = writeln!(std::io::stderr(), $($arg)*);
    }};
}

macro_rules! out {
    ($($arg:tt)*) => {{
        use std::io::Write as _;
        let _ = writeln!(std::io::stdout(), $($arg)*);
    }};
}

macro_rules! out_raw {
    ($($arg:tt)*) => {{
        use std::io::Write as _;
        let _ = write!(std::io::stdout(), $($arg)*);
    }};
}

const DEFAULT_QUESTIONS: &str = "questions.txt";
const DEFAULT_RUNS_DIR: &str = "runs";

#[derive(Debug, Parser)]
#[command(name = "chatterg", version)]
#[command(about = "Asks another bot a list of questions and saves the answers as a report")]
#[command(
    args_conflicts_with_subcommands = true,
    subcommand_negates_reqs = true,
    subcommand_precedence_over_arg = true
)]
#[command(after_help = "\
QUICK START:
  chatterg add \"What is a zeolite?\"          put a question in your bank (questions.txt)
  chatterg list                              see the questions in the bank
  chatterg run https://example.com/agent     ask the bot every question, save a report,
                                             and empty the notebook ready for the next run

EXIT CODES:
  0  every question was processed (some answers may still be marked rejected)
  1  error (bad arguments or file, network, protocol, storage)
  2  run ended early because a question failed with on_failure abort/unknown
  3  gave up: the agent stayed unavailable for --max-waits cooldowns in a row
     (your progress is saved; run the same command again later to carry on)
130  stopped with Ctrl+C (the current question is finished first; your progress is
     saved; run the same command again to carry on)
")]
struct Cli {
    #[command(subcommand)]
    command: Option<Command>,

    /// The advanced form: `chatterg <AGENT_URL> <QUESTIONS_FILE> [options]`. The
    /// notebook is kept as it is afterwards. The two values are optional here only so
    /// that the subcommands can be used instead; clap still insists on them when no
    /// subcommand is given.
    #[command(flatten)]
    legacy: Option<Positionals>,

    #[command(flatten)]
    common: CommonArgs,

    /// (Advanced form) Write a report when the run finishes (every question
    /// processed). The format comes from the file ending: .md, .html, .csv or .json.
    /// Repeat for several formats. Missing folders are created.
    #[arg(long = "report", value_name = "FILE")]
    reports: Vec<PathBuf>,

    /// (Advanced form) Allow --report to replace files that already exist.
    #[arg(long, requires = "reports")]
    overwrite: bool,
}

/// The two values of the advanced form.
#[derive(Debug, Args)]
struct Positionals {
    /// Target A2A agent URL (the agent card is fetched from
    /// `<AGENT_URL>/.well-known/agent-card.json`).
    #[arg(value_name = "AGENT_URL")]
    target: String,

    /// Questions file. `.yaml`/`.yml`: a list of questions (plain strings or full
    /// objects). Any other extension (e.g. `.txt`): one question per line.
    #[arg(value_name = "QUESTIONS_FILE")]
    questions: PathBuf,
}

#[derive(Debug, Subcommand)]
enum Command {
    /// Ask the bot every question in your bank, save a report in a dated folder,
    /// then empty the notebook so the next run starts fresh.
    Run(RunCmd),
    /// Add a question to your question bank (questions.txt).
    Add(AddArgs),
    /// Show the questions in your bank, numbered.
    List(BankFile),
    /// Remove a question from your bank by its number (see `list`).
    Remove(RemoveArgs),
    /// Write a report (Markdown, HTML, CSV or JSON) from a finished or partial run.
    Report(ReportArgs),
    /// Save a backup copy of the notebook right now (safe while a run is going).
    Backup(StoreArg),
    /// List the backups of the notebook, newest first.
    Backups(StoreArg),
    /// Go back to an earlier backup of the notebook (the current state is kept too).
    Rollback(RollbackArgs),
    /// Take the run back to an earlier question, so it is asked again from there.
    Rewind(RewindArgs),
}

#[derive(Debug, Args)]
struct StoreArg {
    /// The notebook.
    #[arg(long, value_name = "SQLITE_PATH", default_value = "chatterg.db")]
    store: PathBuf,
}

#[derive(Debug, Args)]
struct RollbackArgs {
    #[command(flatten)]
    notebook: StoreArg,

    /// Which backup to go back to: the number `chatterg backups` shows (1 = newest).
    /// Without it, the newest ordinary backup is used.
    #[arg(long, value_name = "NUMBER")]
    to: Option<usize>,
}

/// `40` or `q040`: a question's number as `chatterg list` shows it.
fn parse_question_number(text: &str) -> Result<usize, String> {
    text.trim()
        .trim_start_matches(['q', 'Q'])
        .parse::<usize>()
        .ok()
        .filter(|number| *number > 0)
        .ok_or_else(|| format!("expected a question number like 40 or q040, not \"{text}\""))
}

#[derive(Debug, Args)]
struct RewindArgs {
    #[command(flatten)]
    notebook: StoreArg,

    /// The first question to ask again (the number `chatterg list` shows, or `q040`).
    /// Its answer and every later one are forgotten; earlier answers are kept.
    #[arg(long, value_name = "QUESTION", value_parser = parse_question_number)]
    to: usize,
}

/// Settings shared by every way of running.
#[derive(Debug, Args)]
struct CommonArgs {
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

    /// Never send more than this many requests in a period, spread evenly:
    /// `30/hour`, `10/minute`, `100/day`. Retries count too. With `--delay` as
    /// well, whichever makes the longer gap applies.
    #[arg(long, value_name = "N/UNIT", value_parser = application::parse_rate)]
    rate: Option<Duration>,

    /// The longest pause (seconds) a bot may ask for with `Retry-After` and
    /// similar headers. Your own --cooldown is never cut short by this.
    #[arg(long, value_name = "SECONDS", default_value_t = 3600)]
    max_wait: u64,

    /// Seconds to wait for each agent reply before treating it as unavailable.
    #[arg(long, value_name = "SECONDS", default_value_t = 120)]
    timeout: u64,

    /// Short reply text (case-insensitive) meaning "rate limited". Repeatable;
    /// replaces the built-in list (rate limit, too many requests, ...).
    #[arg(long = "rate-limit-phrase", value_name = "PHRASE")]
    rate_limit_phrases: Vec<String>,

    /// Save a backup copy of the notebook after every this many finished questions
    /// (0 = never). Backups live in `<STORE>.backups/`; see `chatterg backups`.
    #[arg(long, value_name = "QUESTIONS", default_value_t = 25)]
    backup_every: usize,

    /// How many of those automatic backups to keep (older ones are deleted). Backups
    /// you ask for, and the safety copies made before a rollback or rewind, are
    /// never deleted.
    #[arg(long, value_name = "N", default_value_t = 10)]
    keep_backups: usize,

    /// The working notebook. Re-running with the same path carries on where the
    /// last run stopped.
    #[arg(long, value_name = "SQLITE_PATH", default_value = "chatterg.db")]
    store: PathBuf,

    /// Continue a run even though the questions file was edited, as long as every
    /// question already asked is unchanged (edits to later questions are fine).
    #[arg(long, conflicts_with = "restart")]
    force_resume: bool,

    /// Put the unfinished run aside as `<STORE>.<timestamp>.bak` and start again
    /// from the first question.
    #[arg(long)]
    restart: bool,
}

/// `chatterg run`: the friendly way.
#[derive(Debug, Args)]
struct RunCmd {
    /// Target A2A agent URL (the agent card is fetched from
    /// `<AGENT_URL>/.well-known/agent-card.json`).
    #[arg(value_name = "AGENT_URL")]
    target: String,

    /// Your question bank. Normally you leave this out: the file `questions.txt` in
    /// this folder is used. `.yaml`/`.yml`: a list of questions. Any other ending:
    /// one question per line.
    #[arg(value_name = "QUESTIONS_FILE")]
    questions: Option<PathBuf>,

    /// Where finished runs are saved (one dated folder per run).
    #[arg(long, value_name = "FOLDER", default_value = DEFAULT_RUNS_DIR)]
    runs_dir: PathBuf,

    #[command(flatten)]
    common: CommonArgs,
}

/// Everything the advanced form was given, gathered in one place.
struct RunArgs {
    target: String,
    questions: PathBuf,
    common: CommonArgs,
    reports: Vec<PathBuf>,
    overwrite: bool,
}

#[derive(Debug, Args)]
struct BankFile {
    /// The question bank file.
    #[arg(long, value_name = "FILE", default_value = DEFAULT_QUESTIONS)]
    file: PathBuf,
}

#[derive(Debug, Args)]
struct AddArgs {
    /// The question, on one line. Put it in quotes.
    #[arg(value_name = "QUESTION", allow_hyphen_values = true)]
    question: String,

    /// Put the question under this heading (created if it is new).
    #[arg(long, value_name = "NAME")]
    section: Option<String>,

    #[command(flatten)]
    bank: BankFile,
}

#[derive(Debug, Args)]
struct RemoveArgs {
    /// The question's number, as shown by `chatterg list`.
    #[arg(value_name = "NUMBER")]
    number: usize,

    #[command(flatten)]
    bank: BankFile,
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

/// How a run that did not fail ended.
enum Outcome {
    Completed,
    EndedEarly,
}

const EXIT_ENDED_EARLY: u8 = 2;
const EXIT_AGENT_UNAVAILABLE: u8 = 3;
const EXIT_INTERRUPTED: u8 = 130;

/// The user pressed Ctrl+C; everything answered so far is saved.
#[derive(Debug)]
struct Stopped {
    position: usize,
    total: usize,
}

impl std::fmt::Display for Stopped {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "Stopped after {} of {} questions. Your progress is saved. \
             Run the same command again to carry on.",
            self.position, self.total
        )
    }
}

impl Error for Stopped {}

/// Waits for Ctrl+C (or, on Unix, a polite `kill`).
#[cfg(unix)]
async fn wait_for_stop_signal() {
    use tokio::signal::unix::{Signal, SignalKind, signal};

    async fn next(signal: &mut Option<Signal>) {
        match signal {
            Some(signal) => {
                signal.recv().await;
            }
            None => std::future::pending::<()>().await,
        }
    }

    let mut terminate = signal(SignalKind::terminate()).ok();
    let mut hangup = signal(SignalKind::hangup()).ok(); // the terminal window was closed

    tokio::select! {
        _ = tokio::signal::ctrl_c() => {}
        () = next(&mut terminate) => {}
        () = next(&mut hangup) => {}
    }
}

#[cfg(not(unix))]
async fn wait_for_stop_signal() {
    let _ = tokio::signal::ctrl_c().await;
}

/// First Ctrl+C: finish the question being asked, then stop. Second: stop at once.
fn stop_on_ctrl_c() -> Cancel {
    let cancel = Cancel::default();
    let signal_handler = cancel.clone();

    tokio::spawn(async move {
        wait_for_stop_signal().await;
        signal_handler.cancel();
        say!(
            "\nStopping: finishing the question being asked, then saving. \
             Press Ctrl+C again to stop at once."
        );

        wait_for_stop_signal().await;
        say!("Stopping now. Everything up to the last answer is saved.");
        std::process::exit(i32::from(EXIT_INTERRUPTED));
    });

    cancel
}

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

        if same_file(path, &args.common.store) {
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

fn progress_reporter() -> application::ProgressCallback {
    let eta = Mutex::new(Eta::new());

    Arc::new(move |progress: &Progress| match progress {
        Progress::Asking { number, total, id, attempt } => {
            let retry = if *attempt > 1 { format!(" (attempt {attempt})") } else { String::new() };

            let left = eta
                .lock()
                .ok()
                .and_then(|mut eta| eta.observe(*number, *total, Instant::now()))
                .map(|left| format!("  (about {} left)", approximately(left)))
                .unwrap_or_default();

            say!("[{number}/{total}] {id}{retry}{left}");
        }
        Progress::Resumed { waits, paused } => {
            say!(
                "  back to normal after {waits} pause{} ({} waiting)",
                if *waits == 1 { "" } else { "s" },
                approximately(*paused)
            );
        }
        Progress::Warning(warning) => {
            say!("  warning: {warning}");
        }
        Progress::Waiting { reason, wait, max_waits, cooldown } => {
            say!(
                "  agent unavailable: {reason}\n  pausing {}s, then resuming (cooldown {wait}/{max_waits})",
                cooldown.as_secs()
            );
        }
    })
}

/// What `execute` leaves behind for the caller to put away.
struct Executed {
    engine: Engine,
    questionnaire: Questionnaire,
    store: Arc<SqliteStore>,
}

/// Opens the notebook, asks every question that has not been answered yet and
/// returns when the run is over. `runs_dir` is set for the friendly `run` command,
/// which also looks after what an earlier run left in the notebook.
async fn execute(
    common: &CommonArgs,
    target: &str,
    questions: &Path,
    runs_dir: Option<&Path>,
    cancel: Cancel,
) -> Result<Executed, Box<dyn Error>> {
    let mut defaults =
        QuestionDefaults { max_followups: common.retries, ..QuestionDefaults::default() };
    if !common.reject_phrases.is_empty() {
        defaults.reject_if_contains = common.reject_phrases.clone();
    }

    let questionnaire = Questionnaire::from_path_with(questions, &defaults)?;
    let store = Arc::new(SqliteStore::open(&common.store)?.with_backups(
        &common.store,
        BackupPolicy { every: common.backup_every, keep: common.keep_backups },
    ));

    if common.restart {
        let backup = backup_path(&common.store);

        if store.archive_and_reset(&backup)? {
            say!("archived the previous run to {}", backup.display());
        }
    }

    if let Some(runs_dir) = runs_dir {
        prepare_notebook(&store, target, &questionnaire, &common.store, runs_dir).await?;
    }

    let transport = A2aTransport::with_timeout(Duration::from_secs(common.timeout));

    let mut options = RunOptions {
        cooldown: Duration::from_secs(common.cooldown),
        max_waits: common.max_waits,
        delay: Duration::from_secs(common.delay),
        force_resume: common.force_resume,
        on_progress: Some(progress_reporter()),
        max_wait: Duration::from_secs(common.max_wait),
        min_interval: common.rate,
        cancel,
        ..RunOptions::default()
    };
    if !common.rate_limit_phrases.is_empty() {
        options.rate_limit_phrases = common.rate_limit_phrases.clone();
    }

    let engine = match application::run_with(
        questionnaire.clone(),
        target,
        &transport,
        Arc::clone(&store),
        &options,
    )
    .await
    {
        Ok(engine) => engine,
        Err(ApplicationError::Interrupted) => {
            let position = store.load().await.ok().flatten().map_or(0, |saved| saved.position);

            return Err(Box::new(Stopped { position, total: questionnaire.questions.len() }));
        }
        Err(error) => return Err(error.into()),
    };

    Ok(Executed { engine, questionnaire, store })
}

/// Before the friendly `run` asks anything: put away a finished run an earlier
/// session left behind, refuse to mix up two different bots, and say when a run
/// is being carried on.
async fn prepare_notebook(
    store: &Arc<SqliteStore>,
    target: &str,
    questionnaire: &Questionnaire,
    store_path: &Path,
    runs_dir: &Path,
) -> Result<(), Box<dyn Error>> {
    let Some(stored) = store.load().await? else {
        return Ok(());
    };

    if matches!(stored.state, State::Complete | State::Aborted) {
        let saved = runs::archive_run(store, &stored, None, None, runs_dir)?;
        say!(
            "An earlier run had finished but was never put away. It is now saved in {}.",
            saved.folder.display()
        );
        return Ok(());
    }

    if let (Some(agent), Ok(requested)) = (&stored.agent, url::Url::parse(target))
        && agent.target != requested.to_string()
    {
        return Err(format!(
            "an unfinished run for {} is waiting in {}. Carry it on by running the same command \
             with that address, or add --restart to put it aside and start over.",
            agent.target,
            store_path.display()
        )
        .into());
    }

    if stored.position > 0 {
        say!(
            "Carrying on: {} of {} questions are already done.",
            stored.position,
            questionnaire.questions.len()
        );
    }

    Ok(())
}

fn describe_saved_file(name: &str) -> &'static str {
    match name {
        "report.html" => "open this one in your browser",
        "report.md" => "the same report as plain text",
        "answers.csv" => "for spreadsheets",
        "notebook.db" => "the raw record",
        _ => "the questions that were asked",
    }
}

/// `chatterg run <url>`: ask everything, put the finished run away, start fresh.
async fn run_friendly(cmd: RunCmd) -> Result<Outcome, Box<dyn Error>> {
    let explicit = cmd.questions.is_some();
    let questions = cmd.questions.clone().unwrap_or_else(|| PathBuf::from(DEFAULT_QUESTIONS));

    if !explicit && !questions.exists() {
        return Err(format!(
            "there is no {} in this folder yet. Add your first question with:\n  \
             chatterg add \"What is a zeolite?\"\nor use a different file:\n  \
             chatterg run {} my_questions.txt",
            questions.display(),
            cmd.target
        )
        .into());
    }

    let cancel = stop_on_ctrl_c();
    let executed =
        execute(&cmd.common, &cmd.target, &questions, Some(&cmd.runs_dir), cancel).await?;
    let report = Report::build(executed.engine.conversation(), Some(&executed.questionnaire));

    let ended_early = match executed.engine.start() {
        EngineState::Complete => {
            out!("{}", report.done_note());
            false
        }
        EngineState::Aborted => {
            say!(
                "the run ended early: a question failed and its on_failure policy is abort/unknown"
            );
            out!("{}", report.stopped_note());
            true
        }
        EngineState::Ready | EngineState::Asking(_) => {
            return Err("the run is not finished".into());
        }
    };

    let saved = runs::archive_run(
        &executed.store,
        executed.engine.conversation(),
        Some(&executed.questionnaire),
        Some(&questions),
        &cmd.runs_dir,
    )
    .map_err(|error| {
        format!(
            "the run is finished and still saved in {}, but it could not be put away: {error}. \
             Run the same command again to try once more.",
            cmd.common.store.display()
        )
    })?;

    out!("\nSaved to {}/", saved.folder.display());
    for file in &saved.files {
        out!("  {file:<13} {}", describe_saved_file(file));
    }
    out!("\nThe notebook is empty again. Run the same command to ask the bot again.");

    Ok(if ended_early { Outcome::EndedEarly } else { Outcome::Completed })
}

/// The advanced form: `chatterg <url> <questions-file> [options]`.
async fn run_legacy(cli: RunArgs) -> Result<Outcome, Box<dyn Error>> {
    let planned_reports = plan_reports(&cli)?;

    let cancel = stop_on_ctrl_c();
    let executed = execute(&cli.common, &cli.target, &cli.questions, None, cancel).await?;

    out_raw!("{}", human::render(executed.engine.conversation()));

    if executed.engine.start() == EngineState::Aborted {
        say!("the run ended early: a question failed and its on_failure policy is abort/unknown");
        return Ok(Outcome::EndedEarly);
    }

    // Every question has been processed: say so, and write the requested reports.
    let finished = Report::build(executed.engine.conversation(), Some(&executed.questionnaire));
    say!("{}", finished.done_note());

    let mut failures = Vec::new();
    for (path, format) in &planned_reports {
        match write_file(path, &finished.render(*format), cli.overwrite) {
            Ok(()) => say!("Report: {}", path.display()),
            Err(error) => failures.push(error),
        }
    }

    if !failures.is_empty() {
        return Err(format!(
            "the run finished and is saved in {}, but a report could not be written: {}. \
             You can make it later with: chatterg report --store {}",
            cli.common.store.display(),
            failures.join("; "),
            cli.common.store.display()
        )
        .into());
    }

    Ok(Outcome::Completed)
}

fn backup_note(file: &Path, existed: bool) {
    if existed {
        out!("(The previous version is kept as {}.)", bank::backup_of(file).display());
    }
}

fn bank_add(args: AddArgs) -> Result<Outcome, Box<dyn Error>> {
    let file = &args.bank.file;
    let existed = file.exists();

    let added = bank::add_to_file(file, &args.question, args.section.as_deref())?;

    out!("Added as question {} in {}.", added.number, file.display());
    backup_note(file, existed);
    Ok(Outcome::Completed)
}

fn bank_remove(args: RemoveArgs) -> Result<Outcome, Box<dyn Error>> {
    let file = &args.bank.file;
    let existed = file.exists();

    let removed = bank::remove_from_file(file, args.number)?;

    out!("Removed question {} from {}: {}", removed.number, file.display(), removed.question);
    backup_note(file, existed);
    Ok(Outcome::Completed)
}

fn bank_list(args: BankFile) -> Result<Outcome, Box<dyn Error>> {
    if !args.file.exists() {
        return Err(format!(
            "there is no {} yet. Add your first question with:\n  chatterg add \"What is a zeolite?\"",
            args.file.display()
        )
        .into());
    }

    let questionnaire = Questionnaire::from_path(&args.file)?;
    out_raw!("{}", bank::render_list(&questionnaire, &args.file.display().to_string()));
    Ok(Outcome::Completed)
}

fn size_text(bytes: u64) -> String {
    match bytes {
        0..=1023 => format!("{bytes} B"),
        1024..=1_048_575 => format!("{} KB", bytes / 1024),
        _ => format!("{:.1} MB", bytes as f64 / 1_048_576.0),
    }
}

async fn backup_command(args: StoreArg) -> Result<Outcome, Box<dyn Error>> {
    let snapshot = recovery::backup_now(&args.store).await?;

    out!(
        "Backup saved: {} (after {} questions, {})",
        snapshot.path.display(),
        snapshot.position,
        size_text(snapshot.bytes)
    );
    Ok(Outcome::Completed)
}

fn backups_command(args: StoreArg) -> Result<Outcome, Box<dyn Error>> {
    let snapshots = backups::list(&args.store);

    if snapshots.is_empty() {
        out!(
            "No backups yet for {}. They are made automatically during a run (every 25 \
             questions unless you change --backup-every), or on demand with: chatterg backup",
            args.store.display()
        );
        return Ok(Outcome::Completed);
    }

    out!(
        "Backups of {} (newest first), kept in {}:\n",
        args.store.display(),
        backups::directory_for(&args.store).display()
    );
    out!(" no  taken (UTC)            questions  kind    size    file");

    for (index, snapshot) in snapshots.iter().enumerate() {
        out!(
            "{:>3}  {}  {:>9}  {:<6}  {:>6}  {}",
            index + 1,
            snapshot.taken_at.format("%Y-%m-%d %H:%M:%S"),
            snapshot.position,
            snapshot.kind.word(),
            size_text(snapshot.bytes),
            snapshot.file_name()
        );
    }

    out!("\nGo back to one with: chatterg rollback --store {} --to NUMBER", args.store.display());
    Ok(Outcome::Completed)
}

async fn rollback_command(args: RollbackArgs) -> Result<Outcome, Box<dyn Error>> {
    let store = &args.notebook.store;
    let done = recovery::rollback(store, args.to).await?;

    if let Some(aside) = &done.moved_aside {
        out!("The damaged notebook was moved aside to {}.", aside.display());
    }

    let of = if done.total > 0 { format!(" of {}", done.total) } else { String::new() };
    out!(
        "Rolled back to the backup {} ({} questions finished{of}).",
        done.restored_from.file_name(),
        done.position
    );
    if let Some(safety) = &done.safety {
        out!("What the notebook held before is kept as {}.", safety.display());
    }
    out!("Run the same command as before to carry on from there.");
    Ok(Outcome::Completed)
}

async fn rewind_command(args: RewindArgs) -> Result<Outcome, Box<dyn Error>> {
    let done = recovery::rewind(&args.notebook.store, args.to).await?;

    out!(
        "Rewound to question {}: {} answer{} will be asked again (the run had finished {} questions).",
        done.to_question,
        done.dropped,
        if done.dropped == 1 { "" } else { "s" },
        done.from_position
    );
    out!("What the notebook held before is kept as {}.", done.safety.display());
    out!("Run the same command as before to carry on from question {}.", done.to_question);
    Ok(Outcome::Completed)
}

/// What to do when the notebook itself is damaged.
fn explain_damage(notebook: &str) {
    let snapshots = backups::list(Path::new(notebook));

    match snapshots.iter().find(|snapshot| snapshot.kind != backups::Kind::Safety) {
        Some(newest) => {
            say!(
                "\nThere is a backup: {} (taken {}, after {} questions).\n\
                 Put it back with:  chatterg rollback --store {notebook}\n\
                 (the damaged file is moved aside, not deleted)",
                newest.file_name(),
                newest.taken_at.format("%Y-%m-%d %H:%M:%S UTC"),
                newest.position
            );
        }
        None => {
            say!(
                "\nThere are no backups of it. To start fresh, move it out of the way:\n  \
                 mv {notebook} {notebook}.damaged"
            );
        }
    }
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
            say!("wrote {}", path.display());
        }
        None => out_raw!("{text}"),
    }

    Ok(Outcome::Completed)
}

#[tokio::main]
async fn main() -> ExitCode {
    let cli = Cli::parse();

    let result = match cli.command {
        Some(Command::Run(cmd)) => run_friendly(cmd).await,
        Some(Command::Add(args)) => bank_add(args),
        Some(Command::List(args)) => bank_list(args),
        Some(Command::Remove(args)) => bank_remove(args),
        Some(Command::Report(args)) => make_report(args).await,
        Some(Command::Backup(args)) => backup_command(args).await,
        Some(Command::Backups(args)) => backups_command(args),
        Some(Command::Rollback(args)) => rollback_command(args).await,
        Some(Command::Rewind(args)) => rewind_command(args).await,
        None => match cli.legacy {
            Some(positionals) => {
                run_legacy(RunArgs {
                    target: positionals.target,
                    questions: positionals.questions,
                    common: cli.common,
                    reports: cli.reports,
                    overwrite: cli.overwrite,
                })
                .await
            }
            None => Err("missing arguments: expected <AGENT_URL> <QUESTIONS_FILE>".into()),
        },
    };

    match result {
        Ok(Outcome::Completed) => ExitCode::SUCCESS,
        Ok(Outcome::EndedEarly) => ExitCode::from(EXIT_ENDED_EARLY),
        Err(error) if error.is::<Stopped>() => {
            say!("{error}");
            ExitCode::from(EXIT_INTERRUPTED)
        }
        Err(error) => {
            say!("error: {error}");

            let mut source = error.source();
            while let Some(cause) = source {
                say!("  caused by: {cause}");
                source = cause.source();
            }

            if let Some(StorageError::Damaged { path, .. }) = error.downcast_ref::<StorageError>() {
                explain_damage(path);
            }

            match error.downcast_ref::<ApplicationError>() {
                Some(ApplicationError::AgentUnavailable { .. }) => {
                    say!("Your progress is saved. Run the same command again to carry on.");
                    ExitCode::from(EXIT_AGENT_UNAVAILABLE)
                }
                _ => ExitCode::FAILURE,
            }
        }
    }
}
