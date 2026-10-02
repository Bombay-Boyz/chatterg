use thiserror::Error;

#[derive(Debug, Error)]
pub enum DomainError {
    #[error("questionnaire contains no questions")]
    EmptyQuestionnaire,

    #[error("questionnaire contains duplicate question id: {0}")]
    DuplicateQuestionId(String),

    #[error("question not found: {0}")]
    QuestionNotFound(String),
}
