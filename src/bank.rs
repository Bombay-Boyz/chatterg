//! The question bank: a plain-text file (normally `questions.txt`) that `chatterg
//! add`, `list` and `remove` edit for you. The rules for what counts as a question
//! come from the same line parser the run uses, so what you add is exactly what
//! gets asked.
//!
//! Edits are written to a temporary file and renamed into place, and the previous
//! version is always kept next to the file as `<name>.bak`.

use std::{
    ffi::OsString,
    path::{Path, PathBuf},
};

use thiserror::Error;

use crate::domain::{Line, Questionnaire, parse_line};

#[derive(Debug, Error)]
pub enum BankError {
    #[error(
        "chatterg can only edit plain-text question files; {0} is a YAML file, please edit it by hand"
    )]
    NotPlainText(String),

    #[error("a question cannot be empty")]
    Empty,

    #[error("a question must be a single line")]
    MultiLine,

    #[error(
        "that text would be read differently from how you typed it: text that starts with #, \
         starts with a list marker (-, * or 1.) or is wrapped in quote marks is changed when \
         the file is read. Please rephrase it"
    )]
    ReadDifferently,

    #[error("a section name must be a single line and must not start with #")]
    BadSection,

    #[error("that question is already in the bank as number {number}: {text}")]
    Duplicate { number: usize, text: String },

    #[error("there is no question number {number}; the bank has {count}")]
    NoSuchQuestion { number: usize, count: usize },

    #[error("cannot read {}: {source}", path.display())]
    Read {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },

    #[error("cannot write {}: {source}", path.display())]
    Write {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
}

/// What happened when a question was added.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Added {
    /// The question's number in the bank (what `list` shows and `remove` takes).
    pub number: usize,
    /// The question exactly as it was stored.
    pub question: String,
}

/// What happened when a question was removed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Removed {
    pub number: usize,
    pub question: String,
}

// ---- pure text transformations ---------------------------------------------------

/// Every question in the text, in order.
pub fn questions_in(text: &str) -> Vec<String> {
    text.lines()
        .filter_map(|line| match parse_line(line) {
            Line::Question(question) => Some(question),
            _ => None,
        })
        .collect()
}

fn clean_question(question: &str) -> Result<String, BankError> {
    let question = question.trim();

    if question.is_empty() {
        return Err(BankError::Empty);
    }

    if question.contains(['\n', '\r']) {
        return Err(BankError::MultiLine);
    }

    match parse_line(question) {
        Line::Question(read) if read == question => Ok(question.to_owned()),
        _ => Err(BankError::ReadDifferently),
    }
}

fn clean_section(section: &str) -> Result<String, BankError> {
    let section = section.trim();

    if section.is_empty() || section.contains(['\n', '\r']) || section.starts_with('#') {
        return Err(BankError::BadSection);
    }

    Ok(section.to_owned())
}

fn heading_name(line: &str) -> Option<Option<String>> {
    match parse_line(line) {
        Line::Section(name) => Some(name),
        _ => None,
    }
}

/// The section the file ends in (a bare `##` ends the current one).
fn section_at_end(lines: &[String]) -> Option<String> {
    let mut current = None;

    for line in lines {
        if let Some(name) = heading_name(line) {
            current = name;
        }
    }

    current
}

/// Line range `start..end` of the first section with this name (heading excluded).
fn find_section(lines: &[String], name: &str) -> Option<(usize, usize)> {
    let wanted = name.to_lowercase();

    let heading = lines.iter().position(
        |line| matches!(heading_name(line), Some(Some(found)) if found.to_lowercase() == wanted),
    )?;

    let end = lines
        .iter()
        .enumerate()
        .skip(heading + 1)
        .find(|(_, line)| heading_name(line).is_some())
        .map_or(lines.len(), |(index, _)| index);

    Some((heading + 1, end))
}

fn to_text(lines: &[String]) -> String {
    let mut text = lines.join("\n");
    text.push('\n');
    text
}

/// Number (1-based) of the question that sits on line `index`.
fn number_of_line(lines: &[String], index: usize) -> usize {
    lines[..=index].iter().filter(|line| matches!(parse_line(line), Line::Question(_))).count()
}

/// Returns the new file text and which question was added.
///
/// Without a section the question goes at the end of the file. With one it goes
/// at the end of that section, which is created at the end of the file if it does
/// not exist yet. Comments, headings and blank lines are never touched.
pub fn add(
    text: &str,
    question: &str,
    section: Option<&str>,
) -> Result<(String, Added), BankError> {
    let question = clean_question(question)?;
    let section = section.map(clean_section).transpose()?;

    if let Some((index, existing)) = questions_in(text)
        .into_iter()
        .enumerate()
        .find(|(_, existing)| existing.to_lowercase() == question.to_lowercase())
    {
        return Err(BankError::Duplicate { number: index + 1, text: existing });
    }

    let mut lines: Vec<String> = text.lines().map(str::to_owned).collect();

    let at = match &section {
        Some(name) => match find_section(&lines, name) {
            Some((start, end)) => {
                let after = (start..end)
                    .rev()
                    .find(|&index| matches!(parse_line(&lines[index]), Line::Question(_)))
                    .map_or(start, |index| index + 1);
                lines.insert(after, question.clone());
                after
            }
            None => {
                if lines.last().is_some_and(|line| !line.trim().is_empty()) {
                    lines.push(String::new());
                }
                lines.push(format!("## {name}"));
                lines.push(question.clone());
                lines.len() - 1
            }
        },
        None => {
            if section_at_end(&lines).is_some() {
                // a bare `##` closes the section so the new question stays unsectioned
                if lines.last().is_some_and(|line| !line.trim().is_empty()) {
                    lines.push(String::new());
                }
                lines.push("##".to_owned());
            }
            lines.push(question.clone());
            lines.len() - 1
        }
    };

    let number = number_of_line(&lines, at);
    Ok((to_text(&lines), Added { number, question }))
}

/// Returns the new file text and which question was removed.
pub fn remove(text: &str, number: usize) -> Result<(String, Removed), BankError> {
    let lines: Vec<String> = text.lines().map(str::to_owned).collect();
    let count = questions_in(text).len();

    let mut seen = 0;
    let target = lines.iter().position(|line| {
        if matches!(parse_line(line), Line::Question(_)) {
            seen += 1;
            seen == number
        } else {
            false
        }
    });

    let Some(index) = target.filter(|_| number >= 1) else {
        return Err(BankError::NoSuchQuestion { number, count });
    };

    let Line::Question(question) = parse_line(&lines[index]) else {
        return Err(BankError::NoSuchQuestion { number, count });
    };

    let mut kept = lines;
    kept.remove(index);

    Ok((to_text(&kept), Removed { number, question }))
}

/// A readable listing of the bank, numbered the way `remove` expects.
pub fn render_list(questionnaire: &Questionnaire, file: &str) -> String {
    let count = questionnaire.questions.len();
    let width = count.to_string().len().max(2);

    let mut out = format!("{file}: {count} question{}\n", if count == 1 { "" } else { "s" });

    let mut current: Option<&Option<String>> = None;

    for (index, question) in questionnaire.questions.iter().enumerate() {
        if current != Some(&question.section) {
            match &question.section {
                Some(name) => out.push_str(&format!("\n## {name}\n")),
                None if current.is_some() => out.push_str("\n(no section)\n"),
                None => out.push('\n'),
            }
            current = Some(&question.section);
        }

        out.push_str(&format!("{:>width$}  {}\n", index + 1, question.question));
    }

    out
}

// ---- files ------------------------------------------------------------------------

fn ensure_plain_text(path: &Path) -> Result<(), BankError> {
    let is_yaml = matches!(
        path.extension().and_then(|e| e.to_str()).map(str::to_ascii_lowercase).as_deref(),
        Some("yaml" | "yml")
    );

    if is_yaml { Err(BankError::NotPlainText(path.display().to_string())) } else { Ok(()) }
}

fn read(path: &Path) -> Result<String, BankError> {
    match std::fs::read_to_string(path) {
        Ok(text) => Ok(text),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(String::new()),
        Err(source) => Err(BankError::Read { path: path.to_owned(), source }),
    }
}

fn sibling(path: &Path, prefix: &str, suffix: &str) -> PathBuf {
    let mut name = OsString::from(prefix);
    name.push(path.file_name().unwrap_or_default());
    name.push(suffix);
    path.with_file_name(name)
}

/// Where the previous version of a bank file is kept.
pub fn backup_of(path: &Path) -> PathBuf {
    sibling(path, "", ".bak")
}

/// Writes the new text next to the file and renames it into place, keeping the
/// previous version as `<name>.bak`.
fn write_keeping_backup(path: &Path, text: &str) -> Result<(), BankError> {
    let write_error = |source| BankError::Write { path: path.to_owned(), source };

    if let Some(parent) = path.parent().filter(|parent| !parent.as_os_str().is_empty()) {
        std::fs::create_dir_all(parent).map_err(write_error)?;
    }

    if path.exists() {
        std::fs::copy(path, backup_of(path)).map_err(write_error)?;
    }

    let temporary = sibling(path, ".", ".tmp");
    std::fs::write(&temporary, text).map_err(write_error)?;
    std::fs::rename(&temporary, path).map_err(|source| {
        let _ = std::fs::remove_file(&temporary);
        write_error(source)
    })
}

/// Adds a question to a plain-text bank file, creating the file if needed.
pub fn add_to_file(path: &Path, question: &str, section: Option<&str>) -> Result<Added, BankError> {
    ensure_plain_text(path)?;

    let (text, added) = add(&read(path)?, question, section)?;
    write_keeping_backup(path, &text)?;

    Ok(added)
}

/// Removes question number `number` (as shown by `list`) from a plain-text bank file.
pub fn remove_from_file(path: &Path, number: usize) -> Result<Removed, BankError> {
    ensure_plain_text(path)?;

    let (text, removed) = remove(&read(path)?, number)?;
    write_keeping_backup(path, &text)?;

    Ok(removed)
}
