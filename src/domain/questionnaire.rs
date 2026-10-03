use serde::{Deserialize, Serialize};

use super::question::Question;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Questionnaire {
    pub questions: Vec<Question>,
}

impl Questionnaire {
    pub fn from_path(
        path: impl AsRef<std::path::Path>,
    ) -> Result<Self, Box<dyn std::error::Error>> {
        let data = std::fs::read_to_string(path)?;
        Ok(serde_yaml::from_str(&data)?)
    }

    pub fn required(&self) -> impl Iterator<Item = &Question> {
        self.questions.iter().filter(|question| question.required)
    }

    pub fn is_empty(&self) -> bool {
        self.questions.is_empty()
    }
}
