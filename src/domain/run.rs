//! Metadata about *how* a run went: when, how long, against whom, with which questions.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

pub type Timestamp = DateTime<Utc>;

/// Wall-clock times around one exchange. Missing when the caller did not measure.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Timing {
    pub sent_at: Option<Timestamp>,
    pub received_at: Option<Timestamp>,
}

impl Timing {
    pub fn new(sent_at: Timestamp, received_at: Timestamp) -> Self {
        Self { sent_at: Some(sent_at), received_at: Some(received_at) }
    }

    /// Never negative, even if the clock moved backwards.
    pub fn latency_ms(&self) -> Option<i64> {
        match (self.sent_at, self.received_at) {
            (Some(sent), Some(received)) => Some((received - sent).num_milliseconds().max(0)),
            _ => None,
        }
    }
}

/// One pause taken because the agent stopped answering.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CooldownEvent {
    pub at: Timestamp,
    pub reason: String,
    pub seconds: u64,
}

/// Who the questions were put to.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AgentInfo {
    pub name: Option<String>,
    /// The URL the user gave.
    pub target: String,
    /// The endpoint messages were actually sent to.
    pub endpoint: String,
    pub protocol: String,
}

/// What a question looked like when the run started, so a changed questions
/// file can be detected and described.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct QuestionSnapshot {
    pub id: String,
    pub question: String,
    pub kind: String,
    #[serde(default)]
    pub values: Vec<String>,
}

/// How the current questions differ from the snapshot taken at the start.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct QuestionnaireChange {
    pub added: Vec<String>,
    pub removed: Vec<String>,
    pub edited: Vec<String>,
    pub reordered: bool,
}

impl QuestionnaireChange {
    pub fn between(stored: &[QuestionSnapshot], current: &[QuestionSnapshot]) -> Self {
        let find = |list: &'_ [QuestionSnapshot], id: &str| list.iter().position(|q| q.id == id);

        let added = current.iter().filter(|q| find(stored, &q.id).is_none()).map(|q| q.id.clone());
        let removed =
            stored.iter().filter(|q| find(current, &q.id).is_none()).map(|q| q.id.clone());

        let edited = stored
            .iter()
            .filter_map(|old| find(current, &old.id).map(|index| (old, &current[index])))
            .filter(|(old, new)| old != new)
            .map(|(old, _)| old.id.clone());

        let common_stored: Vec<_> =
            stored.iter().filter(|q| find(current, &q.id).is_some()).map(|q| &q.id).collect();
        let common_current: Vec<_> =
            current.iter().filter(|q| find(stored, &q.id).is_some()).map(|q| &q.id).collect();

        Self {
            added: added.collect(),
            removed: removed.collect(),
            edited: edited.collect(),
            reordered: common_stored != common_current,
        }
    }

    pub fn is_empty(&self) -> bool {
        self.added.is_empty()
            && self.removed.is_empty()
            && self.edited.is_empty()
            && !self.reordered
    }
}

impl std::fmt::Display for QuestionnaireChange {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        fn part(label: &str, ids: &[String]) -> Option<String> {
            if ids.is_empty() {
                return None;
            }

            let shown: Vec<_> = ids.iter().take(3).cloned().collect();
            let more = if ids.len() > 3 { ", ..." } else { "" };
            Some(format!("{} {label} ({}{more})", ids.len(), shown.join(", ")))
        }

        let mut parts: Vec<String> = [
            part("added", &self.added),
            part("removed", &self.removed),
            part("edited", &self.edited),
        ]
        .into_iter()
        .flatten()
        .collect();

        if self.reordered {
            parts.push("order changed".to_owned());
        }

        if parts.is_empty() {
            f.write_str("settings or text changed")
        } else {
            f.write_str(&parts.join(", "))
        }
    }
}
