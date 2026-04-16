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

#[derive(Clone, Serialize)]
struct FusionVariantCompletedPayload {
    session_id: String,
    variant_index: u8,
    task_file: String,
}

#[derive(Clone, Serialize)]
struct AgentTaskCompletedPayload {
    session_id: String,
    agent_id: String,
    task_file: String,
}

#[derive(Clone, Serialize)]
struct PeerEventPayload {
    session_id: String,
    event_type: String,
    path: String,
}

pub struct TaskFileWatcher {
    #[allow(dead_code)] // Must keep watcher alive to maintain file watching
    watcher: RecommendedWatcher,
    #[allow(dead_code)]
    session_id: String,
}

impl TaskFileWatcher {
    pub fn new(
        session_path: &Path,
        worktrees_path: &Path,
        fusion_worktrees_path: &Path,
        session_id: &str,
        app_handle: AppHandle,
    ) -> Result<Self, notify::Error> {
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
        std::fs::create_dir_all(&tasks_path).ok();
        watcher.watch(&tasks_path, RecursiveMode::NonRecursive)?;
        if worktrees_path.exists() {
            watcher.watch(worktrees_path, RecursiveMode::Recursive)?;
        }
        if fusion_worktrees_path.exists() {
            watcher.watch(fusion_worktrees_path, RecursiveMode::Recursive)?;
        }
        let peer_path = session_path.join("peer");
        std::fs::create_dir_all(&peer_path).ok();
        watcher.watch(&peer_path, RecursiveMode::NonRecursive)?;
        let contracts_path = session_path.join("contracts");
        std::fs::create_dir_all(&contracts_path).ok();
        watcher.watch(&contracts_path, RecursiveMode::NonRecursive)?;

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

    fn extract_fusion_variant(path: &Path) -> Option<u8> {
        let filename = path.file_name()?.to_str()?;
        // Match "fusion-variant-N-task.md" pattern
        if filename.starts_with("fusion-variant-") && filename.ends_with("-task.md") {
            let suffix = filename.strip_prefix("fusion-variant-")?;
            let num_end = suffix.strip_suffix("-task.md")?;
            return num_end.parse::<u8>().ok();
        }
        None
    }

    fn extract_evaluator_id(path: &Path) -> Option<String> {
        let filename = path.file_name()?.to_str()?;
        if filename == "evaluator-task.md" {
            return Some("evaluator".to_string());
        }
        if filename.starts_with("qa-worker-") && filename.ends_with("-task.md") {
            let index = filename
                .strip_prefix("qa-worker-")?
                .strip_suffix("-task.md")?;
            return Some(format!("qa-worker-{}", index));
        }
        None
    }

    fn peer_event_type(path: &Path) -> Option<&'static str> {
        match path.file_name()?.to_str()? {
            "milestone-ready.json" => Some("milestone-ready"),
            "qa-verdict.json" => Some("qa-verdict"),
            "evaluator-feedback.json" => Some("evaluator-feedback"),
            _ => None,
        }
    }

    fn contract_event_type(path: &Path) -> Option<&'static str> {
        let filename = path.file_name()?.to_str()?;
        if filename.starts_with("milestone-") && filename.ends_with(".md") {
            Some("contract-created")
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
        let mut should_emit_plan_update = false;

        for path in &event.paths {
            if let Some(event_type) = Self::contract_event_type(path) {
                let _ = app_handle.emit(
                    event_type,
                    PeerEventPayload {
                        session_id: session_id.to_string(),
                        event_type: event_type.to_string(),
                        path: path.to_string_lossy().to_string(),
                    },
                );
                should_emit_plan_update = true;
                continue;
            }

            if let Some(event_type) = Self::peer_event_type(path) {
                let _ = app_handle.emit(
                    event_type,
                    PeerEventPayload {
                        session_id: session_id.to_string(),
                        event_type: event_type.to_string(),
                        path: path.to_string_lossy().to_string(),
                    },
                );
                should_emit_plan_update = true;
                continue;
            }

            let worker_id = Self::extract_worker_id(path);
            let fusion_variant_index = Self::extract_fusion_variant(path);
            let evaluator_agent_id = Self::extract_evaluator_id(path);
            if worker_id.is_none() && fusion_variant_index.is_none() && evaluator_agent_id.is_none() {
                continue;
            }

            if let Ok(content) = std::fs::read_to_string(path) {
                if content.contains("Status: COMPLETED") || content.contains("**Status**: COMPLETED") {
                    let task_file = path.to_string_lossy().to_string();

                    if let Some(worker_id) = worker_id {
                        let payload = WorkerCompletedPayload {
                            session_id: session_id.to_string(),
                            worker_id,
                            task_file: task_file.clone(),
                        };
                        let _ = app_handle.emit("worker-completed", payload);
                    }

                    if let Some(variant_index) = fusion_variant_index {
                        let payload = FusionVariantCompletedPayload {
                            session_id: session_id.to_string(),
                            variant_index,
                            task_file: task_file.clone(),
                        };
                        let _ = app_handle.emit("fusion-variant-completed", payload);
                    }

                    if let Some(agent_id) = evaluator_agent_id {
                        let payload = AgentTaskCompletedPayload {
                            session_id: session_id.to_string(),
                            agent_id,
                            task_file: task_file.clone(),
                        };
                        let _ = app_handle.emit("evaluator-task-completed", payload);
                    }

                    should_emit_plan_update = true;
                }
            }
        }

        if should_emit_plan_update && !Self::is_debounced(last_emit, debounce) {
            let _ = app_handle.emit("plan-update", session_id);
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

    #[test]
    fn test_extract_fusion_variant() {
        assert_eq!(TaskFileWatcher::extract_fusion_variant(&PathBuf::from("fusion-variant-1-task.md")), Some(1));
        assert_eq!(TaskFileWatcher::extract_fusion_variant(&PathBuf::from("fusion-variant-5-task.md")), Some(5));
        assert_eq!(TaskFileWatcher::extract_fusion_variant(&PathBuf::from("fusion-variant-12-task.md")), Some(12));

        assert_eq!(TaskFileWatcher::extract_fusion_variant(&PathBuf::from("fusion-variant-")), None);
        assert_eq!(TaskFileWatcher::extract_fusion_variant(&PathBuf::from("fusion-variant-foo-task.md")), None);
        assert_eq!(TaskFileWatcher::extract_fusion_variant(&PathBuf::from("fusion-task.md")), None);
        assert_eq!(TaskFileWatcher::extract_fusion_variant(&PathBuf::from("fusion-variant-1.md")), None);
    }

    #[test]
    fn test_extract_evaluator_id() {
        assert_eq!(
            TaskFileWatcher::extract_evaluator_id(&PathBuf::from("evaluator-task.md")),
            Some("evaluator".to_string())
        );
        assert_eq!(
            TaskFileWatcher::extract_evaluator_id(&PathBuf::from("qa-worker-3-task.md")),
            Some("qa-worker-3".to_string())
        );
        assert_eq!(
            TaskFileWatcher::extract_evaluator_id(&PathBuf::from("worker-3-task.md")),
            None
        );
    }

    #[test]
    fn test_peer_event_type() {
        assert_eq!(
            TaskFileWatcher::peer_event_type(&PathBuf::from("milestone-ready.json")),
            Some("milestone-ready")
        );
        assert_eq!(
            TaskFileWatcher::peer_event_type(&PathBuf::from("qa-verdict.json")),
            Some("qa-verdict")
        );
        assert_eq!(
            TaskFileWatcher::peer_event_type(&PathBuf::from("evaluator-feedback.json")),
            Some("evaluator-feedback")
        );
        assert_eq!(
            TaskFileWatcher::peer_event_type(&PathBuf::from("other.json")),
            None
        );
    }

    #[test]
    fn test_contract_event_type() {
        assert_eq!(
            TaskFileWatcher::contract_event_type(&PathBuf::from("milestone-1.md")),
            Some("contract-created")
        );
        assert_eq!(
            TaskFileWatcher::contract_event_type(&PathBuf::from("milestone-final.md")),
            Some("contract-created")
        );
        assert_eq!(
            TaskFileWatcher::contract_event_type(&PathBuf::from("milestone-1.json")),
            None
        );
    }
}
