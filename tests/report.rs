//! Reports are built from a stored run. Everything a bot said is untrusted, so
//! these tests lean hard on escaping.

use std::path::Path;

use chatterg::{
    domain::{
        AnswerRecord, Attempt, Conversation, CooldownEvent, DomainError, QuestionId, Questionnaire,
        State, ValidationStatus,
    },
    output::report::{Format, Report, check_questions, csv_cell, html_escape},
};
use chrono::{Duration, TimeZone, Utc};

fn t0() -> chrono::DateTime<Utc> {
    Utc.with_ymd_and_hms(2026, 10, 7, 9, 0, 0).unwrap()
}

fn attempt(number: usize, asked: &str, reply: &str, status: ValidationStatus, ms: i64) -> Attempt {
    Attempt {
        number,
        request: asked.to_owned(),
        response: reply.to_owned(),
        validation: status,
        sent_at: Some(t0()),
        received_at: Some(t0() + Duration::milliseconds(ms)),
        latency_ms: Some(ms),
    }
}

fn record(id: &str, attempts: Vec<Attempt>) -> AnswerRecord {
    AnswerRecord { question_id: QuestionId::new(id), attempts }
}

fn questionnaire(yaml: &str) -> Questionnaire {
    serde_yaml::from_str(yaml).unwrap()
}

fn q(id: &str, text: &str, section: Option<&str>) -> String {
    let section = section.map(|s| format!(", section: \"{s}\"")).unwrap_or_default();
    format!("  - {{id: {id}, question: \"{text}\", required: true, type: text{section}}}\n")
}

/// Two questions, both accepted, in one named section.
fn tiny() -> (Conversation, Questionnaire) {
    let questionnaire = questionnaire(&format!(
        "questions:\n{}{}",
        q("a", "What is A?", Some("Basics")),
        q("b", "What is B?", Some("Basics"))
    ));

    let conversation = Conversation {
        state: State::Complete,
        answers: vec![
            record(
                "a",
                vec![attempt(1, "What is A?", "A is a letter.", ValidationStatus::Accepted, 250)],
            ),
            record(
                "b",
                vec![attempt(1, "What is B?", "B is another.", ValidationStatus::Accepted, 1500)],
            ),
        ],
        position: 2,
        messages_sent: 2,
        started_at: Some(t0()),
        finished_at: Some(t0() + Duration::seconds(3723)),
        chatterg_version: "9.9.9".into(),
        ..Conversation::default()
    };

    (conversation, questionnaire)
}

// ---- Markdown ------------------------------------------------------------

#[test]
fn markdown_report_for_a_clean_run() {
    let (conversation, questionnaire) = tiny();

    let text = Report::build(&conversation, Some(&questionnaire)).render(Format::Markdown);

    let expected = "\
# Questionnaire report

| | |
|---|---|
| Agent | unknown |
| Run | complete |
| Started | 2026-10-07T09:00:00Z |
| Finished | 2026-10-07T10:02:03Z |
| Duration | 1h 02m 03s |
| chatterg | 9.9.9 |

## Summary

| Questions | Answered | Rejected | Not asked | Retries | Cooldowns | Avg latency | p95 latency |
|---|---|---|---|---|---|---|---|
| 2 | 2 | 0 | 0 | 0 | 0 (0s) | 875 ms | 1.5 s |

## Answers

### Basics

#### a: What is A?

*accepted · 1 attempt · 250 ms*

> A is a letter.

#### b: What is B?

*accepted · 1 attempt · 1.5 s*

> B is another.

## Needs attention

Nothing needs attention: every question was answered and accepted.

## Cooldown log

The agent never stopped answering.

## Appendix: full transcript

### a

**Attempt 1**: accepted, 250 ms, sent 2026-10-07T09:00:00Z

Asked:

> What is A?

Reply:

> A is a letter.

### b

**Attempt 1**: accepted, 1.5 s, sent 2026-10-07T09:00:00Z

Asked:

> What is B?

Reply:

> B is another.
";

    assert_eq!(text, expected);
}

#[test]
fn rendering_twice_gives_identical_text() {
    let (conversation, questionnaire) = tiny();

    for format in [Format::Markdown, Format::Html, Format::Csv, Format::Json] {
        let report = Report::build(&conversation, Some(&questionnaire));
        assert_eq!(report.render(format), report.render(format));
    }
}

#[test]
fn hostile_markdown_stays_inside_its_quote() {
    let (mut conversation, questionnaire) = tiny();
    conversation.answers[0].attempts[0].response =
        "line one\n# Fake heading\n| a | b |\n```\nrm -rf".into();

    let text = Report::build(&conversation, Some(&questionnaire)).render(Format::Markdown);

    assert!(text.contains("> line one\n> # Fake heading\n> | a | b |\n> ```\n> rm -rf"));
    assert!(!text.contains("\n# Fake heading"), "answer text must never start a line");
}

#[test]
fn pipes_in_table_cells_are_escaped() {
    let questionnaire = questionnaire(
        "questions:\n  - {id: a, question: \"A?\", required: true, type: text, reject_if_contains: [\"x|y\"]}\n",
    );
    let conversation = Conversation {
        answers: vec![record(
            "a",
            vec![attempt(1, "A?", "this has X|Y in it", ValidationStatus::Rejected, 5)],
        )],
        position: 1,
        ..Conversation::default()
    };

    let text = Report::build(&conversation, Some(&questionnaire)).render(Format::Markdown);

    // the attention row has exactly three cells even though the reason contains a pipe
    let row = text.lines().find(|line| line.starts_with("| a |")).unwrap();
    assert!(row.contains("x\\|y"), "row was: {row}");
    assert_eq!(row.replace("\\|", "").matches('|').count(), 4, "row was: {row}");
}

// ---- statuses, reasons, numbers -------------------------------------------

#[test]
fn rejected_empty_unknown_and_unasked_questions_are_explained() {
    let questionnaire = questionnaire(&format!(
        "questions:\n  - {{id: a, question: \"A?\", required: true, type: text, reject_if_contains: [\"meeting\"]}}\n{}{}{}",
        q("b", "B?", None),
        q("c", "C?", None),
        q("d", "D?", None),
    ));

    let conversation = Conversation {
        state: State::Aborted,
        answers: vec![
            record(
                "a",
                vec![attempt(1, "A?", "Let's schedule a MEETING", ValidationStatus::Rejected, 100)],
            ),
            record("b", vec![attempt(1, "B?", "   ", ValidationStatus::Rejected, 100)]),
            record("c", vec![attempt(1, "C?", "no idea", ValidationStatus::TargetUnknown, 100)]),
        ],
        position: 3,
        ..Conversation::default()
    };

    let report = Report::build(&conversation, Some(&questionnaire));

    let reasons: Vec<_> = report
        .needs_attention
        .iter()
        .map(|row| (row.id.as_str(), row.status.as_str(), row.reason.as_str()))
        .collect();
    assert_eq!(
        reasons,
        [
            ("a", "rejected", "contains the evasive phrase \"meeting\""),
            ("b", "rejected", "empty reply"),
            ("c", "target_unknown", "the agent said it does not know"),
            ("d", "not_asked", "not asked: the run ended before this question"),
        ]
    );

    assert_eq!(report.summary.total_questions, 4);
    assert_eq!(report.summary.answered, 0);
    assert_eq!(report.summary.rejected, 3);
    assert_eq!(report.summary.not_asked, 1);
    assert_eq!(report.run.state, "ended early");
}

#[test]
fn retries_and_latency_statistics() {
    let questionnaire =
        questionnaire(&format!("questions:\n{}{}", q("a", "A?", None), q("b", "B?", None)));

    let conversation = Conversation {
        state: State::Complete,
        answers: vec![
            record(
                "a",
                vec![
                    attempt(1, "A?", "nope", ValidationStatus::Rejected, 100),
                    attempt(2, "A?", "nope", ValidationStatus::Rejected, 200),
                    attempt(3, "A?", "yes", ValidationStatus::Accepted, 300),
                ],
            ),
            record("b", vec![attempt(1, "B?", "ok", ValidationStatus::Accepted, 400)]),
        ],
        position: 2,
        ..Conversation::default()
    };

    let summary = Report::build(&conversation, Some(&questionnaire)).summary;

    assert_eq!(summary.retries, 2);
    assert_eq!(summary.answered, 2);
    assert_eq!(summary.average_latency_ms, Some(250));
    assert_eq!(summary.p95_latency_ms, Some(400));
}

#[test]
fn p95_uses_nearest_rank() {
    let attempts: Vec<_> = (1..=20)
        .map(|ms| attempt(ms as usize, "A?", "x", ValidationStatus::Accepted, ms * 10))
        .collect();
    let conversation = Conversation {
        answers: vec![record("a", attempts)],
        position: 1,
        ..Conversation::default()
    };

    // rank = ceil(20 * 0.95) = 19 -> 190 ms
    assert_eq!(Report::build(&conversation, None).summary.p95_latency_ms, Some(190));
}

#[test]
fn runs_without_timing_report_na_not_zero() {
    let conversation = Conversation {
        answers: vec![record(
            "a",
            vec![Attempt {
                number: 1,
                request: "A?".into(),
                response: "x".into(),
                validation: ValidationStatus::Accepted,
                sent_at: None,
                received_at: None,
                latency_ms: None,
            }],
        )],
        ..Conversation::default()
    };

    let report = Report::build(&conversation, None);

    assert_eq!(report.summary.average_latency_ms, None);
    assert!(report.render(Format::Markdown).contains("n/a"));
    assert!(!report.render(Format::Markdown).contains("| Started |"));
}

#[test]
fn cooldowns_are_logged_and_totalled() {
    let (mut conversation, questionnaire) = tiny();
    conversation.cooldowns = vec![
        CooldownEvent {
            at: t0(),
            reason: "HTTP request failed with status 429".into(),
            seconds: 120,
        },
        CooldownEvent { at: t0() + Duration::minutes(5), reason: "a | pipe".into(), seconds: 600 },
    ];

    let report = Report::build(&conversation, Some(&questionnaire));
    let text = report.render(Format::Markdown);

    assert_eq!((report.summary.cooldowns, report.summary.cooldown_seconds), (2, 720));
    assert!(text.contains("| 2026-10-07T09:00:00Z | 120s | HTTP request failed with status 429 |"));
    assert!(text.contains("a \\| pipe"));
}

// ---- where questions and sections come from ---------------------------------

#[test]
fn sections_keep_first_seen_order_and_unsectioned_questions_get_their_own_group() {
    let questionnaire = questionnaire(&format!(
        "questions:\n{}{}{}{}",
        q("a", "A?", Some("Second")),
        q("b", "B?", Some("First")),
        q("c", "C?", Some("Second")),
        q("d", "D?", None),
    ));
    let conversation = Conversation::default();

    let report = Report::build(&conversation, Some(&questionnaire));
    let names: Vec<_> = report.sections.iter().map(|s| s.name.as_deref()).collect();

    assert_eq!(names, [Some("Second"), Some("First"), None]);
    assert_eq!(report.sections[0].items.len(), 2);
}

#[test]
fn without_any_sections_questions_use_one_heading_level_less() {
    let questionnaire = questionnaire(&format!("questions:\n{}", q("a", "A?", None)));
    let text =
        Report::build(&Conversation::default(), Some(&questionnaire)).render(Format::Markdown);

    assert!(text.contains("\n### a: A?\n"));
    assert!(!text.contains("####"));
}

#[test]
fn the_stored_snapshot_lists_unasked_questions_when_no_file_is_given() {
    let questionnaire = questionnaire(&format!(
        "questions:\n{}{}",
        q("a", "A?", None),
        q("b", "Second one?", None)
    ));
    let conversation = Conversation {
        answers: vec![record("a", vec![attempt(1, "A?", "x", ValidationStatus::Accepted, 5)])],
        position: 1,
        questionnaire_fingerprint: Some(questionnaire.fingerprint()),
        questionnaire_snapshot: questionnaire.snapshot(),
        ..Conversation::default()
    };

    let report = Report::build(&conversation, None);

    assert_eq!(report.summary.total_questions, 2);
    assert_eq!(report.summary.not_asked, 1);
    assert!(report.render(Format::Markdown).contains("b: Second one?"));
}

#[test]
fn old_runs_without_a_snapshot_still_report_from_their_answers() {
    let conversation = Conversation {
        answers: vec![record(
            "a",
            vec![attempt(1, "What was asked?", "x", ValidationStatus::Accepted, 5)],
        )],
        ..Conversation::default()
    };

    let report = Report::build(&conversation, None);

    assert_eq!(report.sections[0].items[0].question, "What was asked?");
}

#[test]
fn only_the_matching_questions_file_may_be_used() {
    let (mut conversation, questionnaire) = tiny();
    conversation.questionnaire_fingerprint = Some(questionnaire.fingerprint());
    conversation.questionnaire_snapshot = questionnaire.snapshot();

    assert!(check_questions(&conversation, &questionnaire).is_ok());

    let other = questionnaire_with_extra();
    let error = check_questions(&conversation, &other).unwrap_err();
    assert!(
        matches!(&error, DomainError::ReportQuestionsMismatch { summary } if summary.contains("1 added (c)"))
    );

    // runs saved before fingerprints existed cannot be compared, so they are allowed
    conversation.questionnaire_fingerprint = None;
    assert!(check_questions(&conversation, &other).is_ok());
}

fn questionnaire_with_extra() -> Questionnaire {
    questionnaire(&format!(
        "questions:\n{}{}{}",
        q("a", "What is A?", Some("Basics")),
        q("b", "What is B?", Some("Basics")),
        q("c", "What is C?", None)
    ))
}

#[test]
fn a_changed_section_name_is_not_a_changed_question() {
    let one = questionnaire(&format!("questions:\n{}", q("a", "A?", Some("One"))));
    let two = questionnaire(&format!("questions:\n{}", q("a", "A?", Some("Two"))));

    assert_eq!(one.fingerprint(), two.fingerprint());
}

// ---- HTML -----------------------------------------------------------------

const HOSTILE: &str =
    "<script>alert(1)</script> & \"quoted\" 'single' <img src=x onerror=alert(2)>";

#[test]
fn html_escapes_everything_a_bot_or_file_could_inject() {
    let questionnaire = questionnaire(
        "questions:\n  - {id: a, question: \"<b>Q</b> & more\", required: true, type: text, section: \"<i>Sec</i>\", reject_if_contains: [\"<evil>\"]}\n",
    );
    let conversation = Conversation {
        state: State::Complete,
        answers: vec![record(
            "a",
            vec![attempt(1, "<b>Q</b> & more", HOSTILE, ValidationStatus::Rejected, 5)],
        )],
        position: 1,
        cooldowns: vec![CooldownEvent { at: t0(), reason: "<u>why</u>".into(), seconds: 1 }],
        agent: Some(chatterg::domain::AgentInfo {
            name: Some("<Agent>".into()),
            target: "http://x/?a=1&b=2".into(),
            endpoint: "http://x/rpc".into(),
            protocol: "a2a".into(),
        }),
        ..Conversation::default()
    };

    let html = Report::build(&conversation, Some(&questionnaire)).render(Format::Html);

    for raw in
        ["<script", "<img", "<b>Q</b>", "<i>Sec</i>", "<u>why</u>", "<Agent>", "onerror=alert(2)>"]
    {
        assert!(!html.contains(raw), "unescaped {raw:?} leaked into the page");
    }
    assert!(html.contains(
        "&lt;script&gt;alert(1)&lt;/script&gt; &amp; &quot;quoted&quot; &#39;single&#39;"
    ));
    assert!(html.contains("&lt;b&gt;Q&lt;/b&gt; &amp; more"));
    assert!(html.contains("http://x/?a=1&amp;b=2"));
}

#[test]
fn html_is_a_single_self_contained_document() {
    let (conversation, questionnaire) = tiny();
    let html = Report::build(&conversation, Some(&questionnaire)).render(Format::Html);

    assert!(html.starts_with("<!doctype html>"));
    assert!(html.contains("<meta name=\"viewport\""));
    assert!(html.contains("prefers-color-scheme: dark"));
    assert!(html.trim_end().ends_with("</html>"));

    for external in ["http://", "https://", "src=", "<link", "<script"] {
        assert!(!html.contains(external), "report must not load anything external ({external})");
    }
}

#[test]
fn html_escape_covers_the_five_characters() {
    assert_eq!(html_escape("<>&\"'"), "&lt;&gt;&amp;&quot;&#39;");
    assert_eq!(html_escape("plain"), "plain");
}

// ---- CSV ------------------------------------------------------------------

#[test]
fn csv_quotes_commas_quotes_and_newlines() {
    assert_eq!(csv_cell("plain"), "plain");
    assert_eq!(csv_cell("a,b"), "\"a,b\"");
    assert_eq!(csv_cell("say \"hi\""), "\"say \"\"hi\"\"\"");
    assert_eq!(csv_cell("two\nlines"), "\"two\nlines\"");
    assert_eq!(csv_cell(""), "");
}

#[test]
fn csv_neutralises_spreadsheet_formulas() {
    for dangerous in ["=1+1", "+1", "-1", "@SUM(A1)", "\t=1", "\r=1"] {
        assert!(
            csv_cell(dangerous).starts_with('\'') || csv_cell(dangerous).starts_with("\"'"),
            "{dangerous:?} -> {:?}",
            csv_cell(dangerous)
        );
    }

    assert_eq!(
        csv_cell("=HYPERLINK(\"http://x\",\"y\")"),
        "\"'=HYPERLINK(\"\"http://x\"\",\"\"y\"\")\""
    );
    assert_eq!(csv_cell("a=b"), "a=b", "only a leading character is dangerous");
}

#[test]
fn csv_report_has_one_row_per_question_and_crlf_endings() {
    let (conversation, questionnaire) = tiny();

    let csv = Report::build(&conversation, Some(&questionnaire)).render(Format::Csv);

    assert_eq!(
        csv,
        "id,section,question,answer,status,attempts,latency_ms,reason\r\n\
         a,Basics,What is A?,A is a letter.,accepted,1,250,\r\n\
         b,Basics,What is B?,B is another.,accepted,1,1500,\r\n"
    );
}

#[test]
fn csv_keeps_unasked_questions_with_empty_answers() {
    let questionnaire = questionnaire(&format!("questions:\n{}", q("a", "A?", None)));

    let csv = Report::build(&Conversation::default(), Some(&questionnaire)).render(Format::Csv);

    assert!(csv.contains("a,,A?,,not_asked,0,,not asked: the run ended before this question\r\n"));
}

// ---- JSON -----------------------------------------------------------------

#[test]
fn json_report_is_valid_and_carries_the_numbers() {
    let (conversation, questionnaire) = tiny();

    let text = Report::build(&conversation, Some(&questionnaire)).render(Format::Json);
    let value: serde_json::Value = serde_json::from_str(&text).unwrap();

    assert_eq!(value["summary"]["total_questions"], 2);
    assert_eq!(value["summary"]["answered"], 2);
    assert_eq!(value["run"]["state"], "complete");
    assert_eq!(value["sections"][0]["name"], "Basics");
    assert_eq!(value["sections"][0]["items"][1]["latency_ms"], 1500);
    assert!(text.ends_with('\n'));
}

// ---- formats ----------------------------------------------------------------

#[test]
fn the_format_is_guessed_from_the_file_extension() {
    let guess = |name: &str| Format::from_path(Path::new(name));

    assert_eq!(guess("r.md"), Some(Format::Markdown));
    assert_eq!(guess("r.MARKDOWN"), Some(Format::Markdown));
    assert_eq!(guess("r.html"), Some(Format::Html));
    assert_eq!(guess("r.htm"), Some(Format::Html));
    assert_eq!(guess("r.csv"), Some(Format::Csv));
    assert_eq!(guess("r.json"), Some(Format::Json));
    assert_eq!(guess("r.txt"), None);
    assert_eq!(guess("noext"), None);
}
