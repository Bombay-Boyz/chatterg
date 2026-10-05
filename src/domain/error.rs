use std::path::PathBuf;

use thiserror::Error;

#[derive(Debug, Error)]
pub enum DomainError {
    #[error("questionnaire contains no questions")]
    EmptyQuestionnaire,

    #[error("questionnaire contains duplicate question id: {0}")]
    DuplicateQuestionId(String),

    #[error("question not found: {0}")]
    QuestionNotFound(String),

    #[error("stored position {position} is beyond the end of the questionnaire ({len} questions)")]
    InvalidPosition { position: usize, len: usize },

    #[error("cannot read questionnaire {}: {source}", path.display())]
    QuestionnaireRead {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },

    #[error("invalid question #{index} in {}: {source}", path.display())]
    InvalidQuestion {
        path: PathBuf,
        index: usize,
        #[source]
        source: serde_yaml::Error,
    },

    #[error("invalid questionnaire {}: {source}", path.display())]
    QuestionnaireParse {
        path: PathBuf,
        #[source]
        source: serde_yaml::Error,
    },
}
