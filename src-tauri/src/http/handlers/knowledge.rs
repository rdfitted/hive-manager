use axum::{
    extract::{Query, State},
    Json,
};
use regex::Regex;
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use std::ffi::OsString;
use std::fs::{self, File};
use std::io::Read;
use std::path::{Path, PathBuf};
use std::sync::{Arc, OnceLock};
use tokio::sync::Semaphore;

use super::validate_session_id;
use crate::http::error::ApiError;
use crate::http::state::AppState;

const WIKI_FOLDERS: [KnowledgeFolder; 3] = [
    KnowledgeFolder::Patterns,
    KnowledgeFolder::Practices,
    KnowledgeFolder::Research,
];
const PROJECT_FILES: [&str; 2] = ["project-dna.md", "bug-patterns.md"];
const DEFAULT_WIKI_ROOT: &str = "~/.ai-docs/wiki";

pub(crate) const MAX_NODES: usize = 400;
pub(crate) const MAX_EDGES: usize = 1_500;
pub(crate) const MAX_FILE_BYTES: usize = 256 * 1024;
const MAX_PREVIEW_BYTES: usize = 20 * 1024;
const MAX_TITLE_BYTES: usize = 512;
const MAX_LAST_UPDATED_BYTES: usize = 128;
const MAX_EDGE_HINT_BYTES: usize = 512;
const MAX_DISCOVERED_FILES: usize = MAX_NODES * 3;
const MAX_RAW_EDGES: usize = MAX_EDGES * 8;
const MAX_SCAN_ENTRIES: usize = 10_000;
const MAX_DIRECTORY_DEPTH: usize = 32;
const MAX_GRAPH_RESPONSE_BYTES: usize = 2 * 1024 * 1024;
const GRAPH_RESPONSE_OVERHEAD_RESERVE: usize = 64 * 1024;
const MAX_GRAPH_ITEM_BYTES: usize = MAX_GRAPH_RESPONSE_BYTES - GRAPH_RESPONSE_OVERHEAD_RESERVE;
const MAX_CONCURRENT_SCANS: usize = 2;

static KNOWLEDGE_SCAN_LIMITER: OnceLock<Semaphore> = OnceLock::new();

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum KnowledgeFolder {
    Patterns,
    Practices,
    Research,
    Project,
}

impl KnowledgeFolder {
    fn as_str(self) -> &'static str {
        match self {
            Self::Patterns => "patterns",
            Self::Practices => "practices",
            Self::Research => "research",
            Self::Project => "project",
        }
    }

    fn from_id_prefix(prefix: &str) -> Option<Self> {
        WIKI_FOLDERS
            .into_iter()
            .chain(std::iter::once(Self::Project))
            .find(|folder| folder.as_str() == prefix)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum KnowledgeEdgeKind {
    CrossRef,
    Wikilink,
    Global,
    Related,
    From,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub(crate) struct KnowledgeNode {
    pub id: String,
    pub title: String,
    pub folder: KnowledgeFolder,
    /// Folder-relative path only. Absolute filesystem paths never cross the API boundary.
    pub path: String,
    pub last_updated: String,
    pub in_degree: usize,
    pub out_degree: usize,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub(crate) struct KnowledgeEdge {
    pub source: String,
    pub target: String,
    pub kind: KnowledgeEdgeKind,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub(crate) struct KnowledgeGraphResponse {
    pub nodes: Vec<KnowledgeNode>,
    pub edges: Vec<KnowledgeEdge>,
    pub truncated: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub(crate) struct KnowledgePageResponse {
    pub id: String,
    pub title: String,
    pub folder: KnowledgeFolder,
    pub path: String,
    pub content: String,
    pub last_updated: String,
    pub truncated: bool,
}

#[derive(Debug, Default, Deserialize)]
pub struct KnowledgeGraphQuery {
    #[serde(default, alias = "maxNodes")]
    pub max_nodes: Option<usize>,
    #[serde(default, alias = "sessionId")]
    pub session_id: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct KnowledgePageQuery {
    pub id: String,
    #[serde(default, alias = "sessionId")]
    pub session_id: Option<String>,
}

#[derive(Debug, Clone)]
struct SourceFile {
    folder: KnowledgeFolder,
    path: PathBuf,
    /// Canonical root for this folder; used both to form relative IDs and to re-check containment.
    root: PathBuf,
}

#[derive(Debug, Default)]
struct SourceEnumeration {
    sources: Vec<SourceFile>,
    truncated: bool,
}

#[derive(Debug)]
struct ParsedSource {
    node: KnowledgeNode,
    raw_edges: Vec<RawEdge>,
    body: String,
    truncated: bool,
}

#[derive(Debug)]
struct RawEdge {
    source: String,
    target_hint: String,
    kind: KnowledgeEdgeKind,
}

enum SourceRead {
    Parsed(ParsedSource),
    TooLarge,
    Unavailable,
}

#[derive(Default)]
struct NodeLookup {
    by_id: HashMap<String, Vec<String>>,
    by_basename: HashMap<String, Vec<String>>,
    by_title: HashMap<String, Vec<String>>,
}

/// GET /api/knowledge/graph?session_id=<optional-live-session-id>
///
/// Read-only and fail-soft: filesystem failures produce a partial or empty graph, never a 500.
/// Project-local nodes are included only when the caller supplies a live session id.
pub async fn get_knowledge_graph(
    State(state): State<Arc<AppState>>,
    Query(query): Query<KnowledgeGraphQuery>,
) -> Result<Json<KnowledgeGraphResponse>, ApiError> {
    if query.max_nodes == Some(0) {
        return Err(ApiError::bad_request("max_nodes must be greater than zero"));
    }
    let max_nodes = query.max_nodes.unwrap_or(MAX_NODES).min(MAX_NODES);
    let wiki_root = resolve_wiki_root(&state).await;
    let project_root = resolve_project_root(&state, query.session_id.as_deref())?;
    let Ok(_permit) = knowledge_scan_limiter().acquire().await else {
        return Ok(Json(KnowledgeGraphResponse::default()));
    };

    let graph = tokio::task::spawn_blocking(move || {
        scan_knowledge(&wiki_root, project_root.as_deref(), max_nodes)
    })
    .await
    .unwrap_or_default();

    Ok(Json(graph))
}

/// GET /api/knowledge/page?id=patterns/example&session_id=<optional-live-session-id>
///
/// The request supplies an allow-listed node ID, never a filesystem path. Malformed IDs are
/// rejected before any filesystem work; a safe but unknown ID returns 404 without echoing a path.
pub async fn get_knowledge_page(
    State(state): State<Arc<AppState>>,
    Query(query): Query<KnowledgePageQuery>,
) -> Result<Json<KnowledgePageResponse>, ApiError> {
    validate_knowledge_id(&query.id)?;
    let wiki_root = resolve_wiki_root(&state).await;
    let project_root = resolve_project_root(&state, query.session_id.as_deref())?;
    let id = query.id;
    let Ok(_permit) = knowledge_scan_limiter().acquire().await else {
        return Err(ApiError::not_found("Knowledge page not found"));
    };

    let page = tokio::task::spawn_blocking(move || {
        preview_knowledge_page(&wiki_root, project_root.as_deref(), &id)
    })
    .await
    .ok()
    .flatten();

    page.map(Json)
        .ok_or_else(|| ApiError::not_found("Knowledge page not found"))
}

async fn resolve_wiki_root(state: &AppState) -> PathBuf {
    let configured = state.config.read().await.global_wiki_path.clone();
    let env_root = std::env::var_os("HIVE_WIKI_ROOT").filter(|value| !value.is_empty());
    resolve_wiki_root_from(env_root, configured.as_deref(), user_home().as_deref())
}

fn resolve_wiki_root_from(
    env_root: Option<OsString>,
    configured: Option<&str>,
    home: Option<&Path>,
) -> PathBuf {
    let selected = env_root
        .map(PathBuf::from)
        .or_else(|| {
            configured
                .map(str::trim)
                .filter(|value| !value.is_empty())
                .map(PathBuf::from)
        })
        .unwrap_or_else(|| PathBuf::from(DEFAULT_WIKI_ROOT));
    expand_tilde_path(&selected, home)
}

fn user_home() -> Option<PathBuf> {
    let preferred = if cfg!(windows) { "USERPROFILE" } else { "HOME" };
    let fallback = if cfg!(windows) { "HOME" } else { "USERPROFILE" };
    std::env::var_os(preferred)
        .filter(|value| !value.is_empty())
        .or_else(|| std::env::var_os(fallback).filter(|value| !value.is_empty()))
        .map(PathBuf::from)
}

fn expand_tilde_path(path: &Path, home: Option<&Path>) -> PathBuf {
    let Some(raw) = path.to_str() else {
        return path.to_path_buf();
    };
    let Some(rest) = raw.strip_prefix('~') else {
        return path.to_path_buf();
    };
    if !rest.is_empty() && !rest.starts_with('/') && !rest.starts_with('\\') {
        return path.to_path_buf();
    }
    let Some(home) = home else {
        return path.to_path_buf();
    };
    let rest = rest.trim_start_matches(['/', '\\']);
    if rest.is_empty() {
        home.to_path_buf()
    } else {
        home.join(rest)
    }
}

fn knowledge_scan_limiter() -> &'static Semaphore {
    KNOWLEDGE_SCAN_LIMITER.get_or_init(|| Semaphore::new(MAX_CONCURRENT_SCANS))
}

/// Project-local knowledge is opt-in and bound to one exact live session. Requests without a
/// session id deliberately expose only the global allow-listed wiki, so adding or switching other
/// sessions cannot change which `project/*` page a previously loaded graph id resolves to.
fn resolve_project_root(
    state: &AppState,
    session_id: Option<&str>,
) -> Result<Option<PathBuf>, ApiError> {
    let sessions = if session_id.is_some() {
        state.session_controller.read().list_sessions()
    } else {
        Vec::new()
    };
    resolve_project_root_from_sessions(
        session_id,
        sessions
            .iter()
            .map(|session| (session.id.as_str(), session.project_path.as_path())),
    )
}

fn resolve_project_root_from_sessions<'a>(
    session_id: Option<&str>,
    sessions: impl IntoIterator<Item = (&'a str, &'a Path)>,
) -> Result<Option<PathBuf>, ApiError> {
    let Some(session_id) = session_id else {
        return Ok(None);
    };
    if session_id.is_empty() || session_id.len() > 128 {
        return Err(ApiError::bad_request("Invalid session ID"));
    }
    validate_session_id(session_id)?;

    sessions
        .into_iter()
        .find_map(|(candidate_id, project_root)| {
            (candidate_id == session_id).then(|| project_root.to_path_buf())
        })
        .map(Some)
        .ok_or_else(|| ApiError::not_found("Session not found"))
}

pub(crate) fn scan_knowledge(
    wiki_root: &Path,
    project_root: Option<&Path>,
    max_nodes: usize,
) -> KnowledgeGraphResponse {
    let node_cap = max_nodes.clamp(1, MAX_NODES);
    let enumeration = enumerate_sources(wiki_root, project_root);
    let mut truncated = enumeration.truncated;
    let mut nodes = Vec::new();
    let mut raw_edges = Vec::new();
    let mut seen_ids = HashSet::new();
    let mut graph_item_bytes = 0usize;

    for source in enumeration.sources {
        if nodes.len() >= node_cap {
            truncated = true;
            break;
        }
        match read_source(&source) {
            SourceRead::Parsed(parsed) => {
                if seen_ids.contains(&parsed.node.id) {
                    continue;
                }
                let node_bytes = serialized_item_bytes(&parsed.node);
                if graph_item_bytes.saturating_add(node_bytes) > MAX_GRAPH_ITEM_BYTES {
                    truncated = true;
                    break;
                }
                graph_item_bytes += node_bytes;
                seen_ids.insert(parsed.node.id.clone());
                truncated |= parsed.truncated;
                nodes.push(parsed.node);
                for edge in parsed.raw_edges {
                    if raw_edges.len() >= MAX_RAW_EDGES {
                        truncated = true;
                        break;
                    }
                    raw_edges.push(edge);
                }
            }
            SourceRead::TooLarge => truncated = true,
            SourceRead::Unavailable => {}
        }
    }

    let lookup = NodeLookup::from_nodes(&nodes);
    let edge_byte_budget = MAX_GRAPH_ITEM_BYTES.saturating_sub(graph_item_bytes);
    let (edges, edges_truncated) = build_edges(raw_edges, &lookup, edge_byte_budget);
    truncated |= edges_truncated;

    let node_positions: HashMap<String, usize> = nodes
        .iter()
        .enumerate()
        .map(|(index, node)| (node.id.clone(), index))
        .collect();
    for edge in &edges {
        if let Some(index) = node_positions.get(&edge.source) {
            nodes[*index].out_degree += 1;
        }
        if let Some(index) = node_positions.get(&edge.target) {
            nodes[*index].in_degree += 1;
        }
    }

    let response = KnowledgeGraphResponse {
        nodes,
        edges,
        truncated,
    };
    debug_assert!(
        serde_json::to_vec(&response)
            .map(|serialized| serialized.len() <= MAX_GRAPH_RESPONSE_BYTES)
            .unwrap_or(false),
        "knowledge graph response exceeded its serialized size cap"
    );
    response
}

pub(crate) fn preview_knowledge_page(
    wiki_root: &Path,
    project_root: Option<&Path>,
    id: &str,
) -> Option<KnowledgePageResponse> {
    if validate_knowledge_id(id).is_err() {
        return None;
    }

    let source = enumerate_sources(wiki_root, project_root)
        .sources
        .into_iter()
        .find(|source| node_id_for(source).as_deref() == Some(id))?;
    let SourceRead::Parsed(parsed) = read_source(&source) else {
        return None;
    };
    let metadata_truncated = parsed.truncated;
    let (content, content_truncated) = truncate_utf8(parsed.body, MAX_PREVIEW_BYTES);

    Some(KnowledgePageResponse {
        id: parsed.node.id,
        title: parsed.node.title,
        folder: parsed.node.folder,
        path: parsed.node.path,
        content,
        last_updated: parsed.node.last_updated,
        truncated: metadata_truncated || content_truncated,
    })
}

fn serialized_item_bytes<T: Serialize>(value: &T) -> usize {
    // Include an array separator. The fixed response wrapper and degree growth are covered by
    // GRAPH_RESPONSE_OVERHEAD_RESERVE.
    serde_json::to_vec(value)
        .map(|serialized| serialized.len().saturating_add(1))
        .unwrap_or(MAX_GRAPH_ITEM_BYTES.saturating_add(1))
}

fn validate_knowledge_id(id: &str) -> Result<(), ApiError> {
    if id.is_empty() || id.len() > 512 || id.contains('\0') {
        return Err(ApiError::bad_request("Invalid knowledge page ID"));
    }
    if id.starts_with('/')
        || id.starts_with('\\')
        || id.contains('\\')
        || id.get(1..2) == Some(":")
    {
        return Err(ApiError::bad_request("Invalid knowledge page ID"));
    }

    let components: Vec<&str> = id.split('/').collect();
    let Some(folder) = components
        .first()
        .and_then(|prefix| KnowledgeFolder::from_id_prefix(prefix))
    else {
        return Err(ApiError::bad_request("Invalid knowledge page ID"));
    };
    if components.len() < 2
        || components
            .iter()
            .any(|component| component.is_empty() || *component == "." || *component == "..")
    {
        return Err(ApiError::bad_request("Invalid knowledge page ID"));
    }
    if folder == KnowledgeFolder::Project
        && !matches!(id, "project/project-dna" | "project/bug-patterns")
    {
        return Err(ApiError::bad_request("Invalid knowledge page ID"));
    }
    Ok(())
}

fn enumerate_sources(wiki_root: &Path, project_root: Option<&Path>) -> SourceEnumeration {
    let mut enumeration = SourceEnumeration::default();
    let mut entries_seen = 0;
    let canonical_wiki_root = fs::canonicalize(wiki_root).ok();

    for folder in WIKI_FOLDERS {
        let subtree = wiki_root.join(folder.as_str());
        let Ok(metadata) = fs::symlink_metadata(&subtree) else {
            continue;
        };
        if !metadata.is_dir() || metadata.file_type().is_symlink() {
            continue;
        }
        let Ok(canonical_root) = fs::canonicalize(&subtree) else {
            continue;
        };
        if canonical_wiki_root
            .as_ref()
            .is_some_and(|root| !canonical_root.starts_with(root))
        {
            continue;
        }

        let remaining = MAX_DISCOVERED_FILES.saturating_sub(enumeration.sources.len());
        if remaining == 0 {
            enumeration.truncated = true;
            break;
        }
        let mut found = Vec::new();
        collect_markdown_files(
            &canonical_root,
            &canonical_root,
            0,
            remaining,
            &mut entries_seen,
            &mut found,
            &mut enumeration.truncated,
        );
        enumeration
            .sources
            .extend(found.into_iter().map(|path| SourceFile {
                folder,
                path,
                root: canonical_root.clone(),
            }));
    }

    if let Some(project_root) = project_root {
        let ai_docs = project_root.join(".ai-docs");
        let Ok(ai_docs_metadata) = fs::symlink_metadata(&ai_docs) else {
            return enumeration;
        };
        if !ai_docs_metadata.is_dir() || ai_docs_metadata.file_type().is_symlink() {
            return enumeration;
        }
        let canonical_ai_docs = fs::canonicalize(&ai_docs).ok();
        for name in PROJECT_FILES {
            let path = ai_docs.join(name);
            let Ok(metadata) = fs::symlink_metadata(&path) else {
                continue;
            };
            if !metadata.is_file() || metadata.file_type().is_symlink() {
                continue;
            }
            let (Some(root), Ok(canonical_path)) =
                (canonical_ai_docs.as_ref(), fs::canonicalize(&path))
            else {
                continue;
            };
            if !canonical_path.starts_with(root) {
                continue;
            }
            enumeration.sources.push(SourceFile {
                folder: KnowledgeFolder::Project,
                path: canonical_path,
                root: root.clone(),
            });
        }
    }

    enumeration
}

#[allow(clippy::too_many_arguments)]
fn collect_markdown_files(
    canonical_root: &Path,
    directory: &Path,
    depth: usize,
    file_limit: usize,
    entries_seen: &mut usize,
    output: &mut Vec<PathBuf>,
    truncated: &mut bool,
) {
    if depth > MAX_DIRECTORY_DEPTH {
        *truncated = true;
        return;
    }
    if *entries_seen >= MAX_SCAN_ENTRIES || output.len() >= file_limit {
        *truncated = true;
        return;
    }
    let Ok(read_dir) = fs::read_dir(directory) else {
        return;
    };
    // Bound collection itself before sorting. `ReadDir` is lazy, but collecting it wholesale would
    // otherwise let one enormous directory bypass MAX_SCAN_ENTRIES before the loop below runs.
    let remaining_entries = MAX_SCAN_ENTRIES.saturating_sub(*entries_seen);
    let mut enumerated: Vec<_> = read_dir
        .take(remaining_entries.saturating_add(1))
        .collect();
    if enumerated.len() > remaining_entries {
        enumerated.truncate(remaining_entries);
        *truncated = true;
    }
    // Count raw iterator results, including errors, so repeated ReadDir failures cannot evade the
    // global traversal-work ceiling.
    *entries_seen += enumerated.len();
    let mut entries: Vec<_> = enumerated.into_iter().filter_map(Result::ok).collect();
    entries.sort_by_key(|entry| entry.file_name());

    for entry in entries {
        if output.len() >= file_limit {
            *truncated = true;
            return;
        }

        let name = entry.file_name();
        if name.to_string_lossy().starts_with('.') || is_forbidden_name(&name.to_string_lossy()) {
            continue;
        }
        let Ok(file_type) = entry.file_type() else {
            continue;
        };
        if file_type.is_symlink() {
            continue;
        }
        let Ok(canonical_path) = fs::canonicalize(entry.path()) else {
            continue;
        };
        if !canonical_path.starts_with(canonical_root) {
            continue;
        }

        if file_type.is_dir() {
            collect_markdown_files(
                canonical_root,
                &canonical_path,
                depth + 1,
                file_limit,
                entries_seen,
                output,
                truncated,
            );
            if output.len() >= file_limit {
                *truncated = true;
                return;
            }
        } else if file_type.is_file() && has_markdown_extension(&canonical_path) {
            output.push(canonical_path);
        }
    }
}

fn is_forbidden_name(name: &str) -> bool {
    let lower = name.to_ascii_lowercase();
    matches!(
        lower.as_str(),
        "agent-os.db"
            | "agent-os.db-shm"
            | "agent-os.db-wal"
            | ".env"
            | "learnings.jsonl"
            | "log.md"
            | "index.md"
    ) || (lower.starts_with("log-archive") && lower.ends_with(".md"))
}

fn has_markdown_extension(path: &Path) -> bool {
    path.extension()
        .and_then(|extension| extension.to_str())
        .is_some_and(|extension| extension.eq_ignore_ascii_case("md"))
}

fn read_source(source: &SourceFile) -> SourceRead {
    let Ok(metadata) = fs::symlink_metadata(&source.path) else {
        return SourceRead::Unavailable;
    };
    if !metadata.is_file() || metadata.file_type().is_symlink() {
        return SourceRead::Unavailable;
    }
    let Ok(canonical_path) = fs::canonicalize(&source.path) else {
        return SourceRead::Unavailable;
    };
    if !canonical_path.starts_with(&source.root) {
        return SourceRead::Unavailable;
    }

    let Ok(mut file) = File::open(&canonical_path) else {
        return SourceRead::Unavailable;
    };
    let Ok(open_metadata) = file.metadata() else {
        return SourceRead::Unavailable;
    };
    if !open_metadata.is_file() {
        return SourceRead::Unavailable;
    }
    if open_metadata.len() > MAX_FILE_BYTES as u64 {
        return SourceRead::TooLarge;
    }

    let mut bytes = Vec::with_capacity(open_metadata.len() as usize);
    if file
        .by_ref()
        .take((MAX_FILE_BYTES + 1) as u64)
        .read_to_end(&mut bytes)
        .is_err()
    {
        return SourceRead::Unavailable;
    }
    if bytes.len() > MAX_FILE_BYTES {
        return SourceRead::TooLarge;
    }
    let Ok(text) = String::from_utf8(bytes) else {
        return SourceRead::Unavailable;
    };
    let Some(id) = node_id_for(source) else {
        return SourceRead::Unavailable;
    };
    if id.len() > 512 {
        return SourceRead::Unavailable;
    }
    let (frontmatter, body) = split_frontmatter(&text);
    let file_stem = canonical_path
        .file_stem()
        .and_then(|stem| stem.to_str())
        .unwrap_or("untitled");
    let title = frontmatter_field(frontmatter, "title")
        .or_else(|| first_h1(body))
        .unwrap_or_else(|| file_stem.to_string());
    let (title, title_truncated) = truncate_utf8(title, MAX_TITLE_BYTES);
    let last_updated = frontmatter_field(frontmatter, "last_updated").unwrap_or_default();
    let (last_updated, last_updated_truncated) =
        truncate_utf8(last_updated, MAX_LAST_UPDATED_BYTES);
    let node = KnowledgeNode {
        path: format!("{id}.md"),
        id: id.clone(),
        title,
        folder: source.folder,
        last_updated,
        in_degree: 0,
        out_degree: 0,
    };

    let mut raw_edges = Vec::new();
    for key in ["cross_refs", "cross_ref"] {
        for target_hint in frontmatter_list(frontmatter, key) {
            raw_edges.push(RawEdge {
                source: id.clone(),
                target_hint,
                kind: KnowledgeEdgeKind::CrossRef,
            });
        }
    }
    for target_hint in frontmatter_list(frontmatter, "related") {
        raw_edges.push(RawEdge {
            source: id.clone(),
            target_hint,
            kind: KnowledgeEdgeKind::Related,
        });
    }
    raw_edges.extend(parse_body_edges(&id, body));
    let original_edge_count = raw_edges.len();
    raw_edges.retain(|edge| edge.target_hint.len() <= MAX_EDGE_HINT_BYTES);
    let edge_hints_truncated = original_edge_count != raw_edges.len();

    SourceRead::Parsed(ParsedSource {
        node,
        raw_edges,
        body: body.to_string(),
        truncated: title_truncated || last_updated_truncated || edge_hints_truncated,
    })
}

fn node_id_for(source: &SourceFile) -> Option<String> {
    let relative = source.path.strip_prefix(&source.root).ok()?;
    let mut components = Vec::new();
    for component in relative.iter() {
        components.push(component.to_str()?);
    }
    let relative = components.join("/");
    let stem = strip_markdown_extension(&relative);
    if stem.is_empty() {
        None
    } else {
        Some(format!("{}/{}", source.folder.as_str(), stem))
    }
}

fn strip_markdown_extension(value: &str) -> &str {
    let Some(suffix_start) = value.len().checked_sub(3) else {
        return value;
    };
    match (value.get(..suffix_start), value.get(suffix_start..)) {
        (Some(stem), Some(extension)) if extension.eq_ignore_ascii_case(".md") => stem,
        _ => value,
    }
}

fn split_frontmatter(text: &str) -> (&str, &str) {
    let Some(after_opening) = text
        .strip_prefix("---\r\n")
        .or_else(|| text.strip_prefix("---\n"))
    else {
        return ("", text);
    };

    let mut consumed = 0;
    for line in after_opening.split_inclusive('\n') {
        let line_without_ending = line.trim_end_matches(['\r', '\n']);
        if line_without_ending == "---" {
            return (&after_opening[..consumed], &after_opening[consumed + line.len()..]);
        }
        consumed += line.len();
    }
    if after_opening[consumed..].trim_end_matches('\r') == "---" {
        return (&after_opening[..consumed], "");
    }
    ("", text)
}

fn frontmatter_field(frontmatter: &str, key: &str) -> Option<String> {
    frontmatter.lines().find_map(|line| {
        if line.starts_with(char::is_whitespace) {
            return None;
        }
        let (candidate, value) = line.split_once(':')?;
        if !candidate.trim().eq_ignore_ascii_case(key) {
            return None;
        }
        let value = trim_matching_quotes(value.trim());
        (!value.is_empty()).then(|| value.to_string())
    })
}

/// Parse a YAML-shaped field line-by-line. This intentionally is not a YAML parser: corpus values
/// such as issue references containing `#NNN` must not be truncated as YAML comments.
fn frontmatter_list(frontmatter: &str, key: &str) -> Vec<String> {
    let lines: Vec<&str> = frontmatter.lines().collect();
    let mut output = Vec::new();
    let mut index = 0;
    while index < lines.len() {
        let line = lines[index];
        if line.starts_with(char::is_whitespace) {
            index += 1;
            continue;
        }
        let Some((candidate, raw_value)) = line.split_once(':') else {
            index += 1;
            continue;
        };
        if !candidate.trim().eq_ignore_ascii_case(key) {
            index += 1;
            continue;
        }

        let raw_value = raw_value.trim();
        if raw_value.starts_with('[') && raw_value.ends_with(']') {
            for part in raw_value[1..raw_value.len() - 1].split(',') {
                let value = trim_matching_quotes(part.trim());
                if !value.is_empty() {
                    output.push(value.to_string());
                }
            }
        } else if !raw_value.is_empty() {
            let value = trim_matching_quotes(raw_value);
            if !value.is_empty() {
                output.push(value.to_string());
            }
        } else {
            index += 1;
            while index < lines.len() {
                let nested = lines[index];
                if let Some(value) = nested.trim_start().strip_prefix("- ") {
                    let value = trim_matching_quotes(value.trim());
                    if !value.is_empty() {
                        output.push(value.to_string());
                    }
                    index += 1;
                    continue;
                }
                if nested.trim().is_empty() {
                    index += 1;
                    continue;
                }
                break;
            }
            continue;
        }
        index += 1;
    }
    output
}

fn trim_matching_quotes(value: &str) -> &str {
    if value.len() >= 2
        && ((value.starts_with('"') && value.ends_with('"'))
            || (value.starts_with('\'') && value.ends_with('\'')))
    {
        &value[1..value.len() - 1]
    } else {
        value
    }
}

fn first_h1(body: &str) -> Option<String> {
    body.lines().find_map(|line| {
        line.strip_prefix("# ")
            .map(str::trim)
            .filter(|title| !title.is_empty())
            .map(str::to_string)
    })
}

fn parse_body_edges(source: &str, body: &str) -> Vec<RawEdge> {
    static WIKILINK: OnceLock<Regex> = OnceLock::new();
    static GLOBAL: OnceLock<Regex> = OnceLock::new();
    static RELATED: OnceLock<Regex> = OnceLock::new();
    static FROM: OnceLock<Regex> = OnceLock::new();

    let wikilink = WIKILINK.get_or_init(|| Regex::new(r"\[\[([^\]]+?)\]\]").unwrap());
    let global =
        GLOBAL.get_or_init(|| Regex::new(r"(?i)->\s*global:\s*(.+?)\s*$").unwrap());
    let related = RELATED
        .get_or_init(|| Regex::new(r"(?i)(?:<->|->)\s*related:\s*(.+?)\s*$").unwrap());
    let from = FROM.get_or_init(|| Regex::new(r"(?i)<-\s*from:\s*(.+?)\s*$").unwrap());

    let mut edges = Vec::new();
    for captures in wikilink.captures_iter(body) {
        let target = captures[1].split('|').next().unwrap_or("").trim();
        if !target.is_empty() {
            edges.push(RawEdge {
                source: source.to_string(),
                target_hint: target.to_string(),
                kind: KnowledgeEdgeKind::Wikilink,
            });
        }
    }
    for line in body.lines() {
        for (regex, kind) in [
            (global, KnowledgeEdgeKind::Global),
            (related, KnowledgeEdgeKind::Related),
            (from, KnowledgeEdgeKind::From),
        ] {
            let Some(captures) = regex.captures(line) else {
                continue;
            };
            let target = captures[1].trim();
            if !target.is_empty() {
                edges.push(RawEdge {
                    source: source.to_string(),
                    target_hint: target.to_string(),
                    kind,
                });
            }
            break;
        }
    }
    edges
}

impl NodeLookup {
    fn from_nodes(nodes: &[KnowledgeNode]) -> Self {
        let mut lookup = Self::default();
        for node in nodes {
            insert_lookup(&mut lookup.by_id, normalize_lookup(&node.id), &node.id);
            let basename = node.id.rsplit('/').next().unwrap_or(&node.id);
            insert_lookup(
                &mut lookup.by_basename,
                normalize_lookup(basename),
                &node.id,
            );
            insert_lookup(
                &mut lookup.by_title,
                normalize_lookup(&node.title),
                &node.id,
            );
        }
        lookup
    }
}

fn insert_lookup(map: &mut HashMap<String, Vec<String>>, key: String, id: &str) {
    map.entry(key).or_default().push(id.to_string());
}

fn normalize_lookup(value: &str) -> String {
    value.trim().to_lowercase()
}

fn resolve_hint(hint: &str, lookup: &NodeLookup) -> Option<String> {
    let mut value = hint.trim();
    if let Some(inner) = embedded_wikilink(value) {
        value = inner;
    }
    value = value.split('|').next().unwrap_or("").trim();
    let normalized_slashes = value.replace('\\', "/");
    let unannotated = strip_path_annotation(normalized_slashes.trim().trim_matches(['<', '>']));
    let mut slug = unannotated.to_string();
    while let Some(rest) = slug.strip_prefix("./") {
        slug = rest.to_string();
    }
    slug = normalize_parent_relative_hint(slug)?;
    slug = strip_markdown_extension(&slug).to_string();
    if slug.is_empty()
        || slug.starts_with('/')
        || slug.get(1..2) == Some(":")
        || slug
            .split('/')
            .any(|component| component.is_empty() || component == "." || component == "..")
    {
        return None;
    }

    let explicit_path = slug.contains('/');
    if explicit_path {
        let prefix = slug.split('/').next()?;
        if KnowledgeFolder::from_id_prefix(prefix).is_none() {
            return None;
        }
    }

    if let Some(id) = unique_lookup(&lookup.by_id, &normalize_lookup(&slug)) {
        return Some(id);
    }
    // An explicit path that did not match an allow-listed ID must not fall back by basename: that
    // would turn `clients/foo.md` into an edge to an unrelated allow-listed `patterns/foo.md`.
    if explicit_path {
        return None;
    }

    let basename = slug.rsplit('/').next().unwrap_or(&slug);
    unique_lookup(&lookup.by_basename, &normalize_lookup(basename))
        .or_else(|| unique_lookup(&lookup.by_title, &normalize_lookup(&slug)))
}

/// Corpus research pages sometimes refer to a sibling allow-listed subtree with
/// `../patterns/foo.md`. These strings are lookup hints only (never filesystem paths), but parent
/// components are still normalized narrowly: after removing leading parents, the hint must name
/// one of the three global wiki allow-list roots. `../clients/foo.md` therefore remains invalid.
fn normalize_parent_relative_hint(mut slug: String) -> Option<String> {
    let had_parent_prefix = slug.starts_with("../");
    while let Some(rest) = slug.strip_prefix("../") {
        slug = rest.to_string();
    }

    if let Some(rest) = slug.strip_prefix("wiki/") {
        slug = rest.to_string();
    }
    if !had_parent_prefix {
        return Some(slug);
    }

    let (prefix, remainder) = slug.split_once('/')?;
    if remainder.is_empty()
        || !WIKI_FOLDERS
            .into_iter()
            .any(|folder| folder.as_str() == prefix)
    {
        return None;
    }
    Some(slug)
}

fn embedded_wikilink(value: &str) -> Option<&str> {
    let opening = value.find("[[")? + 2;
    let remainder = value.get(opening..)?;
    let closing = remainder.find("]]")?;
    remainder.get(..closing)
}

fn strip_path_annotation(value: &str) -> &str {
    let lower = value.to_ascii_lowercase();
    let Some(extension_start) = lower.rfind(".md") else {
        return value;
    };
    let extension_end = extension_start + 3;
    let Some(remainder) = value.get(extension_end..) else {
        return value;
    };
    let annotation_follows = remainder.is_empty()
        || remainder.starts_with('#')
        || remainder.starts_with('(')
        || remainder
            .chars()
            .next()
            .is_some_and(char::is_whitespace);
    if annotation_follows {
        value.get(..extension_end).unwrap_or(value)
    } else {
        value
    }
}

fn unique_lookup(map: &HashMap<String, Vec<String>>, key: &str) -> Option<String> {
    let matches = map.get(key)?;
    (matches.len() == 1).then(|| matches[0].clone())
}

fn build_edges(
    raw_edges: Vec<RawEdge>,
    lookup: &NodeLookup,
    max_item_bytes: usize,
) -> (Vec<KnowledgeEdge>, bool) {
    let mut edges = Vec::new();
    let mut seen = HashSet::new();
    let mut truncated = false;
    let mut item_bytes = 0usize;

    for raw in raw_edges {
        if edges.len() >= MAX_EDGES {
            truncated = true;
            break;
        }
        let Some(target) = resolve_hint(&raw.target_hint, lookup) else {
            continue;
        };
        if target == raw.source {
            continue;
        }
        let key = (raw.source.clone(), target.clone(), raw.kind);
        if seen.contains(&key) {
            continue;
        }
        let edge = KnowledgeEdge {
            source: raw.source,
            target,
            kind: raw.kind,
        };
        let edge_bytes = serialized_item_bytes(&edge);
        if item_bytes.saturating_add(edge_bytes) > max_item_bytes {
            truncated = true;
            break;
        }
        item_bytes += edge_bytes;
        seen.insert(key);
        edges.push(edge);
    }
    (edges, truncated)
}

fn truncate_utf8(mut value: String, max_bytes: usize) -> (String, bool) {
    if value.len() <= max_bytes {
        return (value, false);
    }
    let mut boundary = max_bytes;
    while !value.is_char_boundary(boundary) {
        boundary -= 1;
    }
    value.truncate(boundary);
    (value, true)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::TempDir;

    fn write_page(root: &Path, relative: &str, contents: &str) {
        let path = root.join(relative);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, contents).unwrap();
    }

    #[test]
    fn scanner_enforces_allowlists_and_builds_all_five_edge_kinds() {
        let wiki = TempDir::new().unwrap();
        let project = TempDir::new().unwrap();
        write_page(
            wiki.path(),
            "patterns/source.md",
            "---\ntitle: Source #153\nlast_updated: 2026-07-18\ncross_refs:\n  - wiki/practices/cross.md\n  - clients/cross.md\nrelated: [research/frontmatter-related.md]\n---\n# Source\n[[Wiki Target]]\n-> global: patterns/global.md\n-> related: [[body-related]] (strong match)\n<-> related: wiki/research/frontmatter-related.md (§details)\n<- from: research/from.md\n",
        );
        write_page(
            wiki.path(),
            "practices/cross.md",
            "---\ntitle: Cross\n---\n# Cross\n",
        );
        write_page(
            wiki.path(),
            "practices/wiki-target.md",
            "---\ntitle: Wiki Target\n---\n",
        );
        write_page(wiki.path(), "patterns/global.md", "# Global\n");
        write_page(wiki.path(), "practices/body-related.md", "# Related\n");
        write_page(
            wiki.path(),
            "research/frontmatter-related.md",
            "# Frontmatter Related\n",
        );
        write_page(wiki.path(), "research/from.md", "# From\n");
        write_page(
            wiki.path(),
            "clients/cross.md",
            "---\ntitle: Private Client\n---\n",
        );
        write_page(
            project.path(),
            ".ai-docs/project-dna.md",
            "---\ntitle: Project DNA\n---\n# Project DNA\n",
        );
        write_page(
            project.path(),
            ".ai-docs/bug-patterns.md",
            "# Bug Patterns\n",
        );
        write_page(
            project.path(),
            ".ai-docs/architecture.md",
            "# Must Not Be Scanned\n",
        );

        let graph = scan_knowledge(wiki.path(), Some(project.path()), MAX_NODES);
        let node_ids: HashSet<_> = graph.nodes.iter().map(|node| node.id.as_str()).collect();
        assert!(node_ids.contains("patterns/source"));
        assert!(node_ids.contains("project/project-dna"));
        assert!(node_ids.contains("project/bug-patterns"));
        assert!(!node_ids.iter().any(|id| id.contains("clients")));
        assert!(!node_ids.contains("project/architecture"));

        let kinds: HashSet<_> = graph.edges.iter().map(|edge| edge.kind).collect();
        assert_eq!(
            kinds,
            HashSet::from([
                KnowledgeEdgeKind::CrossRef,
                KnowledgeEdgeKind::Wikilink,
                KnowledgeEdgeKind::Global,
                KnowledgeEdgeKind::Related,
                KnowledgeEdgeKind::From,
            ])
        );
        assert!(graph.edges.iter().all(|edge| {
            node_ids.contains(edge.source.as_str()) && node_ids.contains(edge.target.as_str())
        }));
        assert!(graph.edges.iter().any(|edge| {
            edge.kind == KnowledgeEdgeKind::Related
                && edge.target == "practices/body-related"
        }));
        assert!(graph.edges.iter().any(|edge| {
            edge.kind == KnowledgeEdgeKind::Related
                && edge.target == "research/frontmatter-related"
        }));
        assert!(!serde_json::to_string(&graph)
            .unwrap()
            .contains(&wiki.path().to_string_lossy().to_string()));
        assert_eq!(
            graph
                .nodes
                .iter()
                .find(|node| node.id == "patterns/source")
                .unwrap()
                .title,
            "Source #153"
        );
    }

    #[test]
    fn relative_allowlisted_hints_resolve_without_disallowed_basename_fallback() {
        let nodes = vec![KnowledgeNode {
            id: "patterns/cross".to_string(),
            title: "Cross".to_string(),
            folder: KnowledgeFolder::Patterns,
            path: "patterns/cross.md".to_string(),
            last_updated: String::new(),
            in_degree: 0,
            out_degree: 0,
        }];
        let lookup = NodeLookup::from_nodes(&nodes);
        assert_eq!(
            resolve_hint("patterns/cross.md", &lookup).as_deref(),
            Some("patterns/cross")
        );
        assert_eq!(
            resolve_hint("../patterns/cross.md", &lookup).as_deref(),
            Some("patterns/cross")
        );
        assert_eq!(
            resolve_hint(
                "../../wiki/patterns/cross.md (corpus annotation)",
                &lookup,
            )
            .as_deref(),
            Some("patterns/cross")
        );
        assert_eq!(resolve_hint("clients/cross.md", &lookup), None);
        assert_eq!(resolve_hint("partners/cross.md", &lookup), None);
        assert_eq!(resolve_hint("../clients/cross.md", &lookup), None);
        assert_eq!(resolve_hint("../partners/cross.md", &lookup), None);
        assert_eq!(resolve_hint("../project/project-dna.md", &lookup), None);
        assert_eq!(
            resolve_hint("../patterns/../clients/cross.md", &lookup),
            None
        );
    }

    #[test]
    fn project_root_is_bound_to_the_requested_session_only() {
        let first = Path::new("D:/projects/first");
        let second = Path::new("D:/projects/second");
        let sessions = [("session-a", first), ("session-b", second)];

        assert!(resolve_project_root_from_sessions(None, sessions.iter().copied())
            .ok()
            .flatten()
            .is_none());
        assert_eq!(
            resolve_project_root_from_sessions(Some("session-a"), sessions.iter().copied())
                .ok()
                .flatten()
                .as_deref(),
            Some(first)
        );
        assert_eq!(
            resolve_project_root_from_sessions(Some("session-b"), sessions.iter().copied())
                .ok()
                .flatten()
                .as_deref(),
            Some(second)
        );

        // Keeping the old graph's session id after the live session switches cannot resolve the
        // replacement project's identically-named `project/project-dna` node.
        let switched = [("session-b", second)];
        let error = match resolve_project_root_from_sessions(
            Some("session-a"),
            switched.iter().copied(),
        ) {
            Ok(_) => panic!("stale session unexpectedly resolved a project"),
            Err(error) => error,
        };
        assert_eq!(error.status, axum::http::StatusCode::NOT_FOUND);

        let invalid = match resolve_project_root_from_sessions(
            Some("../session-a"),
            sessions.iter().copied(),
        ) {
            Ok(_) => panic!("invalid session id unexpectedly resolved a project"),
            Err(error) => error,
        };
        assert_eq!(invalid.status, axum::http::StatusCode::BAD_REQUEST);
    }

    #[test]
    fn knowledge_queries_accept_snake_and_camel_case_session_ids() {
        let snake: KnowledgeGraphQuery = serde_json::from_value(serde_json::json!({
            "session_id": "session-a",
            "max_nodes": 12
        }))
        .unwrap();
        assert_eq!(snake.session_id.as_deref(), Some("session-a"));
        assert_eq!(snake.max_nodes, Some(12));

        let camel: KnowledgePageQuery = serde_json::from_value(serde_json::json!({
            "id": "patterns/example",
            "sessionId": "session-b"
        }))
        .unwrap();
        assert_eq!(camel.session_id.as_deref(), Some("session-b"));
    }

    #[test]
    fn preview_is_id_based_strips_frontmatter_and_caps_content() {
        let wiki = TempDir::new().unwrap();
        let body = format!("# Preview\n{}", "é".repeat(MAX_PREVIEW_BYTES));
        write_page(
            wiki.path(),
            "patterns/preview.md",
            &format!(
                "---\ntitle: Preview Title\nlast_updated: 2026-07-18\n---\n{body}"
            ),
        );

        let page = preview_knowledge_page(wiki.path(), None, "patterns/preview").unwrap();
        assert_eq!(page.id, "patterns/preview");
        assert_eq!(page.path, "patterns/preview.md");
        assert_eq!(page.title, "Preview Title");
        assert!(!page.content.contains("last_updated:"));
        assert!(page.truncated);
        assert!(page.content.len() <= MAX_PREVIEW_BYTES);
        assert!(page.content.is_char_boundary(page.content.len()));
        assert!(preview_knowledge_page(wiki.path(), None, "patterns/missing").is_none());
    }

    #[test]
    fn preview_id_validation_rejects_traversal_and_absolute_forms() {
        for invalid in [
            "",
            "../clients/secret",
            "patterns/../clients/secret",
            "/patterns/secret",
            "C:/patterns/secret",
            "\\\\server\\patterns\\secret",
            "patterns\\secret",
            "clients/secret",
            "project/architecture",
        ] {
            assert!(validate_knowledge_id(invalid).is_err(), "accepted {invalid}");
        }
        assert!(validate_knowledge_id("patterns/nested/page").is_ok());
        assert!(validate_knowledge_id("project/project-dna").is_ok());
    }

    #[test]
    fn node_and_file_bounds_degrade_safely() {
        let wiki = TempDir::new().unwrap();
        write_page(wiki.path(), "patterns/a.md", "# A\n");
        write_page(wiki.path(), "patterns/b.md", "# B\n");
        write_page(wiki.path(), "patterns/c.md", "# C\n");
        write_page(
            wiki.path(),
            "patterns/oversize.md",
            &"x".repeat(MAX_FILE_BYTES + 1),
        );

        let graph = scan_knowledge(wiki.path(), None, 2);
        assert_eq!(graph.nodes.len(), 2);
        assert!(graph.truncated);

        let uncapped = scan_knowledge(wiki.path(), None, MAX_NODES);
        assert!(uncapped.truncated);
        assert!(!uncapped
            .nodes
            .iter()
            .any(|node| node.id.ends_with("oversize")));

        let missing = scan_knowledge(&wiki.path().join("missing"), None, MAX_NODES);
        assert!(missing.nodes.is_empty());
        assert!(missing.edges.is_empty());
    }

    #[test]
    fn metadata_and_serialized_graph_size_are_bounded() {
        let wiki = TempDir::new().unwrap();
        let oversized_title = "é".repeat(MAX_TITLE_BYTES);
        let oversized_updated = "x".repeat(MAX_LAST_UPDATED_BYTES + 50);
        write_page(
            wiki.path(),
            "patterns/metadata.md",
            &format!(
                "---\ntitle: {oversized_title}\nlast_updated: {oversized_updated}\n---\n# Body\n"
            ),
        );

        let graph = scan_knowledge(wiki.path(), None, MAX_NODES);
        let node = graph
            .nodes
            .iter()
            .find(|node| node.id == "patterns/metadata")
            .unwrap();
        assert!(node.title.len() <= MAX_TITLE_BYTES);
        assert!(node.title.is_char_boundary(node.title.len()));
        assert!(node.last_updated.len() <= MAX_LAST_UPDATED_BYTES);
        assert!(graph.truncated);
        assert!(serde_json::to_vec(&graph).unwrap().len() <= MAX_GRAPH_RESPONSE_BYTES);

        let page = preview_knowledge_page(wiki.path(), None, "patterns/metadata").unwrap();
        assert!(page.title.len() <= MAX_TITLE_BYTES);
        assert!(page.truncated);
    }

    #[test]
    fn directory_entry_collection_is_bounded_before_sorting() {
        let root = TempDir::new().unwrap();
        for index in 0..12 {
            write_page(root.path(), &format!("page-{index:02}.md"), "# Page\n");
        }
        let canonical_root = fs::canonicalize(root.path()).unwrap();
        let mut entries_seen = MAX_SCAN_ENTRIES - 3;
        let mut output = Vec::new();
        let mut truncated = false;

        collect_markdown_files(
            &canonical_root,
            &canonical_root,
            0,
            MAX_DISCOVERED_FILES,
            &mut entries_seen,
            &mut output,
            &mut truncated,
        );

        assert_eq!(entries_seen, MAX_SCAN_ENTRIES);
        assert_eq!(output.len(), 3);
        assert!(truncated);
    }

    #[test]
    fn resolved_edges_are_hard_capped() {
        let nodes: Vec<_> = (0..51)
            .map(|index| KnowledgeNode {
                id: format!("patterns/page-{index}"),
                title: format!("Page {index}"),
                folder: KnowledgeFolder::Patterns,
                path: format!("patterns/page-{index}.md"),
                last_updated: String::new(),
                in_degree: 0,
                out_degree: 0,
            })
            .collect();
        let lookup = NodeLookup::from_nodes(&nodes);
        let raw_edges = nodes
            .iter()
            .flat_map(|source| {
                nodes.iter().filter_map(move |target| {
                    (source.id != target.id).then(|| RawEdge {
                        source: source.id.clone(),
                        target_hint: target.id.clone(),
                        kind: KnowledgeEdgeKind::CrossRef,
                    })
                })
            })
            .collect();

        let (edges, truncated) = build_edges(raw_edges, &lookup, MAX_GRAPH_ITEM_BYTES);
        assert_eq!(edges.len(), MAX_EDGES);
        assert!(truncated);
    }

    #[test]
    fn resolved_edges_respect_the_serialized_response_budget() {
        let nodes = vec![
            KnowledgeNode {
                id: "patterns/source".to_string(),
                title: "Source".to_string(),
                folder: KnowledgeFolder::Patterns,
                path: "patterns/source.md".to_string(),
                last_updated: String::new(),
                in_degree: 0,
                out_degree: 0,
            },
            KnowledgeNode {
                id: "patterns/target".to_string(),
                title: "Target".to_string(),
                folder: KnowledgeFolder::Patterns,
                path: "patterns/target.md".to_string(),
                last_updated: String::new(),
                in_degree: 0,
                out_degree: 0,
            },
        ];
        let lookup = NodeLookup::from_nodes(&nodes);
        let raw_edges = vec![RawEdge {
            source: "patterns/source".to_string(),
            target_hint: "patterns/target".to_string(),
            kind: KnowledgeEdgeKind::CrossRef,
        }];

        let (edges, truncated) = build_edges(raw_edges, &lookup, 1);
        assert!(edges.is_empty());
        assert!(truncated);
    }

    #[test]
    fn wiki_root_precedence_and_tilde_expansion_are_pure() {
        let home = Path::new("C:/Users/tester");
        assert_eq!(
            resolve_wiki_root_from(
                Some(OsString::from("D:/configured/wiki")),
                Some("~/ignored"),
                Some(home),
            ),
            PathBuf::from("D:/configured/wiki")
        );
        assert_eq!(
            resolve_wiki_root_from(None, Some("~/.ai-docs/wiki"), Some(home)),
            home.join(".ai-docs/wiki")
        );
        assert_eq!(
            expand_tilde_path(Path::new("~someone/wiki"), Some(home)),
            PathBuf::from("~someone/wiki")
        );
        assert_eq!(strip_markdown_extension("éxy"), "éxy");
        assert_eq!(strip_markdown_extension("é.md"), "é");
    }

    #[cfg(unix)]
    #[test]
    fn scanner_skips_symlinked_subtrees_and_files() {
        use std::os::unix::fs::symlink;

        let wiki = TempDir::new().unwrap();
        let private = TempDir::new().unwrap();
        write_page(private.path(), "secret.md", "# Private\n");
        fs::create_dir_all(wiki.path().join("practices")).unwrap();
        symlink(private.path(), wiki.path().join("patterns")).unwrap();
        symlink(
            private.path().join("secret.md"),
            wiki.path().join("practices/secret.md"),
        )
        .unwrap();

        let graph = scan_knowledge(wiki.path(), None, MAX_NODES);
        assert!(graph.nodes.is_empty());
    }
}
