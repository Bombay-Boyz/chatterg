use super::{
    conversation::{Conversation, State, Transition},
    error::DomainError,
    question::{FailurePolicy, Question},
    questionnaire::Questionnaire,
};

#[derive(Debug, Clone, PartialEq)]
pub enum EngineState {
    Ready,
    Asking(Question),
    Complete,
    Aborted,
}

/// Drives a questionnaire. The position lives in [`Conversation`] so that it is
/// persisted atomically with the answers.
#[derive(Debug, Clone)]
pub struct Engine {
    questionnaire: Questionnaire,
    conversation: Conversation,
}

impl Engine {
    pub fn new(questionnaire: Questionnaire) -> Self {
        Self { questionnaire, conversation: Conversation::default() }
    }

    /// Rebuilds an engine from persisted state, continuing exactly where it stopped.
    pub fn resume(
        questionnaire: Questionnaire,
        conversation: Conversation,
    ) -> Result<Self, DomainError> {
        let len = questionnaire.questions.len();

        if conversation.position > len {
            return Err(DomainError::InvalidPosition { position: conversation.position, len });
        }

        for record in &conversation.answers {
            if !questionnaire.questions.iter().any(|question| question.id == record.question_id) {
                return Err(DomainError::QuestionNotFound(record.question_id.as_str().to_owned()));
            }
        }

        Ok(Self { questionnaire, conversation })
    }

    pub fn start(&self) -> EngineState {
        match self.conversation.state {
            State::Complete => EngineState::Complete,
            State::Aborted => EngineState::Aborted,
            _ => self.current().cloned().map(EngineState::Asking).unwrap_or(EngineState::Complete),
        }
    }

    pub fn position(&self) -> usize {
        self.conversation.position
    }

    pub fn submit(self, response: String) -> Submission {
        let question = match self.start() {
            EngineState::Asking(question) => question,
            EngineState::Aborted => return Submission::Aborted(self),
            EngineState::Ready | EngineState::Complete => return Submission::Complete(self),
        };

        let Self { questionnaire, conversation } = self;

        match conversation.receive(&question, response) {
            Transition::Accepted(conversation) => Self { questionnaire, conversation }.advance(),

            Transition::FollowUp(conversation) => {
                Submission::Retry(Self { questionnaire, conversation }, question)
            }

            Transition::Rejected(conversation)
            | Transition::Unverified(conversation)
            | Transition::Unknown(conversation) => {
                Self { questionnaire, conversation }.apply_failure(question)
            }
        }
    }

    pub fn conversation(&self) -> &Conversation {
        &self.conversation
    }

    fn advance(mut self) -> Submission {
        self.conversation.position += 1;

        match self.current().cloned() {
            Some(question) => Submission::Next(self, question),
            None => {
                self.conversation.state = State::Complete;
                Submission::Complete(self)
            }
        }
    }

    fn abort(mut self) -> Self {
        self.conversation.state = State::Aborted;
        self
    }

    fn apply_failure(self, question: Question) -> Submission {
        match question.on_failure {
            FailurePolicy::Abort => Submission::Aborted(self.abort()),
            FailurePolicy::Skip | FailurePolicy::Continue => self.advance(),
            FailurePolicy::Unknown => Submission::Unknown(self.abort()),
        }
    }

    fn current(&self) -> Option<&Question> {
        self.questionnaire.questions.get(self.conversation.position)
    }
}

#[derive(Debug, Clone)]
pub enum Submission {
    Next(Engine, Question),
    Retry(Engine, Question),
    Aborted(Engine),
    Unknown(Engine),
    Complete(Engine),
}
