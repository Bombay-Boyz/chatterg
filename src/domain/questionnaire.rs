use serde::{Deserialize, Serialize};

use super::question::Question;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Questionnaire {
    pub questions: Vec<Question>,
}

impl Questionnaire {
    pub fn required(&self) -> impl Iterator<Item = &Question> {
        self.questions.iter().filter(|question| question.required)
    }

    pub fn is_empty(&self) -> bool {
        self.questions.is_empty()
    }
}
