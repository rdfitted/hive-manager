use crate::delivery::{Delivery, ProtocolConfirmation};
use std::collections::VecDeque;

/// A response supplies identity, never confirmation. Early notifications are bounded.
pub(crate) struct TurnCorrelation {
    thread: String,
    confirmation: Option<ProtocolConfirmation>,
    expected: Option<String>,
    early: VecDeque<(String, String, bool)>,
    completed: bool,
}

impl TurnCorrelation {
    pub fn new(thread: String) -> Self {
        Self {
            thread,
            confirmation: None,
            expected: None,
            early: VecDeque::new(),
            completed: false,
        }
    }
    pub fn response(&mut self, turn: &str) {
        if turn.is_empty() {
            return;
        }
        self.confirmation = Some(ProtocolConfirmation::new(self.thread.clone(), turn.into()));
        self.expected = Some(turn.into());
        let early = std::mem::take(&mut self.early);
        for (thread, turn, completed) in early {
            self.observe(&thread, &turn, completed);
        }
    }
    pub fn observe(&mut self, thread: &str, turn: &str, completed: bool) {
        if thread != self.thread || turn.is_empty() {
            return;
        }
        if let Some(confirmation) = &self.confirmation {
            // observe_started owns the authoritative two-ID check.
            if completed {
                // Completion must match, but cannot manufacture a started signal.
                self.completed |= self.expected.as_deref() == Some(turn);
            } else {
                // The thread boundary was checked above, including early events.
                confirmation.observe_started(&self.thread, turn);
            }
        } else {
            if self.early.len() == 32 {
                self.early.pop_front();
            }
            self.early
                .push_back((thread.into(), turn.into(), completed));
        }
    }
    pub fn delivery(&self) -> Delivery {
        self.confirmation
            .as_ref()
            .map(ProtocolConfirmation::delivery)
            .unwrap_or_else(Delivery::protocol_pending)
    }
    pub fn completed(&self) -> bool {
        self.completed
    }
}
