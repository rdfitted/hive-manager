//! `PtyManager` pieces for folder-trust handling (#466).

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use super::PtyManager;
use crate::pty::output_ring::OutputSnapshot;

/// Looks up a session's recorded project directory by session id.
pub type SessionProjects = Arc<dyn Fn(&str) -> Option<PathBuf> + Send + Sync>;

/// How long a spawn waits for the session roster before giving up on pre-trust.
const PROJECT_LOOKUP_WAIT: Duration = Duration::from_millis(50);

impl PtyManager {
    /// Install the lookup the spawn path uses to check pre-trust against a session's
    /// recorded project. `SessionController::new` installs one over its roster.
    pub fn set_session_projects(&self, lookup: SessionProjects) {
        *self.session_projects.write() = Some(lookup);
    }

    /// Pre-trust flags for a spawn, only for the seat's own managed worktree of its
    /// session's recorded project. No lookup, or no answer, means no flags.
    pub(super) fn spawn_trust_args(&self, command: &str, id: &str, cwd: Option<&str>, store_dir: &Path, args: &[&str]) -> Vec<String> {
        let lookup = self.session_projects.read().clone();
        let project_of = |session_id: &str| {
            let project = lookup.as_ref().and_then(|lookup| lookup(session_id));
            if project.is_none() {
                tracing::info!(agent = %id, session = %session_id,
                    "no folder pre-trust: the session's project is unknown or its roster was busy");
            }
            project
        };
        crate::cli::folder_trust::spawn_trust_args(command, id, cwd, &project_of, store_dir, args)
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

    /// Run `decide` on `id`'s current instance while it is held: a same-id spawn
    /// publishes its instance under the write lock, so none can land until `decide`
    /// returns. `decide` must not call back into this manager.
    pub fn with_instance_held<T>(&self, id: &str, decide: impl FnOnce(Option<uuid::Uuid>) -> T) -> T {
        let instances = self.instances.read();
        decide(instances.get(id).map(|current| current.id))
    }
}

/// A [`SessionProjects`] over a controller roster, held weakly. It waits briefly for the
/// roster lock and answers `None` rather than block a spawn made while it is held.
pub fn roster_projects<S: Send + Sync + 'static>(
    roster: std::sync::Weak<parking_lot::RwLock<std::collections::HashMap<String, S>>>,
    project: fn(&S) -> PathBuf,
) -> SessionProjects {
    Arc::new(move |session_id| {
        let roster = roster.upgrade()?;
        let sessions = roster.try_read_for(PROJECT_LOOKUP_WAIT)?;
        sessions.get(session_id).map(project)
    })
}
