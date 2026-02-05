use notify::{Event, RecommendedWatcher, RecursiveMode, Watcher};
use serde::Serialize;
use std::path::Path;
use std::sync::{mpsc::channel, Arc, Mutex};
use std::time::{Duration, Instant};
use tauri::{AppHandle, Emitter};

#[derive(Clone, Serialize)]
struct WorkerCompletedPayload {
    session_id: String,
    worker_id: u8,
    task_file: String,
}

pub struct TaskFileWatcher {
    #[allow(dead_code)] // Must keep watcher alive to maintain file watching
    watcher: RecommendedWatcher,
    #[allow(dead_code)]
    session_id: String,
}

impl TaskFileWatcher {
    pub fn new(session_path: &Path, session_id: &str, app_handle: AppHandle) -> Result<Self, notify::Error> {
        let (tx, rx) = channel();
        let debounce = Duration::from_millis(500);
        let last_emit = Arc::new(Mutex::new(Instant::now() - debounce));

        let mut watcher = notify::recommended_watcher(move |res: Result<Event, _>| {
            if let Ok(event) = res {
                let _ = tx.send(event);
            }
        })?;

        // Watch the tasks directory
        let tasks_path = session_path.join("tasks");
        watcher.watch(&tasks_path, RecursiveMode::NonRecursive)?;

        let session_id_owned = session_id.to_string();
        let app_handle_clone = app_handle.clone();
        let last_emit_clone = Arc::clone(&last_emit);

        std::thread::spawn(move || {
            while let Ok(event) = rx.recv() {
                Self::handle_event(&event, &session_id_owned, &app_handle_clone, &last_emit_clone, debounce);
            }
        });

        Ok(Self {
            watcher,
            session_id: session_id.to_string(),
        })
    }

    fn is_debounced(last_emit: &Arc<Mutex<Instant>>, debounce: Duration) -> bool {
        let mut last = last_emit.lock().unwrap();
        let now = Instant::now();
        if now.duration_since(*last) < debounce {
            return true;
        }
        *last = now;
        false
    }

    fn extract_worker_id(path: &Path) -> Option<u8> {
        let filename = path.file_name()?.to_str()?;
        // Match "worker-N-task.md" pattern
        if filename.starts_with("worker-") && filename.ends_with("-task.md") {
            let num_str = filename.strip_prefix("worker-")?.strip_suffix("-task.md")?;
            num_str.parse().ok()
        } else {
            None
        }
    }

    fn handle_event(
        event: &Event,
        session_id: &str,
        app_handle: &AppHandle,
        last_emit: &Arc<Mutex<Instant>>,
        debounce: Duration,
    ) {
        for path in &event.paths {
            if let Some(filename) = path.file_name().and_then(|n| n.to_str()) {
                if filename.starts_with("worker-") && filename.ends_with("-task.md") {
                    if let Ok(content) = std::fs::read_to_string(path) {
                        if content.contains("Status: COMPLETED") || content.contains("**Status**: COMPLETED") {
                            // Extract worker ID from filename
                            if let Some(worker_id) = Self::extract_worker_id(path) {
                                // Emit worker-specific completion event
                                let payload = WorkerCompletedPayload {
                                    session_id: session_id.to_string(),
                                    worker_id,
                                    task_file: path.to_string_lossy().to_string(),
                                };
                                let _ = app_handle.emit("worker-completed", payload);
                            }

                            if Self::is_debounced(last_emit, debounce) {
                                return;
                            }
                            let _ = app_handle.emit("plan-update", session_id);
                            return;
                        }
                    }
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::TaskFileWatcher;
    use std::path::PathBuf;

    #[test]
    fn test_extract_worker_id() {
        assert_eq!(TaskFileWatcher::extract_worker_id(&PathBuf::from("worker-1-task.md")), Some(1));
        assert_eq!(TaskFileWatcher::extract_worker_id(&PathBuf::from("worker-5-task.md")), Some(5));
        assert_eq!(TaskFileWatcher::extract_worker_id(&PathBuf::from("worker-12-task.md")), Some(12));

        assert_eq!(TaskFileWatcher::extract_worker_id(&PathBuf::from("worker-task.md")), None);
        assert_eq!(TaskFileWatcher::extract_worker_id(&PathBuf::from("planner-1-task.md")), None);
        assert_eq!(TaskFileWatcher::extract_worker_id(&PathBuf::from("worker-1.md")), None);
    }
}
