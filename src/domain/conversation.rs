use serde::{Deserialize, Serialize};

use super::{
    answer::{AnswerRecord, Attempt, ValidationStatus},
    question::{Question, QuestionId},
    validation::validate,
};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum State {
    Ready,
    Waiting(QuestionId),
    Complete,
    /// The run ended early (abort or unknown policy). Terminal: never resumed.
    Aborted,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Conversation {
    pub state: State,
    pub answers: Vec<AnswerRecord>,

    /// Index of the question currently being asked. Authoritative; never derived
    /// from `answers.len()`.
    pub position: usize,

    /// Number of messages sent *and answered*. The next message id is this + 1,
    /// so an undelivered message keeps its id when retried after a restart.
    pub messages_sent: u64,
}

impl Default for Conversation {
    fn default() -> Self {
        Self { state: State::Ready, answers: Vec::new(), position: 0, messages_sent: 0 }
    }
}

impl Conversation {
    pub fn new() -> Self {
        Self::default()
    }

    /// Sequence number to use for the next outgoing message (1-based).
    pub fn next_message_number(&self) -> u64 {
        self.messages_sent + 1
    }

    pub fn ask(self, question: &Question) -> Self {
        Self { state: State::Waiting(question.id.clone()), ..self }
    }

    pub fn receive(self, question: &Question, response: String) -> Transition {
        let status = validate(question, &response);

        let record = self
            .answers
            .iter()
            .find(|record| record.question_id == question.id)
            .cloned()
            .unwrap_or_else(|| AnswerRecord::new(question.id.clone()));

        let attempt = Attempt {
            number: record.attempts.len() + 1,
            request: question.question.clone(),
            response,
            validation: status.clone(),
        };

        let updated = record.push(attempt);

        let answers = self
            .answers
            .iter()
            .filter(|answer| answer.question_id != question.id)
            .cloned()
            .chain(std::iter::once(updated.clone()))
            .collect();

        let conversation = Self {
            state: State::Ready,
            answers,
            position: self.position,
            messages_sent: self.messages_sent + 1,
        };

        match status {
            ValidationStatus::Accepted => Transition::Accepted(conversation),

            ValidationStatus::Rejected => {
                let attempts = updated.attempts.len();

                if attempts <= question.max_followups {
                    Transition::FollowUp(conversation)
                } else {
                    Transition::Rejected(conversation)
                }
            }

            ValidationStatus::Unverified => Transition::Unverified(conversation),

            ValidationStatus::TargetUnknown => Transition::Unknown(conversation),
        }
    }

    pub fn answer(&self, question_id: &QuestionId) -> Option<&AnswerRecord> {
        self.answers.iter().find(|answer| &answer.question_id == question_id)
    }
}

#[derive(Debug, Clone, PartialEq)]
pub enum Transition {
    Accepted(Conversation),
    FollowUp(Conversation),
    Rejected(Conversation),
    Unverified(Conversation),
    Unknown(Conversation),
}
