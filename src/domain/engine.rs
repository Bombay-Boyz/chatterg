use super::{
    conversation::{Conversation, State, Transition},
    error::DomainError,
    question::{FailurePolicy, Question},
    questionnaire::{Questionnaire, fingerprint_of},
    run::{AgentInfo, CooldownEvent, QuestionSnapshot, QuestionnaireChange, Timestamp, Timing},
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
        let conversation = Conversation {
            questionnaire_fingerprint: Some(questionnaire.fingerprint()),
            questionnaire_snapshot: questionnaire.snapshot(),
            ..Conversation::default()
        };

        Self { questionnaire, conversation }
    }

    /// Rebuilds an engine from persisted state, continuing exactly where it stopped.
    ///
    /// Fails if the questions changed since the run started.
    pub fn resume(
        questionnaire: Questionnaire,
        conversation: Conversation,
    ) -> Result<Self, DomainError> {
        Self::resume_with(questionnaire, conversation, false)
    }

    /// Like [`resume`](Self::resume), but accepts a changed questions file as long
    /// as every question already asked (and the one in progress) is unchanged.
    pub fn resume_forced(
        questionnaire: Questionnaire,
        conversation: Conversation,
    ) -> Result<Self, DomainError> {
        Self::resume_with(questionnaire, conversation, true)
    }

    fn resume_with(
        questionnaire: Questionnaire,
        mut conversation: Conversation,
        force: bool,
    ) -> Result<Self, DomainError> {
        let current = questionnaire.snapshot();
        let fingerprint = fingerprint_of(&current);

        // Conversations from before fingerprints existed have none: adopt the current one.
        if let Some(stored) = &conversation.questionnaire_fingerprint
            && *stored != fingerprint
        {
            let summary =
                QuestionnaireChange::between(&conversation.questionnaire_snapshot, &current)
                    .to_string();

            if !asked_questions_unchanged(&conversation, &current) {
                return Err(DomainError::QuestionnaireChangedUnsafe { summary });
            }

            if !force {
                return Err(DomainError::QuestionnaireChanged { summary });
            }
        }

        conversation.questionnaire_fingerprint = Some(fingerprint);
        conversation.questionnaire_snapshot = current;

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

    pub fn total_questions(&self) -> usize {
        self.questionnaire.questions.len()
    }

    pub fn position(&self) -> usize {
        self.conversation.position
    }

    pub fn submit(self, response: String) -> Submission {
        self.submit_timed(response, Timing::default())
    }

    /// Like [`submit`](Self::submit), recording when the exchange happened.
    pub fn submit_timed(self, response: String, timing: Timing) -> Submission {
        let question = match self.start() {
            EngineState::Asking(question) => question,
            EngineState::Aborted => return Submission::Aborted(self),
            EngineState::Ready | EngineState::Complete => return Submission::Complete(self),
        };

        let Self { questionnaire, conversation } = self;

        match conversation.receive_at(&question, response, timing) {
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

    /// Marks the start of the run (only the first time).
    pub fn begin(&mut self, at: Timestamp) {
        self.conversation.started_at.get_or_insert(at);
    }

    /// Remembers who is being asked (only the first time).
    pub fn set_agent(&mut self, agent: AgentInfo) {
        self.conversation.agent.get_or_insert(agent);
    }

    pub fn record_cooldowns(&mut self, events: Vec<CooldownEvent>) {
        self.conversation.cooldowns.extend(events);
    }

    /// Stamps the end of a finished (complete or aborted) run.
    pub fn finish(mut self, at: Timestamp) -> Self {
        if matches!(self.conversation.state, State::Complete | State::Aborted) {
            self.conversation.finished_at.get_or_insert(at);
        }

        self
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

/// True when every question already asked, plus the one in progress, is exactly
/// as it was when the run started. Later questions may have changed freely.
fn asked_questions_unchanged(conversation: &Conversation, current: &[QuestionSnapshot]) -> bool {
    let stored = &conversation.questionnaire_snapshot;
    let asked = (conversation.position + 1).min(stored.len());

    current.len() >= asked && stored[..asked] == current[..asked]
}
