use serde::{Deserialize, Serialize};

use super::{
    answer::{AnswerRecord, Attempt, ValidationStatus},
    question::{Question, QuestionId},
    run::{AgentInfo, CooldownEvent, QuestionSnapshot, Timestamp, Timing},
    validation::validate,
};

/// Version of the stored conversation format. Bump when the shape changes and
/// add a step to `storage::migrate`.
pub const SCHEMA_VERSION: u32 = 2;

fn current_schema_version() -> u32 {
    SCHEMA_VERSION
}

fn unknown_version() -> String {
    "unknown".to_owned()
}

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

    /// Format version of this record (see [`SCHEMA_VERSION`]).
    #[serde(default = "current_schema_version")]
    pub schema_version: u32,

    #[serde(default)]
    pub started_at: Option<Timestamp>,
    #[serde(default)]
    pub finished_at: Option<Timestamp>,

    #[serde(default)]
    pub agent: Option<AgentInfo>,

    /// chatterg version that created the run.
    #[serde(default = "unknown_version")]
    pub chatterg_version: String,

    /// Hash and copy of the questions at the start of the run (see `Engine::resume`).
    #[serde(default)]
    pub questionnaire_fingerprint: Option<String>,
    #[serde(default)]
    pub questionnaire_snapshot: Vec<QuestionSnapshot>,

    /// Pauses taken because the agent stopped answering.
    #[serde(default)]
    pub cooldowns: Vec<CooldownEvent>,
}

impl Default for Conversation {
    fn default() -> Self {
        Self {
            state: State::Ready,
            answers: Vec::new(),
            position: 0,
            messages_sent: 0,
            schema_version: SCHEMA_VERSION,
            started_at: None,
            finished_at: None,
            agent: None,
            chatterg_version: env!("CARGO_PKG_VERSION").to_owned(),
            questionnaire_fingerprint: None,
            questionnaire_snapshot: Vec::new(),
            cooldowns: Vec::new(),
        }
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
        self.receive_at(question, response, Timing::default())
    }

    /// Like [`receive`](Self::receive), recording when the exchange happened.
    pub fn receive_at(self, question: &Question, response: String, timing: Timing) -> Transition {
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
            sent_at: timing.sent_at,
            received_at: timing.received_at,
            latency_ms: timing.latency_ms(),
        };

        let updated = record.push(attempt);

        let answers = self
            .answers
            .iter()
            .filter(|answer| answer.question_id != question.id)
            .cloned()
            .chain(std::iter::once(updated.clone()))
            .collect();

        let conversation =
            Self { state: State::Ready, answers, messages_sent: self.messages_sent + 1, ..self };

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

    /// A copy with every wall-clock value removed, for comparing two runs that
    /// should have produced the same results at different times.
    pub fn without_timing(&self) -> Self {
        let mut copy = self.clone();

        copy.started_at = None;
        copy.finished_at = None;
        copy.cooldowns.clear();

        for record in &mut copy.answers {
            for attempt in &mut record.attempts {
                attempt.sent_at = None;
                attempt.received_at = None;
                attempt.latency_ms = None;
            }
        }

        copy
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
