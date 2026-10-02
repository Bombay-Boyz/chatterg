use super::{
    conversation::{Conversation, Transition},
    question::{FailurePolicy, Question},
    questionnaire::Questionnaire,
};

#[derive(Debug, Clone, PartialEq)]
pub enum EngineState {
    Ready,
    Asking(Question),
    Complete,
}

#[derive(Debug, Clone)]
pub struct Engine {
    questionnaire: Questionnaire,
    position: usize,
    conversation: Conversation,
}

impl Engine {
    pub fn new(questionnaire: Questionnaire) -> Self {
        Self { questionnaire, position: 0, conversation: Conversation::default() }
    }

    pub fn start(&self) -> EngineState {
        self.current().cloned().map(EngineState::Asking).unwrap_or(EngineState::Complete)
    }

    pub fn submit(self, response: String) -> Submission {
        let Some(question) = self.current().cloned() else {
            return Submission::Complete;
        };

        let Self { questionnaire, position, conversation } = self;

        match conversation.receive(&question, response) {
            Transition::Accepted(conversation) => {
                Self { questionnaire, position, conversation }.advance()
            }

            Transition::FollowUp(conversation) => {
                Submission::Retry(Self { questionnaire, position, conversation }, question)
            }

            Transition::Rejected(conversation)
            | Transition::Unverified(conversation)
            | Transition::Unknown(conversation) => {
                Self { questionnaire, position, conversation }.apply_failure(question)
            }
        }
    }

    pub fn conversation(&self) -> &Conversation {
        &self.conversation
    }

    fn advance(self) -> Submission {
        let next = Self {
            questionnaire: self.questionnaire,
            position: self.position + 1,
            conversation: self.conversation,
        };

        match next.current().cloned() {
            Some(question) => Submission::Next(next, question),
            None => Submission::Complete,
        }
    }

    fn apply_failure(self, question: Question) -> Submission {
        match question.on_failure {
            FailurePolicy::Abort => Submission::Aborted(self),

            FailurePolicy::Skip | FailurePolicy::Continue => self.advance(),

            FailurePolicy::Unknown => Submission::Unknown(self),
        }
    }

    fn current(&self) -> Option<&Question> {
        self.questionnaire.questions.get(self.position)
    }
}

#[derive(Debug, Clone)]
pub enum Submission {
    Next(Engine, Question),
    Retry(Engine, Question),
    Aborted(Engine),
    Unknown(Engine),
    Complete,
}
