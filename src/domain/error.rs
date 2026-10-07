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

    #[error(
        "the questions file changed since this run started ({summary}). The already-asked \
         questions are unchanged, so you can continue with the edited list using \
         --force-resume, or start over with --restart (or a new --store). Note: plain-text \
         ids are positional (q001, q002, ...), so inserting a line renumbers every following \
         question."
    )]
    QuestionnaireChanged { summary: String },

    #[error(
        "the questions file changed since this run started ({summary}), and questions that \
         were already asked are affected, so the run cannot be resumed safely. Start over \
         with --restart (the old run is archived) or use a new --store. Note: plain-text ids \
         are positional (q001, q002, ...), so inserting a line renumbers every following \
         question."
    )]
    QuestionnaireChangedUnsafe { summary: String },

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
