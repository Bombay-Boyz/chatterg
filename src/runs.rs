//! Putting a finished run away: the report, the questions that were asked and the
//! raw notebook go into their own dated folder, and the working notebook is
//! emptied so the next run starts from the first question again.
//!
//! Nothing is emptied until every file has been written.

use std::path::{Path, PathBuf};

use chrono::Utc;
use thiserror::Error;

use crate::{
    domain::{Conversation, Questionnaire, Timestamp},
    output::report::{Format, Report},
    storage::{StorageError, sqlite::SqliteStore},
};

#[derive(Debug, Error)]
pub enum RunsError {
    #[error("cannot create the folder {}: {source}", path.display())]
    CreateFolder {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },

    #[error("cannot write {}: {source}", path.display())]
    WriteFile {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },

    #[error(transparent)]
    Archive(#[from] StorageError),
}

/// What was saved.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Archived {
    pub folder: PathBuf,
    /// File names inside `folder`, most useful first.
    pub files: Vec<String>,
}

/// Lower-case letters and digits separated by single dashes, at most 50 long.
pub fn slug(text: &str) -> String {
    let mut slug = String::new();

    for character in text.chars() {
        if character.is_ascii_alphanumeric() {
            slug.push(character.to_ascii_lowercase());
        } else if !slug.ends_with('-') {
            slug.push('-');
        }
    }

    slug.trim_matches('-').chars().take(50).collect::<String>().trim_matches('-').to_owned()
}

/// `20261008-153012-flowmarket-social-nxtbrane`
pub fn folder_name(conversation: &Conversation, now: Timestamp) -> String {
    let when = conversation.finished_at.unwrap_or(now).format("%Y%m%d-%H%M%S");

    let who = conversation
        .agent
        .as_ref()
        .and_then(|agent| url::Url::parse(&agent.target).ok())
        .map(|url| format!("{}{}", url.host_str().unwrap_or(""), url.path()))
        .map(|text| slug(&text))
        .filter(|slug| !slug.is_empty())
        .unwrap_or_else(|| "run".to_owned());

    format!("{when}-{who}")
}

/// A folder name inside `runs_dir` that does not exist yet.
fn unused_folder(runs_dir: &Path, name: &str) -> PathBuf {
    let first = runs_dir.join(name);
    if !first.exists() {
        return first;
    }

    (2..)
        .map(|n| runs_dir.join(format!("{name}-{n}")))
        .find(|candidate| !candidate.exists())
        .unwrap_or(first)
}

fn write(path: &Path, text: &str) -> Result<(), RunsError> {
    std::fs::write(path, text)
        .map_err(|source| RunsError::WriteFile { path: path.to_owned(), source })
}

/// Saves a finished (or ended-early) run into `runs_dir/<dated folder>/` and then
/// empties the working notebook.
///
/// With `questionnaire` the report gets its section headings and `questions_file`
/// is copied along; for a leftover run found in a notebook neither is known and
/// the report is built from what the notebook itself remembers.
pub fn archive_run(
    store: &SqliteStore,
    conversation: &Conversation,
    questionnaire: Option<&Questionnaire>,
    questions_file: Option<&Path>,
    runs_dir: &Path,
) -> Result<Archived, RunsError> {
    let folder = unused_folder(runs_dir, &folder_name(conversation, Utc::now()));

    std::fs::create_dir_all(&folder)
        .map_err(|source| RunsError::CreateFolder { path: folder.clone(), source })?;

    let written = write_everything(&folder, conversation, questionnaire, questions_file);

    let mut files = match written {
        Ok(files) => files,
        Err(error) => {
            // nothing has been emptied yet; do not leave a half-written folder behind
            let _ = std::fs::remove_dir_all(&folder);
            return Err(error);
        }
    };

    // Last step: copy the notebook and empty it. If this fails the run is still in
    // the notebook and can be archived again.
    if store.archive_and_reset(&folder.join("notebook.db"))? {
        files.push("notebook.db".to_owned());
    }

    Ok(Archived { folder, files })
}

fn write_everything(
    folder: &Path,
    conversation: &Conversation,
    questionnaire: Option<&Questionnaire>,
    questions_file: Option<&Path>,
) -> Result<Vec<String>, RunsError> {
    let report = Report::build(conversation, questionnaire);
    let mut files = Vec::new();

    for (name, format) in [
        ("report.html", Format::Html),
        ("report.md", Format::Markdown),
        ("answers.csv", Format::Csv),
    ] {
        write(&folder.join(name), &report.render(format))?;
        files.push(name.to_owned());
    }

    if let Some(source) = questions_file {
        let name =
            format!("questions.{}", source.extension().and_then(|e| e.to_str()).unwrap_or("txt"));
        let target = folder.join(&name);

        std::fs::copy(source, &target)
            .map_err(|source| RunsError::WriteFile { path: target.clone(), source })?;
        files.push(name);
    }

    Ok(files)
}
