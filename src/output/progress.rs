//! A running estimate of how long a run still has to go.

use std::time::{Duration, Instant};

/// Watches how fast questions are being finished and predicts the rest.
///
/// It only counts questions finished since *this* session began, so resuming a
/// run does not distort the estimate, and it waits for a few data points before
/// saying anything.
#[derive(Debug, Clone)]
pub struct Eta {
    first: Option<(usize, Instant)>,
}

/// Questions that must be finished in this session before an estimate is shown.
const MIN_SAMPLES: usize = 2;

impl Default for Eta {
    fn default() -> Self {
        Self::new()
    }
}

impl Eta {
    pub fn new() -> Self {
        Self { first: None }
    }

    /// Call whenever question `number` (1-based, of `total`) is about to be asked.
    pub fn observe(&mut self, number: usize, total: usize, now: Instant) -> Option<Duration> {
        let (first_number, started) = *self.first.get_or_insert((number, now));

        let finished = number.checked_sub(first_number).filter(|done| *done >= MIN_SAMPLES)?;
        let per_question = now.checked_duration_since(started)? / u32::try_from(finished).ok()?;
        let remaining = total.checked_sub(number)? + 1; // this question is still to come

        per_question.checked_mul(u32::try_from(remaining).ok()?)
    }
}

/// "11h 41m", "35m", "40s": rough on purpose.
pub fn approximately(duration: Duration) -> String {
    let seconds = duration.as_secs();
    let (hours, minutes) = (seconds / 3600, (seconds % 3600) / 60);

    match (hours, minutes) {
        (0, 0) => format!("{seconds}s"),
        (0, minutes) => format!("{minutes}m"),
        (hours, minutes) => format!("{hours}h {minutes:02}m"),
    }
}
