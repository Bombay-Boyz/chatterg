//! A cheap "please stop" signal shared between the part of the program that
//! hears Ctrl+C and the part that is busy asking questions.

use std::sync::Arc;

use tokio::sync::watch;

#[derive(Clone)]
pub struct Cancel {
    state: Arc<watch::Sender<bool>>,
}

impl Default for Cancel {
    fn default() -> Self {
        let (state, _) = watch::channel(false);
        Self { state: Arc::new(state) }
    }
}

impl Cancel {
    /// Asks everything that is waiting on this signal to stop. Safe to call more than once.
    pub fn cancel(&self) {
        self.state.send_replace(true);
    }

    pub fn is_cancelled(&self) -> bool {
        *self.state.borrow()
    }

    /// Completes as soon as `cancel` has been called (immediately if it already was).
    pub async fn cancelled(&self) {
        let mut changes = self.state.subscribe();

        while !*changes.borrow_and_update() {
            if changes.changed().await.is_err() {
                return;
            }
        }
    }
}
