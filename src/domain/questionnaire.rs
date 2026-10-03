use std::{collections::HashSet, path::Path};

use serde::{Deserialize, Serialize};

use super::{error::DomainError, question::Question};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Questionnaire {
    pub questions: Vec<Question>,
}

impl Questionnaire {
    /// Loads and validates a questionnaire. Never falls back to a built-in one.
    pub fn from_path(path: impl AsRef<Path>) -> Result<Self, DomainError> {
        let path = path.as_ref();

        let data = std::fs::read_to_string(path)
            .map_err(|source| DomainError::QuestionnaireRead { path: path.to_owned(), source })?;

        let questionnaire: Self = serde_yaml::from_str(&data)
            .map_err(|source| DomainError::QuestionnaireParse { path: path.to_owned(), source })?;

        questionnaire.validate()?;
        Ok(questionnaire)
    }

    /// Rejects empty questionnaires and duplicate question ids.
    pub fn validate(&self) -> Result<(), DomainError> {
        if self.is_empty() {
            return Err(DomainError::EmptyQuestionnaire);
        }

        let mut seen = HashSet::new();
        for question in &self.questions {
            if !seen.insert(question.id.as_str()) {
                return Err(DomainError::DuplicateQuestionId(question.id.as_str().to_owned()));
            }
        }

        Ok(())
    }

    pub fn required(&self) -> impl Iterator<Item = &Question> {
        self.questions.iter().filter(|question| question.required)
    }

    pub fn is_empty(&self) -> bool {
        self.questions.is_empty()
    }
}
