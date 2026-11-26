//! `PtyManager` accessors for the folder-trust prompt sweep (#466).

use super::{PtyError, PtyManager};
use crate::pty::output_ring::OutputSnapshot;

impl PtyManager {
    pub(super) fn record_trust_spawn(&self, id: &str, command: &str, args: &[&str], size: (u16, u16)) {
        self.pty_sizes.write().insert(id.to_string(), size);
        let mut opt_outs = self.trust_opt_outs.write();
        if crate::cli::folder_trust::operator_sets_trust(command, args) {
            opt_outs.insert(id.to_string());
        } else {
            opt_outs.remove(id);
        }
    }

    pub(super) fn forget_trust_spawn(&self, id: &str) {
        self.pty_sizes.write().remove(id);
        self.trust_opt_outs.write().remove(id);
    }

    /// Whether this spawn's own arguments set folder trust, so its prompt is the
    /// operator's to answer.
    pub fn operator_sets_trust(&self, id: &str) -> bool {
        self.trust_opt_outs.read().contains(id)
    }

    /// The PTY's size as last spawned or resized.
    pub fn pty_size(&self, id: &str) -> Option<(u16, u16)> {
        self.pty_sizes.read().get(id).copied()
    }

    /// `id`'s output from `offset` on (`OutputRing::since`), only while `instance` runs it.
    pub fn output_since(&self, id: &str, instance: uuid::Uuid, offset: u64) -> Option<OutputSnapshot> {
        // Sessions before instances, the same order as `create_session`.
        let sessions = self.sessions.read();
        self.instances.read().get(id).filter(|current| current.id == instance)?;
        let ring = sessions.get(id)?.output_history();
        drop(sessions);
        let since = ring.lock().since(offset);
        Some(since)
    }

    /// Write to `instance` only if no other input reached it after `revision` (from
    /// [`PtyManager::input_fence`]), so an answer read off an earlier frame is never sent
    /// after the operator has typed.
    pub fn write_instance_at_revision(&self, id: &str, instance: uuid::Uuid, revision: u64, data: &[u8]) -> Result<(), PtyError> {
        let (session, _admission) = self.admit_input_instance(id, Some(instance), Some(revision))?;
        session.write(data)
    }
}
