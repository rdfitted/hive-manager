use notify::{Event, RecommendedWatcher, RecursiveMode, Watcher};
use std::path::Path;
use std::sync::{mpsc::channel, Arc, Mutex};
use std::time::{Duration, Instant};
use tauri::{AppHandle, Emitter};

pub struct TaskFileWatcher {
    watcher: RecommendedWatcher,
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
