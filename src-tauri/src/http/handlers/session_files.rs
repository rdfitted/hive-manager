use std::fs;
use std::io::Read;
use std::path::{Path as FsPath, PathBuf};
use std::sync::Arc;

use axum::{
    extract::{Path, Query, State},
    http::StatusCode,
    Json,
};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use crate::http::{error::ApiError, state::AppState};
use crate::storage::{canonicalize_within, StorageError};

use super::validate_session_id;

const MAX_FILE_SIZE: usize = 500 * 1024;
const BINARY_SNIFF_SIZE: usize = 8 * 1024;

#[derive(Debug, Serialize)]
pub struct SessionFilesResponse {
    pub files: Vec<SessionFileEntry>,
}

#[derive(Debug, Serialize)]
pub struct SessionFileEntry {
    pub path: String,
    pub name: String,
    pub is_dir: bool,
    pub size: u64,
    pub modified: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct SessionFileContentQuery {
    pub path: String,
}

#[derive(Debug, Serialize)]
pub struct SessionFileContentResponse {
    pub path: String,
    pub content: String,
    pub size: usize,
}

pub async fn list_session_files(
    State(state): State<Arc<AppState>>,
    Path(session_id): Path<String>,
) -> Result<Json<SessionFilesResponse>, ApiError> {
    validate_session_id(&session_id)?;
    let root = resolve_session_files_root(&state, &session_id)?;
    let mut files = Vec::new();
    collect_session_files(&root, &root, &mut files).map_err(map_path_error)?;
    files.sort_by(|left, right| left.path.cmp(&right.path));
    Ok(Json(SessionFilesResponse { files }))
}

pub async fn read_session_file(
    State(state): State<Arc<AppState>>,
    Path(session_id): Path<String>,
    Query(query): Query<SessionFileContentQuery>,
) -> Result<Json<SessionFileContentResponse>, ApiError> {
    validate_session_id(&session_id)?;
    if query.path.trim().is_empty() || query.path.contains('\0') {
        return Err(ApiError::bad_request("File path cannot be empty or contain NUL"));
    }

    let root = resolve_session_files_root(&state, &session_id)?;
    let requested_path = FsPath::new(&query.path);
    let safe_path = canonicalize_within(&root, requested_path).map_err(map_path_error)?;
    let mut file = fs::File::open(&safe_path).map_err(|error| map_io_error(error, &query.path))?;
    let metadata = file
        .metadata()
        .map_err(|error| map_io_error(error, &query.path))?;
    if !metadata.is_file() {
        return Err(ApiError::bad_request("Requested path is not a file"));
    }
    if metadata.len() > MAX_FILE_SIZE as u64 {
        return Err(ApiError::new(
            StatusCode::PAYLOAD_TOO_LARGE,
            format!("File exceeds the {} byte read limit", MAX_FILE_SIZE),
        ));
    }

    let mut bytes = Vec::with_capacity(metadata.len() as usize);
    file.by_ref()
        .take((MAX_FILE_SIZE + 1) as u64)
        .read_to_end(&mut bytes)
        .map_err(|error| map_io_error(error, &query.path))?;
    if bytes.len() > MAX_FILE_SIZE {
        return Err(ApiError::new(
            StatusCode::PAYLOAD_TOO_LARGE,
            format!("File exceeds the {} byte read limit", MAX_FILE_SIZE),
        ));
    }
    if bytes
        .iter()
        .take(BINARY_SNIFF_SIZE)
        .any(|byte| *byte == 0)
    {
        return Err(ApiError::new(
            StatusCode::UNSUPPORTED_MEDIA_TYPE,
            "Binary files cannot be displayed",
        ));
    }

    let size = bytes.len();
    let content = String::from_utf8(bytes).map_err(|_| {
        ApiError::new(
            StatusCode::UNSUPPORTED_MEDIA_TYPE,
            "Non-UTF-8 files cannot be displayed",
        )
    })?;
    Ok(Json(SessionFileContentResponse {
        path: normalize_relative_path(requested_path),
        content,
        size,
    }))
}

fn resolve_session_files_root(state: &AppState, session_id: &str) -> Result<PathBuf, ApiError> {
    let live_project_path = state
        .session_controller
        .read()
        .get_session(session_id)
        .map(|session| session.project_path);

    let project_path = match live_project_path {
        Some(path) => Some(path),
        None => match state.storage.load_session(session_id) {
            Ok(session) => Some(PathBuf::from(session.project_path)),
            Err(StorageError::SessionNotFound(_)) => None,
            Err(error) => return Err(ApiError::internal(error.to_string())),
        },
    };

    if let Some(project_path) = project_path.as_ref() {
        let project_hive_dir = project_path.join(".hive-manager");
        let project_session_dir = project_hive_dir.join(session_id);
        if project_session_dir.is_dir() {
            let safe_hive_dir = canonicalize_within(project_path, FsPath::new(".hive-manager"))
                .map_err(map_path_error)?;
            return canonicalize_within(&safe_hive_dir, FsPath::new(session_id))
                .map_err(map_path_error);
        }
    }

    let fallback = state.storage.session_dir(session_id);
    if fallback.is_dir() {
        let safe_sessions_dir =
            canonicalize_within(state.storage.base_dir(), FsPath::new("sessions"))
                .map_err(map_path_error)?;
        return canonicalize_within(&safe_sessions_dir, FsPath::new(session_id))
            .map_err(map_path_error);
    }

    if project_path.is_none() {
        Err(ApiError::not_found(format!(
            "Session {session_id} not found"
        )))
    } else {
        Err(ApiError::not_found(format!(
            "No files found for session {session_id}"
        )))
    }
}

fn collect_session_files(
    root: &FsPath,
    directory: &FsPath,
    files: &mut Vec<SessionFileEntry>,
) -> Result<(), StorageError> {
    collect_session_files_with_metadata(root, directory, files, &read_session_file_metadata)
}

fn read_session_file_metadata(path: &FsPath) -> std::io::Result<fs::Metadata> {
    fs::metadata(path)
}

fn collect_session_files_with_metadata<M>(
    root: &FsPath,
    directory: &FsPath,
    files: &mut Vec<SessionFileEntry>,
    read_metadata: &M,
) -> Result<(), StorageError>
where
    M: Fn(&FsPath) -> std::io::Result<fs::Metadata>,
{
    let mut entries = Vec::new();
    for entry in fs::read_dir(directory)? {
        match entry {
            Ok(entry) => entries.push(entry),
            Err(error) => tracing::debug!(
                error = %error,
                "Skipping session file entry that could not be enumerated"
            ),
        }
    }
    entries.sort_by_key(|entry| entry.file_name());

    for entry in entries {
        let lexical_path = entry.path();
        let relative_path = match lexical_path.strip_prefix(root) {
            Ok(path) => path,
            Err(error) => {
                tracing::debug!(
                    error = %error,
                    "Skipping session file entry outside the listing root"
                );
                continue;
            }
        };
        let canonical_path = match canonicalize_within(root, relative_path) {
            Ok(path) => path,
            Err(error) => {
                tracing::debug!(
                    error = %error,
                    "Skipping session file entry that could not be safely resolved"
                );
                continue;
            }
        };
        let entry_type = match entry.file_type() {
            Ok(entry_type) => entry_type,
            Err(error) => {
                tracing::debug!(
                    error = %error,
                    "Skipping session file entry whose type could not be read"
                );
                continue;
            }
        };
        let metadata = match read_metadata(&canonical_path) {
            Ok(metadata) => metadata,
            Err(error) => {
                tracing::debug!(
                    error = %error,
                    "Skipping session file entry whose metadata could not be read"
                );
                continue;
            }
        };
        let is_dir = metadata.is_dir();

        if is_dir && !entry_type.is_symlink() {
            if let Err(error) =
                collect_session_files_with_metadata(root, &lexical_path, files, read_metadata)
            {
                tracing::debug!(
                    error = %error,
                    "Skipping session file subtree that could not be enumerated"
                );
                continue;
            }
        }

        files.push(SessionFileEntry {
            path: normalize_relative_path(relative_path),
            name: entry.file_name().to_string_lossy().into_owned(),
            is_dir,
            size: if is_dir { 0 } else { metadata.len() },
            modified: metadata
                .modified()
                .ok()
                .map(|timestamp| DateTime::<Utc>::from(timestamp).to_rfc3339()),
        });
    }

    Ok(())
}

fn normalize_relative_path(path: &FsPath) -> String {
    path.to_string_lossy().replace('\\', "/")
}

fn map_path_error(error: StorageError) -> ApiError {
    match error {
        StorageError::InvalidPath(message) => ApiError::bad_request(message),
        StorageError::Io(error) => map_io_error(error, "session file"),
        other => ApiError::internal(other.to_string()),
    }
}

fn map_io_error(error: std::io::Error, path: &str) -> ApiError {
    if error.kind() == std::io::ErrorKind::NotFound {
        ApiError::not_found(format!("Session file not found: {path}"))
    } else {
        ApiError::internal(format!("Failed to access session file {path}: {error}"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn listing_skips_one_metadata_failure_without_losing_valid_siblings() {
        let root = tempfile::tempdir().unwrap();
        let good_path = root.path().join("good.txt");
        let blocked_path = root.path().join("blocked.txt");
        fs::write(&good_path, "safe\n").unwrap();
        fs::write(&blocked_path, "blocked\n").unwrap();
        let blocked_path = fs::canonicalize(blocked_path).unwrap();

        let mut files = Vec::new();
        collect_session_files_with_metadata(
            root.path(),
            root.path(),
            &mut files,
            &|path| {
                if path == blocked_path {
                    Err(std::io::Error::new(
                        std::io::ErrorKind::PermissionDenied,
                        "injected metadata failure",
                    ))
                } else {
                    fs::metadata(path)
                }
            },
        )
        .unwrap();

        let paths = files
            .iter()
            .map(|entry| entry.path.as_str())
            .collect::<Vec<_>>();
        assert_eq!(paths, vec!["good.txt"]);
    }
}
