//! Turns a stored run into a document people can read, share or load into a
//! spreadsheet. Pure functions only: the same run always renders the same text
//! (no clock is read), which keeps the output testable.
//!
//! Everything a bot said is untrusted input. Each renderer escapes it for its
//! own format: HTML-escaped, Markdown-quoted, and spreadsheet-formula-safe CSV.

use std::collections::HashMap;

use chrono::SecondsFormat;
use serde::Serialize;

use crate::domain::{
    AnswerRecord, Attempt, Conversation, DomainError, Question, Questionnaire, QuestionnaireChange,
    Timestamp, ValidationStatus,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Format {
    Markdown,
    Html,
    Csv,
    Json,
}

impl Format {
    /// Guesses the format from a file name's extension.
    pub fn from_path(path: &std::path::Path) -> Option<Self> {
        match path.extension()?.to_str()?.to_ascii_lowercase().as_str() {
            "md" | "markdown" => Some(Self::Markdown),
            "html" | "htm" => Some(Self::Html),
            "csv" => Some(Self::Csv),
            "json" => Some(Self::Json),
            _ => None,
        }
    }
}

// ---- the data every format is rendered from ------------------------------

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Report {
    pub run: RunInfo,
    pub summary: Summary,
    pub sections: Vec<SectionReport>,
    pub needs_attention: Vec<Attention>,
    pub cooldowns: Vec<CooldownRow>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct RunInfo {
    pub state: String,
    pub agent_name: Option<String>,
    pub agent_target: Option<String>,
    pub protocol: Option<String>,
    pub started_at: Option<String>,
    pub finished_at: Option<String>,
    pub duration_seconds: Option<i64>,
    pub chatterg_version: String,
    pub questions_fingerprint: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Summary {
    pub total_questions: usize,
    pub answered: usize,
    pub rejected: usize,
    pub not_asked: usize,
    /// Extra attempts beyond the first, over all questions.
    pub retries: usize,
    pub cooldowns: usize,
    pub cooldown_seconds: u64,
    pub average_latency_ms: Option<i64>,
    pub p95_latency_ms: Option<i64>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct SectionReport {
    pub name: Option<String>,
    pub items: Vec<Item>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Item {
    pub id: String,
    pub question: String,
    /// The final reply, whatever its status (rejected replies are kept so you can read them).
    pub answer: Option<String>,
    /// `accepted`, `rejected`, `unverified`, `target_unknown` or `not_asked`.
    pub status: String,
    pub attempts: usize,
    pub latency_ms: Option<i64>,
    pub reason: Option<String>,
    pub attempt_log: Vec<AttemptRow>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct AttemptRow {
    pub number: usize,
    pub asked: String,
    pub reply: String,
    pub status: String,
    pub sent_at: Option<String>,
    pub latency_ms: Option<i64>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Attention {
    pub id: String,
    pub status: String,
    pub reason: String,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct CooldownRow {
    pub at: String,
    pub seconds: u64,
    pub reason: String,
}

/// Refuses to describe a run with questions that are not the ones it used.
pub fn check_questions(
    conversation: &Conversation,
    questionnaire: &Questionnaire,
) -> Result<(), DomainError> {
    let Some(stored) = &conversation.questionnaire_fingerprint else {
        return Ok(()); // an old run: nothing to compare with
    };

    if *stored == questionnaire.fingerprint() {
        return Ok(());
    }

    let change = QuestionnaireChange::between(
        &conversation.questionnaire_snapshot,
        &questionnaire.snapshot(),
    );

    Err(DomainError::ReportQuestionsMismatch { summary: change.to_string() })
}

impl Report {
    /// `questionnaire` adds sections and lets the report list questions that were
    /// never asked. Without it the stored snapshot (or the answers themselves) is used.
    pub fn build(conversation: &Conversation, questionnaire: Option<&Questionnaire>) -> Self {
        let records: HashMap<&str, &AnswerRecord> = conversation
            .answers
            .iter()
            .map(|record| (record.question_id.as_str(), record))
            .collect();
        let defined: HashMap<&str, &Question> = questionnaire
            .map(|q| q.questions.iter().map(|question| (question.id.as_str(), question)).collect())
            .unwrap_or_default();

        // (id, question text, section) in the order the run asked them
        let mut order: Vec<(String, String, Option<String>)> = if let Some(q) = questionnaire {
            q.questions
                .iter()
                .map(|question| {
                    (
                        question.id.as_str().to_owned(),
                        question.question.clone(),
                        question.section.clone(),
                    )
                })
                .collect()
        } else if !conversation.questionnaire_snapshot.is_empty() {
            conversation
                .questionnaire_snapshot
                .iter()
                .map(|snap| (snap.id.clone(), snap.question.clone(), None))
                .collect()
        } else {
            Vec::new()
        };

        // answers that are not in that list (old runs, or a list we do not have)
        for record in &conversation.answers {
            let id = record.question_id.as_str();

            if !order.iter().any(|(known, _, _)| known == id) {
                let text = record.attempts.first().map(|a| a.request.clone()).unwrap_or_default();
                order.push((id.to_owned(), text, None));
            }
        }

        let items: Vec<(Option<String>, Item)> = order
            .into_iter()
            .map(|(id, text, section)| {
                let item = build_item(
                    &id,
                    &text,
                    records.get(id.as_str()).copied(),
                    defined.get(id.as_str()).copied(),
                );
                (section, item)
            })
            .collect();

        let mut summary = summarise(&items, conversation);
        summary.not_asked = items.iter().filter(|(_, item)| item.status == "not_asked").count();

        let needs_attention = items
            .iter()
            .filter(|(_, item)| item.status != "accepted")
            .map(|(_, item)| Attention {
                id: item.id.clone(),
                status: item.status.clone(),
                reason: item.reason.clone().unwrap_or_default(),
            })
            .collect();

        Self {
            run: run_info(conversation),
            summary,
            sections: group(items),
            needs_attention,
            cooldowns: conversation
                .cooldowns
                .iter()
                .map(|event| CooldownRow {
                    at: stamp(event.at),
                    seconds: event.seconds,
                    reason: event.reason.clone(),
                })
                .collect(),
        }
    }

    pub fn render(&self, format: Format) -> String {
        match format {
            Format::Markdown => self.to_markdown(),
            Format::Html => self.to_html(),
            Format::Csv => self.to_csv(),
            Format::Json => self.to_json(),
        }
    }
}

fn stamp(time: Timestamp) -> String {
    time.to_rfc3339_opts(SecondsFormat::Secs, true)
}

fn status_name(status: &ValidationStatus) -> &'static str {
    match status {
        ValidationStatus::Accepted => "accepted",
        ValidationStatus::Rejected => "rejected",
        ValidationStatus::Unverified => "unverified",
        ValidationStatus::TargetUnknown => "target_unknown",
    }
}

fn run_info(conversation: &Conversation) -> RunInfo {
    let state = match &conversation.state {
        crate::domain::State::Ready => "in progress".to_owned(),
        crate::domain::State::Waiting(_) => "in progress".to_owned(),
        crate::domain::State::Complete => "complete".to_owned(),
        crate::domain::State::Aborted => "ended early".to_owned(),
    };

    let duration_seconds = match (conversation.started_at, conversation.finished_at) {
        (Some(start), Some(end)) => Some((end - start).num_seconds().max(0)),
        _ => None,
    };

    RunInfo {
        state,
        agent_name: conversation.agent.as_ref().and_then(|agent| agent.name.clone()),
        agent_target: conversation.agent.as_ref().map(|agent| agent.target.clone()),
        protocol: conversation.agent.as_ref().map(|agent| agent.protocol.clone()),
        started_at: conversation.started_at.map(stamp),
        finished_at: conversation.finished_at.map(stamp),
        duration_seconds,
        chatterg_version: conversation.chatterg_version.clone(),
        questions_fingerprint: conversation.questionnaire_fingerprint.clone(),
    }
}

fn build_item(
    id: &str,
    text: &str,
    record: Option<&AnswerRecord>,
    defined: Option<&Question>,
) -> Item {
    let Some(record) = record.filter(|record| !record.attempts.is_empty()) else {
        return Item {
            id: id.to_owned(),
            question: text.to_owned(),
            answer: None,
            status: "not_asked".to_owned(),
            attempts: 0,
            latency_ms: None,
            reason: Some("not asked: the run ended before this question".to_owned()),
            attempt_log: Vec::new(),
        };
    };

    let last: &Attempt = &record.attempts[record.attempts.len() - 1];
    let status = status_name(&last.validation);

    let reason = (last.validation != ValidationStatus::Accepted)
        .then(|| why_not_accepted(&last.response, &last.validation, defined));

    Item {
        id: id.to_owned(),
        question: text.to_owned(),
        answer: Some(last.response.clone()),
        status: status.to_owned(),
        attempts: record.attempts.len(),
        latency_ms: last.latency_ms,
        reason,
        attempt_log: record
            .attempts
            .iter()
            .map(|attempt| AttemptRow {
                number: attempt.number,
                asked: attempt.request.clone(),
                reply: attempt.response.clone(),
                status: status_name(&attempt.validation).to_owned(),
                sent_at: attempt.sent_at.map(stamp),
                latency_ms: attempt.latency_ms,
            })
            .collect(),
    }
}

/// A deterministic explanation of a non-accepted answer.
fn why_not_accepted(
    response: &str,
    status: &ValidationStatus,
    question: Option<&Question>,
) -> String {
    if *status == ValidationStatus::TargetUnknown {
        return "the agent said it does not know".to_owned();
    }

    if response.trim().is_empty() {
        return "empty reply".to_owned();
    }

    if let Some(question) = question {
        let lowered = response.to_lowercase();

        if let Some(phrase) = question
            .reject_if_contains
            .iter()
            .find(|phrase| lowered.contains(&phrase.to_lowercase()))
        {
            return format!("contains the evasive phrase \"{phrase}\"");
        }
    }

    "did not meet the answer rules".to_owned()
}

fn summarise(items: &[(Option<String>, Item)], conversation: &Conversation) -> Summary {
    let answered = items.iter().filter(|(_, item)| item.status == "accepted").count();
    let rejected = items
        .iter()
        .filter(|(_, item)| item.status != "accepted" && item.status != "not_asked")
        .count();
    let retries: usize = items.iter().map(|(_, item)| item.attempts.saturating_sub(1)).sum();

    let mut latencies: Vec<i64> = items
        .iter()
        .flat_map(|(_, item)| item.attempt_log.iter().filter_map(|attempt| attempt.latency_ms))
        .collect();
    latencies.sort_unstable();

    let average_latency_ms =
        (!latencies.is_empty()).then(|| latencies.iter().sum::<i64>() / latencies.len() as i64);

    // nearest-rank percentile
    let p95_latency_ms = (!latencies.is_empty()).then(|| {
        let rank = (latencies.len() * 95).div_ceil(100).max(1);
        latencies[rank - 1]
    });

    Summary {
        total_questions: items.len(),
        answered,
        rejected,
        not_asked: 0,
        retries,
        cooldowns: conversation.cooldowns.len(),
        cooldown_seconds: conversation.cooldowns.iter().map(|event| event.seconds).sum(),
        average_latency_ms,
        p95_latency_ms,
    }
}

/// Groups consecutive items by section, keeping first-seen order.
fn group(items: Vec<(Option<String>, Item)>) -> Vec<SectionReport> {
    let mut sections: Vec<SectionReport> = Vec::new();

    for (name, item) in items {
        match sections.iter_mut().find(|section| section.name == name) {
            Some(section) => section.items.push(item),
            None => sections.push(SectionReport { name, items: vec![item] }),
        }
    }

    sections
}

// ---- shared formatting helpers --------------------------------------------

fn latency_text(ms: Option<i64>) -> String {
    match ms {
        None => "n/a".to_owned(),
        Some(ms) if ms < 1000 => format!("{ms} ms"),
        Some(ms) => format!("{:.1} s", ms as f64 / 1000.0),
    }
}

fn duration_text(seconds: i64) -> String {
    let (hours, minutes, secs) = (seconds / 3600, (seconds % 3600) / 60, seconds % 60);

    match (hours, minutes) {
        (0, 0) => format!("{secs}s"),
        (0, _) => format!("{minutes}m {secs:02}s"),
        _ => format!("{hours}h {minutes:02}m {secs:02}s"),
    }
}

fn plural(count: usize, word: &str) -> String {
    if count == 1 { format!("{count} {word}") } else { format!("{count} {word}s") }
}

fn agent_text(run: &RunInfo) -> String {
    match (&run.agent_name, &run.agent_target) {
        (Some(name), Some(target)) => format!("{name} ({target})"),
        (None, Some(target)) => target.clone(),
        (Some(name), None) => name.clone(),
        (None, None) => "unknown".to_owned(),
    }
}

fn any_named_section(report: &Report) -> bool {
    report.sections.iter().any(|section| section.name.is_some())
}

// ---- Markdown ---------------------------------------------------------------

/// One line of text made safe to sit inside a Markdown table cell or heading.
fn md_line(text: &str) -> String {
    text.replace(['\r', '\n'], " ").replace('|', "\\|")
}

fn md_quote(text: &str) -> String {
    if text.trim().is_empty() {
        return "> *(empty)*".to_owned();
    }

    text.lines().map(|line| format!("> {line}")).collect::<Vec<_>>().join("\n")
}

impl Report {
    fn to_markdown(&self) -> String {
        let run = &self.run;
        let summary = &self.summary;
        let mut out = String::new();

        out.push_str("# Questionnaire report\n\n| | |\n|---|---|\n");
        out.push_str(&format!("| Agent | {} |\n", md_line(&agent_text(run))));
        if let Some(protocol) = &run.protocol {
            out.push_str(&format!("| Protocol | {} |\n", md_line(protocol)));
        }
        out.push_str(&format!("| Run | {} |\n", run.state));
        if let Some(started) = &run.started_at {
            out.push_str(&format!("| Started | {started} |\n"));
        }
        if let Some(finished) = &run.finished_at {
            out.push_str(&format!("| Finished | {finished} |\n"));
        }
        if let Some(seconds) = run.duration_seconds {
            out.push_str(&format!("| Duration | {} |\n", duration_text(seconds)));
        }
        out.push_str(&format!("| chatterg | {} |\n", md_line(&run.chatterg_version)));
        if let Some(fingerprint) = &run.questions_fingerprint {
            out.push_str(&format!(
                "| Questions fingerprint | `{}` |\n",
                &fingerprint[..fingerprint.len().min(12)]
            ));
        }

        out.push_str("\n## Summary\n\n");
        out.push_str("| Questions | Answered | Rejected | Not asked | Retries | Cooldowns | Avg latency | p95 latency |\n");
        out.push_str("|---|---|---|---|---|---|---|---|\n");
        out.push_str(&format!(
            "| {} | {} | {} | {} | {} | {} ({}s) | {} | {} |\n",
            summary.total_questions,
            summary.answered,
            summary.rejected,
            summary.not_asked,
            summary.retries,
            summary.cooldowns,
            summary.cooldown_seconds,
            latency_text(summary.average_latency_ms),
            latency_text(summary.p95_latency_ms),
        ));

        out.push_str("\n## Answers\n");
        let item_marker = if any_named_section(self) { "####" } else { "###" };

        for section in &self.sections {
            if let Some(name) = &section.name {
                out.push_str(&format!("\n### {}\n", md_line(name)));
            }

            for item in &section.items {
                out.push_str(&format!(
                    "\n{item_marker} {}: {}\n\n",
                    item.id,
                    md_line(&item.question)
                ));
                out.push_str(&format!(
                    "*{} · {} · {}*\n\n",
                    item.status.replace('_', " "),
                    plural(item.attempts, "attempt"),
                    latency_text(item.latency_ms)
                ));

                match &item.answer {
                    Some(answer) => out.push_str(&format!("{}\n", md_quote(answer))),
                    None => out.push_str("> *(no answer)*\n"),
                }
            }
        }

        out.push_str("\n## Needs attention\n\n");
        if self.needs_attention.is_empty() {
            out.push_str("Nothing needs attention: every question was answered and accepted.\n");
        } else {
            out.push_str("| Question | Status | Reason |\n|---|---|---|\n");
            for row in &self.needs_attention {
                out.push_str(&format!(
                    "| {} | {} | {} |\n",
                    md_line(&row.id),
                    row.status.replace('_', " "),
                    md_line(&row.reason)
                ));
            }
        }

        out.push_str("\n## Cooldown log\n\n");
        if self.cooldowns.is_empty() {
            out.push_str("The agent never stopped answering.\n");
        } else {
            out.push_str("| Time | Waited | Reason |\n|---|---|---|\n");
            for row in &self.cooldowns {
                out.push_str(&format!(
                    "| {} | {}s | {} |\n",
                    row.at,
                    row.seconds,
                    md_line(&row.reason)
                ));
            }
        }

        out.push_str("\n## Appendix: full transcript\n");
        for item in self.sections.iter().flat_map(|section| &section.items) {
            if item.attempt_log.is_empty() {
                continue;
            }

            out.push_str(&format!("\n### {}\n", item.id));
            for attempt in &item.attempt_log {
                out.push_str(&format!(
                    "\n**Attempt {}**: {}, {}{}\n\nAsked:\n\n{}\n\nReply:\n\n{}\n",
                    attempt.number,
                    attempt.status.replace('_', " "),
                    latency_text(attempt.latency_ms),
                    attempt.sent_at.as_ref().map(|at| format!(", sent {at}")).unwrap_or_default(),
                    md_quote(&attempt.asked),
                    md_quote(&attempt.reply),
                ));
            }
        }

        out
    }
}

// ---- HTML -----------------------------------------------------------------

/// Escapes text for HTML element content and attribute values.
pub fn html_escape(text: &str) -> String {
    let mut out = String::with_capacity(text.len());

    for character in text.chars() {
        match character {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&#39;"),
            other => out.push(other),
        }
    }

    out
}

const HTML_STYLE: &str = r#"
:root { --bg:#fff; --fg:#1b1f24; --muted:#59636e; --line:#d0d7de; --card:#f6f8fa;
        --ok:#1a7f37; --bad:#cf222e; --warn:#9a6700; }
@media (prefers-color-scheme: dark) {
  :root { --bg:#0d1117; --fg:#e6edf3; --muted:#9198a1; --line:#30363d; --card:#161b22;
          --ok:#3fb950; --bad:#f85149; --warn:#d29922; }
}
* { box-sizing: border-box; }
body { margin:0 auto; max-width:60rem; padding:1.5rem 1rem 4rem; background:var(--bg); color:var(--fg);
       font:16px/1.55 system-ui, -apple-system, "Segoe UI", Roboto, sans-serif; }
h1 { margin-top:0; } h2 { margin-top:2.2rem; border-bottom:1px solid var(--line); padding-bottom:.3rem; }
h3 { margin-bottom:.2rem; } h4 { margin:1.4rem 0 .2rem; }
.scroll { overflow-x:auto; }
table { border-collapse:collapse; width:100%; margin:.6rem 0; }
th, td { border:1px solid var(--line); padding:.4rem .6rem; text-align:left; vertical-align:top; }
th { background:var(--card); }
.meta { color:var(--muted); font-size:.9rem; margin:.1rem 0 .4rem; }
blockquote { margin:.2rem 0 .8rem; padding:.5rem .9rem; background:var(--card);
             border-left:4px solid var(--line); white-space:pre-wrap; overflow-wrap:anywhere; }
.status { font-weight:600; } .accepted { color:var(--ok); } .rejected, .target_unknown { color:var(--bad); }
.not_asked, .unverified { color:var(--warn); }
details { margin:.5rem 0; } summary { cursor:pointer; font-weight:600; }
"#;

fn status_class(status: &str) -> &'static str {
    match status {
        "accepted" => "accepted",
        "rejected" => "rejected",
        "target_unknown" => "target_unknown",
        "not_asked" => "not_asked",
        _ => "unverified",
    }
}

fn html_status(status: &str) -> String {
    format!(
        "<span class=\"status {}\">{}</span>",
        status_class(status),
        html_escape(&status.replace('_', " "))
    )
}

fn html_row(cells: &[String]) -> String {
    let cells: String = cells.iter().map(|cell| format!("<td>{cell}</td>")).collect();
    format!("<tr>{cells}</tr>\n")
}

impl Report {
    fn to_html(&self) -> String {
        let run = &self.run;
        let summary = &self.summary;
        let mut out = String::new();

        out.push_str("<!doctype html>\n<html lang=\"en\">\n<head>\n<meta charset=\"utf-8\">\n");
        out.push_str("<meta name=\"viewport\" content=\"width=device-width, initial-scale=1\">\n");
        out.push_str("<title>Questionnaire report</title>\n<style>");
        out.push_str(HTML_STYLE);
        out.push_str("</style>\n</head>\n<body>\n<h1>Questionnaire report</h1>\n");

        out.push_str("<div class=\"scroll\"><table>\n");
        let mut facts: Vec<(&str, String)> = vec![("Agent", agent_text(run))];
        if let Some(protocol) = &run.protocol {
            facts.push(("Protocol", protocol.clone()));
        }
        facts.push(("Run", run.state.clone()));
        if let Some(started) = &run.started_at {
            facts.push(("Started", started.clone()));
        }
        if let Some(finished) = &run.finished_at {
            facts.push(("Finished", finished.clone()));
        }
        if let Some(seconds) = run.duration_seconds {
            facts.push(("Duration", duration_text(seconds)));
        }
        facts.push(("chatterg", run.chatterg_version.clone()));
        if let Some(fingerprint) = &run.questions_fingerprint {
            facts.push((
                "Questions fingerprint",
                fingerprint[..fingerprint.len().min(12)].to_owned(),
            ));
        }
        for (label, value) in facts {
            out.push_str(&format!("<tr><th>{label}</th><td>{}</td></tr>\n", html_escape(&value)));
        }
        out.push_str("</table></div>\n");

        out.push_str("<h2>Summary</h2>\n<div class=\"scroll\"><table>\n<tr><th>Questions</th><th>Answered</th><th>Rejected</th><th>Not asked</th><th>Retries</th><th>Cooldowns</th><th>Avg latency</th><th>p95 latency</th></tr>\n");
        out.push_str(&html_row(&[
            summary.total_questions.to_string(),
            summary.answered.to_string(),
            summary.rejected.to_string(),
            summary.not_asked.to_string(),
            summary.retries.to_string(),
            format!("{} ({}s)", summary.cooldowns, summary.cooldown_seconds),
            latency_text(summary.average_latency_ms),
            latency_text(summary.p95_latency_ms),
        ]));
        out.push_str("</table></div>\n<h2>Answers</h2>\n");

        let named = any_named_section(self);
        for section in &self.sections {
            if let Some(name) = &section.name {
                out.push_str(&format!("<h3>{}</h3>\n", html_escape(name)));
            }

            for item in &section.items {
                let tag = if named { "h4" } else { "h3" };
                out.push_str(&format!(
                    "<{tag}>{}: {}</{tag}>\n<p class=\"meta\">{} · {} · {}</p>\n",
                    html_escape(&item.id),
                    html_escape(&item.question),
                    html_status(&item.status),
                    plural(item.attempts, "attempt"),
                    latency_text(item.latency_ms),
                ));
                match &item.answer {
                    Some(answer) if !answer.trim().is_empty() => {
                        out.push_str(&format!(
                            "<blockquote>{}</blockquote>\n",
                            html_escape(answer)
                        ));
                    }
                    Some(_) => out.push_str("<blockquote><em>(empty)</em></blockquote>\n"),
                    None => out.push_str("<blockquote><em>(no answer)</em></blockquote>\n"),
                }
            }
        }

        out.push_str("<h2>Needs attention</h2>\n");
        if self.needs_attention.is_empty() {
            out.push_str(
                "<p>Nothing needs attention: every question was answered and accepted.</p>\n",
            );
        } else {
            out.push_str("<div class=\"scroll\"><table>\n<tr><th>Question</th><th>Status</th><th>Reason</th></tr>\n");
            for row in &self.needs_attention {
                out.push_str(&html_row(&[
                    html_escape(&row.id),
                    html_status(&row.status),
                    html_escape(&row.reason),
                ]));
            }
            out.push_str("</table></div>\n");
        }

        out.push_str("<h2>Cooldown log</h2>\n");
        if self.cooldowns.is_empty() {
            out.push_str("<p>The agent never stopped answering.</p>\n");
        } else {
            out.push_str("<div class=\"scroll\"><table>\n<tr><th>Time</th><th>Waited</th><th>Reason</th></tr>\n");
            for row in &self.cooldowns {
                out.push_str(&html_row(&[
                    html_escape(&row.at),
                    format!("{}s", row.seconds),
                    html_escape(&row.reason),
                ]));
            }
            out.push_str("</table></div>\n");
        }

        out.push_str("<h2>Appendix: full transcript</h2>\n");
        for item in self.sections.iter().flat_map(|section| &section.items) {
            if item.attempt_log.is_empty() {
                continue;
            }

            out.push_str(&format!(
                "<details><summary>{}: {} attempt{}</summary>\n",
                html_escape(&item.id),
                item.attempts,
                if item.attempts == 1 { "" } else { "s" }
            ));
            for attempt in &item.attempt_log {
                out.push_str(&format!(
                    "<p class=\"meta\">Attempt {} · {} · {}{}</p>\n<blockquote>{}</blockquote>\n<blockquote>{}</blockquote>\n",
                    attempt.number,
                    html_status(&attempt.status),
                    latency_text(attempt.latency_ms),
                    attempt.sent_at.as_ref().map(|at| format!(" · sent {}", html_escape(at))).unwrap_or_default(),
                    html_escape(&attempt.asked),
                    html_escape(&attempt.reply),
                ));
            }
            out.push_str("</details>\n");
        }

        out.push_str("</body>\n</html>\n");
        out
    }
}

// ---- CSV ------------------------------------------------------------------

/// One CSV cell (RFC 4180 quoting). Cells that start with a character spreadsheets
/// treat as a formula (`= + - @`, tab, carriage return) get a leading apostrophe,
/// because the text came from a bot and must never run as a formula.
pub fn csv_cell(value: &str) -> String {
    let mut text = value.to_owned();

    if matches!(text.chars().next(), Some('=' | '+' | '-' | '@' | '\t' | '\r')) {
        text.insert(0, '\'');
    }

    if text.contains([',', '"', '\n', '\r']) {
        format!("\"{}\"", text.replace('"', "\"\""))
    } else {
        text
    }
}

impl Report {
    fn to_csv(&self) -> String {
        let mut out =
            String::from("id,section,question,answer,status,attempts,latency_ms,reason\r\n");

        for section in &self.sections {
            for item in &section.items {
                let row = [
                    csv_cell(&item.id),
                    csv_cell(section.name.as_deref().unwrap_or("")),
                    csv_cell(&item.question),
                    csv_cell(item.answer.as_deref().unwrap_or("")),
                    csv_cell(&item.status),
                    item.attempts.to_string(),
                    item.latency_ms.map(|ms| ms.to_string()).unwrap_or_default(),
                    csv_cell(item.reason.as_deref().unwrap_or("")),
                ];

                out.push_str(&row.join(","));
                out.push_str("\r\n");
            }
        }

        out
    }

    fn to_json(&self) -> String {
        // Serialising plain structs of strings and numbers cannot fail.
        serde_json::to_string_pretty(self).unwrap_or_else(|_| "{}".to_owned()) + "\n"
    }
}
