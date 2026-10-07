use std::{collections::HashSet, path::Path};

use sha2::{Digest, Sha256};

use serde::{Deserialize, Serialize};
use serde_yaml::Value;

use super::{
    error::DomainError,
    question::{AnswerType, FailurePolicy, Question, QuestionId},
    run::QuestionSnapshot,
};

/// Phrases that mark an answer as evasive when none are configured.
pub const DEFAULT_REJECT_PHRASES: &[&str] = &[
    "meeting",
    "schedule a call",
    "book a call",
    "book a demo",
    "calendly",
    "get in touch",
    "contact us",
];

const FOLLOWUP_PREFIX: &str =
    "Please answer directly and concisely, without proposing a meeting or call: ";

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Questionnaire {
    pub questions: Vec<Question>,
}

/// Settings applied to questions given as bare text (no explicit fields).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct QuestionDefaults {
    /// Extra attempts after the first ask (total asks = `max_followups + 1`).
    pub max_followups: usize,
    pub on_failure: FailurePolicy,
    pub reject_if_contains: Vec<String>,
}

impl Default for QuestionDefaults {
    fn default() -> Self {
        Self {
            max_followups: 2,
            on_failure: FailurePolicy::Continue,
            reject_if_contains: DEFAULT_REJECT_PHRASES.iter().map(|p| (*p).to_owned()).collect(),
        }
    }
}

impl Questionnaire {
    /// Loads a questionnaire using the default settings for bare questions.
    pub fn from_path(path: impl AsRef<Path>) -> Result<Self, DomainError> {
        Self::from_path_with(path, &QuestionDefaults::default())
    }

    /// Loads and validates a questionnaire. Never falls back to a built-in one.
    ///
    /// Accepted inputs:
    /// - `.yaml` / `.yml`: either `questions: [...]` or a bare list, where each
    ///   entry is a plain string or a full question object (they can be mixed).
    /// - anything else (e.g. `.txt`): one question per line; blank lines and
    ///   `#` comments are ignored, and list markers (`-`, `*`, `1.`) and
    ///   surrounding quotes are stripped.
    ///
    /// Questions can be grouped into sections for reports: a `## Heading` line in
    /// a text file, a `section:` key on a YAML question, or a YAML `sections:` list
    /// of `{name, questions}`.
    pub fn from_path_with(
        path: impl AsRef<Path>,
        defaults: &QuestionDefaults,
    ) -> Result<Self, DomainError> {
        let path = path.as_ref();

        let data = std::fs::read_to_string(path)
            .map_err(|source| DomainError::QuestionnaireRead { path: path.to_owned(), source })?;

        let is_yaml = matches!(
            path.extension().and_then(|e| e.to_str()).map(str::to_ascii_lowercase).as_deref(),
            Some("yaml" | "yml")
        );

        let questionnaire = if is_yaml {
            Self::from_yaml(&data, path, defaults)?
        } else {
            Self::from_lines(&data, defaults)
        };

        questionnaire.validate()?;
        Ok(questionnaire)
    }

    fn from_yaml(
        data: &str,
        path: &Path,
        defaults: &QuestionDefaults,
    ) -> Result<Self, DomainError> {
        let parse = |source| DomainError::QuestionnaireParse { path: path.to_owned(), source };

        let root: Value = serde_yaml::from_str(data).map_err(parse)?;

        let entries = match root {
            Value::Sequence(entries) => entries.into_iter().map(|entry| (None, entry)).collect(),
            Value::Mapping(mut map) => {
                let questions = map.remove("questions");
                let sections = map.remove("sections");

                match (questions, sections) {
                    (Some(_), Some(_)) => {
                        return Err(parse(yaml_error(
                            "use either `questions:` or `sections:`, not both",
                        )));
                    }
                    (Some(Value::Sequence(entries)), None) => {
                        entries.into_iter().map(|entry| (None, entry)).collect()
                    }
                    (Some(Value::Null) | None, None) => Vec::new(),
                    (Some(_), None) => {
                        return Err(parse(yaml_error("`questions` must be a list")));
                    }
                    (None, Some(Value::Sequence(sections))) => {
                        flatten_sections(sections).map_err(parse)?
                    }
                    (None, Some(Value::Null)) => Vec::new(),
                    (None, Some(_)) => {
                        return Err(parse(yaml_error("`sections` must be a list")));
                    }
                }
            }
            Value::Null => Vec::new(),
            _ => {
                return Err(parse(yaml_error(
                    "expected a list of questions or a `questions:` list",
                )));
            }
        };

        let width = id_width(entries.len());
        let mut questions = Vec::with_capacity(entries.len());

        for (index, (section, entry)) in entries.into_iter().enumerate() {
            let number = index + 1;

            let question = match entry {
                Value::String(text) => {
                    plain_question(number, width, text.trim(), section.as_deref(), defaults)
                }
                other => {
                    let mut question =
                        serde_yaml::from_value::<Question>(other).map_err(|source| {
                            DomainError::InvalidQuestion {
                                path: path.to_owned(),
                                index: number,
                                source,
                            }
                        })?;

                    if question.section.is_none() {
                        question.section = section;
                    }

                    question
                }
            };

            questions.push(question);
        }

        Ok(Self { questions })
    }

    fn from_lines(data: &str, defaults: &QuestionDefaults) -> Self {
        let mut section: Option<String> = None;
        let mut entries: Vec<(Option<String>, String)> = Vec::new();

        for line in data.lines() {
            match parse_line(line) {
                Line::Skip => {}
                Line::Section(name) => section = name,
                Line::Question(text) => entries.push((section.clone(), text)),
            }
        }

        let width = id_width(entries.len());

        Self {
            questions: entries
                .iter()
                .enumerate()
                .map(|(index, (section, text))| {
                    plain_question(index + 1, width, text, section.as_deref(), defaults)
                })
                .collect(),
        }
    }

    /// Rejects empty questionnaires and duplicate question ids.
    pub fn validate(&self) -> Result<(), DomainError> {
        if self.is_empty() {
            return Err(DomainError::EmptyQuestionnaire);
        }

        let mut seen = HashSet::new();
        for question in &self.questions {
            if !seen.insert(question.id.as_str()) {
                return Err(DomainError::DuplicateQuestionId(question.id.as_str().to_owned()));
            }
        }

        Ok(())
    }

    /// What matters for telling whether a run can continue: ids, texts, types and
    /// allowed values. Retry and evasion settings are deliberately left out.
    pub fn snapshot(&self) -> Vec<QuestionSnapshot> {
        self.questions
            .iter()
            .map(|question| QuestionSnapshot {
                id: question.id.as_str().to_owned(),
                question: question.question.clone(),
                kind: question.answer_type.as_str().to_owned(),
                values: question.values.clone(),
            })
            .collect()
    }

    /// SHA-256 (hex) over the snapshot. Stable across runs and machines.
    pub fn fingerprint(&self) -> String {
        fingerprint_of(&self.snapshot())
    }

    pub fn required(&self) -> impl Iterator<Item = &Question> {
        self.questions.iter().filter(|question| question.required)
    }

    pub fn is_empty(&self) -> bool {
        self.questions.is_empty()
    }
}

fn id_width(count: usize) -> usize {
    count.to_string().len().max(3)
}

fn plain_question(
    number: usize,
    width: usize,
    text: &str,
    section: Option<&str>,
    defaults: &QuestionDefaults,
) -> Question {
    Question {
        id: QuestionId::new(format!("q{number:0width$}")),
        question: text.to_owned(),
        required: true,
        answer_type: AnswerType::Text,
        values: Vec::new(),
        max_followups: defaults.max_followups,
        on_failure: defaults.on_failure.clone(),
        reject_if_contains: defaults.reject_if_contains.clone(),
        followup: Some(format!("{FOLLOWUP_PREFIX}{text}")),
        section: section.map(str::to_owned),
    }
}

fn yaml_error(message: &str) -> serde_yaml::Error {
    <serde_yaml::Error as serde::de::Error>::custom(message)
}

/// `sections: [{name, questions: [...]}]` -> one flat list remembering each section.
fn flatten_sections(
    sections: Vec<Value>,
) -> Result<Vec<(Option<String>, Value)>, serde_yaml::Error> {
    let mut flat = Vec::new();

    for (index, section) in sections.into_iter().enumerate() {
        let Value::Mapping(mut map) = section else {
            return Err(yaml_error(&format!(
                "section #{} must have a `name` and `questions`",
                index + 1
            )));
        };

        let name = match map.remove("name") {
            Some(Value::String(name)) if !name.trim().is_empty() => name.trim().to_owned(),
            _ => {
                return Err(yaml_error(&format!(
                    "section #{} needs a non-empty `name`",
                    index + 1
                )));
            }
        };

        match map.remove("questions") {
            Some(Value::Sequence(entries)) => {
                flat.extend(entries.into_iter().map(|entry| (Some(name.clone()), entry)));
            }
            Some(Value::Null) | None => {}
            Some(_) => {
                return Err(yaml_error(&format!(
                    "`questions` in section \"{name}\" must be a list"
                )));
            }
        }
    }

    Ok(flat)
}

/// What one line of a text questions file means.
enum Line {
    Skip,
    /// `## Heading` starts a section; a bare `##` ends it.
    Section(Option<String>),
    Question(String),
}

fn parse_line(line: &str) -> Line {
    let trimmed = line.trim();

    if trimmed.starts_with("##") {
        let name = trimmed.trim_start_matches('#').trim();
        return Line::Section((!name.is_empty()).then(|| name.to_owned()));
    }

    match clean_line(trimmed) {
        Some(text) => Line::Question(text),
        None => Line::Skip,
    }
}

/// Normalises one line of a text file; `None` means "skip this line".
fn clean_line(line: &str) -> Option<String> {
    let mut text = line.trim();

    if text.is_empty() || text.starts_with('#') {
        return None;
    }

    // list markers: "- ", "* ", "12. ", "12) "
    if let Some(rest) = text.strip_prefix("- ").or_else(|| text.strip_prefix("* ")) {
        text = rest.trim();
    } else if let Some(position) = text.find(['.', ')']) {
        let (digits, rest) = text.split_at(position);
        if !digits.is_empty()
            && digits.chars().all(|c| c.is_ascii_digit())
            && rest[1..].starts_with(char::is_whitespace)
        {
            text = rest[1..].trim();
        }
    }

    // matching surrounding quotes
    for quote in ['"', '\''] {
        if text.len() >= 2 && text.starts_with(quote) && text.ends_with(quote) {
            text = text[1..text.len() - 1].trim();
        }
    }

    (!text.is_empty()).then(|| text.to_owned())
}

/// Hex SHA-256 of a question snapshot.
pub fn fingerprint_of(snapshot: &[QuestionSnapshot]) -> String {
    // Serialising plain structs of strings cannot fail; an empty hash input would
    // still be deterministic, so fall back to that rather than panic.
    let bytes = serde_json::to_vec(snapshot).unwrap_or_default();

    Sha256::digest(&bytes).iter().map(|byte| format!("{byte:02x}")).collect()
}
