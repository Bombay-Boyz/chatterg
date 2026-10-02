use serde::{Deserialize, Serialize};

use super::question::QuestionId;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Attempt {
    pub number: usize,
    pub request: String,
    pub response: String,
    pub validation: ValidationStatus,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ValidationStatus {
    Accepted,
    Rejected,
    Unverified,
    TargetUnknown,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AnswerRecord {
    pub question_id: QuestionId,
    pub attempts: Vec<Attempt>,
}

impl AnswerRecord {
    pub fn new(question_id: QuestionId) -> Self {
        Self { question_id, attempts: Vec::new() }
    }

    pub fn push(&self, attempt: Attempt) -> Self {
        let mut attempts = self.attempts.clone();
        attempts.push(attempt);

        Self { question_id: self.question_id.clone(), attempts }
    }

    pub fn accepted(&self) -> bool {
        self.attempts.last().is_some_and(|attempt| attempt.validation == ValidationStatus::Accepted)
    }
}
