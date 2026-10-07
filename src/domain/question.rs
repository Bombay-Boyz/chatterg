use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct QuestionId(String);

impl QuestionId {
    pub fn new(value: impl Into<String>) -> Self {
        Self(value.into())
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum AnswerType {
    #[serde(rename = "string")]
    String,

    #[serde(rename = "text")]
    Text,

    #[serde(rename = "integer")]
    Integer,

    #[serde(rename = "list")]
    List,

    #[serde(rename = "enum")]
    Enum,
}

impl AnswerType {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::String => "string",
            Self::Text => "text",
            Self::Integer => "integer",
            Self::List => "list",
            Self::Enum => "enum",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum FailurePolicy {
    #[default]
    #[serde(rename = "abort")]
    Abort,

    #[serde(rename = "skip")]
    Skip,

    #[serde(rename = "unknown")]
    Unknown,

    #[serde(rename = "continue")]
    Continue,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Question {
    pub id: QuestionId,
    pub question: String,
    pub required: bool,

    #[serde(rename = "type")]
    pub answer_type: AnswerType,

    #[serde(default)]
    pub values: Vec<String>,

    #[serde(default)]
    pub max_followups: usize,

    #[serde(default)]
    pub on_failure: FailurePolicy,

    /// Case-insensitive phrases that make an answer count as evasive (rejected),
    /// e.g. "meeting", "schedule a call".
    #[serde(default)]
    pub reject_if_contains: Vec<String>,

    /// Text to send on retries instead of repeating `question` verbatim.
    #[serde(default)]
    pub followup: Option<String>,
}
