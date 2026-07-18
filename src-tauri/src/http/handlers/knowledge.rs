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

/// Compiled-in ceiling for the global wiki subtrees the Atlas may read. Operator config can only
/// narrow or re-select within this array (see `resolve_wiki_folders_from`); it can never add a
/// folder, so an operator config value can never become an arbitrary filesystem path.
///
/// `agents/` is deliberately excluded, and that exclusion is a product decision, not an oversight:
///   1. Privacy. It holds third-party recruiting-candidate dossiers with personal contact details
///      of people who never consented to appear in a graph UI.
///   2. Signal. It is 63 of roughly 163 otherwise-reachable files, so including it would swamp the
///      graph with one subtree.
///   3. Correctness. It carries 11 unfiltered `README.md` / `_TEMPLATE.md` files. Duplicate
///      basenames collide in the `by_title` / `by_basename` maps, `unique_lookup` then returns
///      `None` for the ambiguous key, and unrelated edges elsewhere in the graph silently drop.
/// Re-including `agents` therefore requires fixing (3) first, not just adding the variant.
const WIKI_FOLDERS: [KnowledgeFolder; 7] = [
    KnowledgeFolder::Patterns,
    KnowledgeFolder::Practices,
    KnowledgeFolder::Research,
    KnowledgeFolder::Clients,
    KnowledgeFolder::Partners,
    KnowledgeFolder::Vendors,
    KnowledgeFolder::Operations,
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
    Clients,
    Partners,
    Vendors,
    Operations,
    Project,
}

impl KnowledgeFolder {
    /// Directory name on disk and the stable node-ID prefix carried over the API.
    ///
    /// This is a *separate* mechanism from the `#[serde(rename_all = "snake_case")]` wire encoding
    /// above; the two only happen to agree. Both must be updated together when a variant is added,
    /// and `as_str_matches_the_serialized_folder_tag` pins them to each other.
    fn as_str(self) -> &'static str {
        match self {
            Self::Patterns => "patterns",
            Self::Practices => "practices",
            Self::Research => "research",
            Self::Clients => "clients",
            Self::Partners => "partners",
            Self::Vendors => "vendors",
            Self::Operations => "operations",
            Self::Project => "project",
        }
    }

    /// Map an ID prefix onto the *compiled-in* allowlist, deliberately ignoring any operator
    /// narrowing. Narrowing is enforced once, at enumeration: a folder excluded by config produces
    /// no sources, so its IDs 404 instead of 400. Keeping validation on the superset means the
    /// status code for an unknown page does not leak which folders an operator has switched off.
    fn from_id_prefix(prefix: &str) -> Option<Self> {
        WIKI_FOLDERS
            .into_iter()
            .chain(std::iter::once(Self::Project))
            .find(|folder| folder.as_str() == prefix)
    }
}

/// Resolve the effective wiki folder set from optional operator config.
///
/// Security property: entries are *matched against* the compiled-in `WIKI_FOLDERS` variants. They
/// select, they never construct. An unrecognized entry — `"agents"`, `"../../../etc"`, an absolute
/// path — is logged and dropped, so config can only ever narrow or reorder the scan, never widen
/// it or aim it at a directory outside the wiki root. Absent or fully-unrecognized config falls
/// back to the built-in default rather than scanning nothing, so a typo degrades to the documented
/// behavior instead of silently emptying the Atlas.
fn resolve_wiki_folders_from(configured: Option<&[String]>) -> Vec<KnowledgeFolder> {
    let Some(configured) = configured else {
        return WIKI_FOLDERS.to_vec();
    };

    let mut selected: Vec<KnowledgeFolder> = Vec::new();
    for raw in configured {
        let name = raw.trim().to_ascii_lowercase();
        if name.is_empty() {
            continue;
        }
        match WIKI_FOLDERS
            .into_iter()
            .find(|folder| folder.as_str() == name)
        {
            Some(folder) => {
                if !selected.contains(&folder) {
                    selected.push(folder);
                }
            }
            None => tracing::warn!(
                entry = %raw,
                "ignoring unrecognized knowledge_wiki_folders entry; the Knowledge Atlas folder \
                 set is bounded to the compiled-in wiki allowlist"
            ),
        }
    }

    if selected.is_empty() {
        WIKI_FOLDERS.to_vec()
    } else {
        selected
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

/// Optional bounds and session context for a Knowledge Atlas graph request.
#[derive(Debug, Default, Deserialize)]
pub struct KnowledgeGraphQuery {
    #[serde(default, alias = "maxNodes")]
    pub max_nodes: Option<usize>,
    #[serde(default, alias = "sessionId")]
    pub session_id: Option<String>,
}

/// Stable page identifier and optional session context for a preview request.
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
    let wiki_folders = resolve_wiki_folders(&state).await;
    let project_root = resolve_project_root(&state, query.session_id.as_deref())?;
    let Ok(_permit) = knowledge_scan_limiter().acquire().await else {
        return Ok(Json(KnowledgeGraphResponse::default()));
    };

    let graph = tokio::task::spawn_blocking(move || {
        scan_knowledge(&wiki_root, &wiki_folders, project_root.as_deref(), max_nodes)
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
    let wiki_folders = resolve_wiki_folders(&state).await;
    let project_root = resolve_project_root(&state, query.session_id.as_deref())?;
    let id = query.id;
    let Ok(_permit) = knowledge_scan_limiter().acquire().await else {
        return Err(ApiError::not_found("Knowledge page not found"));
    };

    let page = tokio::task::spawn_blocking(move || {
        preview_knowledge_page(&wiki_root, &wiki_folders, project_root.as_deref(), &id)
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

async fn resolve_wiki_folders(state: &AppState) -> Vec<KnowledgeFolder> {
    let configured = state.config.read().await.knowledge_wiki_folders.clone();
    resolve_wiki_folders_from(configured.as_deref())
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

/// Build a bounded graph from the global wiki and the session project's allowlisted documents.
///
/// `wiki_folders` is the effective folder set, already resolved against the compiled-in
/// `WIKI_FOLDERS` ceiling by `resolve_wiki_folders_from`. Callers must never synthesize it from
/// raw operator input.
pub(crate) fn scan_knowledge(
    wiki_root: &Path,
    wiki_folders: &[KnowledgeFolder],
    project_root: Option<&Path>,
    max_nodes: usize,
) -> KnowledgeGraphResponse {
    let node_cap = max_nodes.clamp(1, MAX_NODES);
    let enumeration = enumerate_sources(wiki_root, wiki_folders, project_root);
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

/// Resolve an allowlisted page ID and return its bounded Markdown preview.
pub(crate) fn preview_knowledge_page(
    wiki_root: &Path,
    wiki_folders: &[KnowledgeFolder],
    project_root: Option<&Path>,
    id: &str,
) -> Option<KnowledgePageResponse> {
    if validate_knowledge_id(id).is_err() {
        return None;
    }

    let source = enumerate_sources(wiki_root, wiki_folders, project_root)
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

fn enumerate_sources(
    wiki_root: &Path,
    wiki_folders: &[KnowledgeFolder],
    project_root: Option<&Path>,
) -> SourceEnumeration {
    let mut enumeration = SourceEnumeration::default();
    let mut entries_seen = 0;
    let canonical_wiki_root = fs::canonicalize(wiki_root).ok();

    // The subtree path is always `wiki_root` joined with a `KnowledgeFolder::as_str()` literal, so
    // no caller-supplied string ever reaches the filesystem here.
    for &folder in wiki_folders {
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

    // Session-scoped project context is the most relevant part of the graph. Keep the exact
    // project allowlist ahead of the global corpus so it cannot be displaced when the caller's
    // node cap is smaller than (or already saturated by) the wiki.
    enumeration
        .sources
        .sort_by_key(|source| source.folder != KnowledgeFolder::Project);

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
    let Ok(source_metadata) = fs::symlink_metadata(&source.path) else {
        return SourceRead::Unavailable;
    };
    if !source_metadata.is_file() || source_metadata.file_type().is_symlink() {
        return SourceRead::Unavailable;
    }
    let Ok(canonical_path) = fs::canonicalize(&source.path) else {
        return SourceRead::Unavailable;
    };
    if !canonical_path.starts_with(&source.root) {
        return SourceRead::Unavailable;
    }
    let Ok(target_metadata) = fs::symlink_metadata(&canonical_path) else {
        return SourceRead::Unavailable;
    };
    if !target_metadata.is_file()
        || target_metadata.file_type().is_symlink()
        || !validated_path_matches(&source_metadata, &target_metadata)
    {
        return SourceRead::Unavailable;
    }

    let Some((mut file, open_metadata)) = open_file_if_unchanged(&canonical_path, &target_metadata)
    else {
        return SourceRead::Unavailable;
    };
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

/// Open `path` only when the resulting handle still identifies the file validated by the caller.
/// Unix compares device/inode identity; Windows resolves the live handle's final path. Validating
/// the handle, rather than re-checking the pathname, closes the replacement or symlink-swap window
/// between canonicalization and `File::open`.
fn open_file_if_unchanged(
    path: &Path,
    expected_metadata: &fs::Metadata,
) -> Option<(File, fs::Metadata)> {
    let file = File::open(path).ok()?;
    let opened_metadata = file.metadata().ok()?;
    if !opened_metadata.is_file()
        || !opened_file_matches(path, expected_metadata, &file, &opened_metadata)
    {
        return None;
    }
    Some((file, opened_metadata))
}

#[cfg(unix)]
fn validated_path_matches(source: &fs::Metadata, target: &fs::Metadata) -> bool {
    same_unix_file_identity(source, target)
}

#[cfg(windows)]
fn validated_path_matches(_source: &fs::Metadata, _target: &fs::Metadata) -> bool {
    // Windows verifies the final path of the opened handle below, which also detects a swapped
    // parent-directory reparse point.
    true
}

#[cfg(not(any(unix, windows)))]
fn validated_path_matches(_source: &fs::Metadata, _target: &fs::Metadata) -> bool {
    false
}

#[cfg(unix)]
fn opened_file_matches(
    _path: &Path,
    expected: &fs::Metadata,
    _file: &File,
    opened: &fs::Metadata,
) -> bool {
    same_unix_file_identity(expected, opened)
}

#[cfg(unix)]
fn same_unix_file_identity(expected: &fs::Metadata, opened: &fs::Metadata) -> bool {
    use std::os::unix::fs::MetadataExt;

    expected.dev() == opened.dev() && expected.ino() == opened.ino()
}

#[cfg(windows)]
fn opened_file_matches(
    path: &Path,
    _expected: &fs::Metadata,
    file: &File,
    _opened: &fs::Metadata,
) -> bool {
    final_path_for_open_file(file).is_some_and(|opened_path| opened_path == path)
}

#[cfg(windows)]
fn final_path_for_open_file(file: &File) -> Option<PathBuf> {
    use std::os::windows::ffi::OsStringExt;
    use std::os::windows::io::AsRawHandle;

    #[link(name = "kernel32")]
    unsafe extern "system" {
        fn GetFinalPathNameByHandleW(
            file: *mut std::ffi::c_void,
            file_path: *mut u16,
            file_path_len: u32,
            flags: u32,
        ) -> u32;
    }

    let handle = file.as_raw_handle();
    // SAFETY: `handle` belongs to the live `File`; a null buffer with length zero only queries the
    // required UTF-16 buffer length and does not transfer ownership.
    let required = unsafe { GetFinalPathNameByHandleW(handle, std::ptr::null_mut(), 0, 0) };
    if required == 0 {
        return None;
    }
    let mut buffer = vec![0u16; required as usize + 1];
    // SAFETY: `buffer` is writable for the advertised length and the file handle remains live for
    // the duration of the call.
    let written = unsafe {
        GetFinalPathNameByHandleW(handle, buffer.as_mut_ptr(), buffer.len() as u32, 0)
    };
    if written == 0 || written as usize >= buffer.len() {
        return None;
    }
    buffer.truncate(written as usize);
    Some(PathBuf::from(OsString::from_wide(&buffer)))
}

#[cfg(not(any(unix, windows)))]
fn opened_file_matches(
    _path: &Path,
    _expected: &fs::Metadata,
    _file: &File,
    _opened: &fs::Metadata,
) -> bool {
    // File identity APIs are platform-specific. Refuse to read rather than weakening the
    // containment guarantee on an unsupported target.
    false
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
/// components are still normalized narrowly: after removing leading parents, the hint must name one
/// of the global wiki allow-list roots in `WIKI_FOLDERS`. That set now spans the operational roots
/// (`patterns`, `practices`, `research`) *and* the relationship/entity roots (`clients`,
/// `partners`, `vendors`, `operations`), so `../clients/foo.md` resolves — that cross-half linking
/// is the point of this folder set.
///
/// Still invalid, by design: `../agents/foo.md` (never allow-listed, see `WIKI_FOLDERS`) and
/// `../project/project-dna.md` (`project` is session-scoped and must not be reachable from a
/// global-wiki hint).
///
/// This gate intentionally uses the compiled-in ceiling rather than the operator-narrowed set. A
/// hint only becomes an edge when it resolves to a node that was actually enumerated, so narrowing
/// already removes those edges; re-checking here would add nothing but a second source of truth.
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
            "---\ntitle: Source #153\nlast_updated: 2026-07-18\ncross_refs:\n  - wiki/practices/cross.md\n  - agents/cross.md\nrelated: [research/frontmatter-related.md]\n---\n# Source\n[[Wiki Target]]\n-> global: patterns/global.md\n-> related: [[body-related]] (strong match)\n<-> related: wiki/research/frontmatter-related.md (§details)\n<- from: research/from.md\n",
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
        // `agents/` is the never-allow-listed folder, so this fixture keeps proving the allowlist
        // gate now that `clients/` is deliberately in-scope.
        write_page(
            wiki.path(),
            "agents/cross.md",
            "---\ntitle: Recruiting Dossier\n---\n",
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

        let graph = scan_knowledge(
            wiki.path(),
            &WIKI_FOLDERS,
            Some(project.path()),
            MAX_NODES,
        );
        let node_ids: HashSet<_> = graph.nodes.iter().map(|node| node.id.as_str()).collect();
        assert!(node_ids.contains("patterns/source"));
        assert!(node_ids.contains("project/project-dna"));
        assert!(node_ids.contains("project/bug-patterns"));
        assert!(!node_ids.iter().any(|id| id.contains("agents")));
        assert!(!node_ids.contains("project/architecture"));
        assert!(!serde_json::to_string(&graph)
            .unwrap()
            .contains("Recruiting Dossier"));

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
    fn project_nodes_are_reserved_ahead_of_a_saturated_global_corpus() {
        let wiki = TempDir::new().unwrap();
        let project = TempDir::new().unwrap();
        for index in 0..MAX_NODES {
            write_page(
                wiki.path(),
                &format!("patterns/global-{index:03}.md"),
                &format!("# Global {index}\n"),
            );
        }
        write_page(
            project.path(),
            ".ai-docs/project-dna.md",
            "# Project DNA\n",
        );
        write_page(
            project.path(),
            ".ai-docs/bug-patterns.md",
            "# Bug Patterns\n",
        );

        let graph = scan_knowledge(wiki.path(), &WIKI_FOLDERS, Some(project.path()), MAX_NODES);
        let node_ids: Vec<_> = graph.nodes.iter().map(|node| node.id.as_str()).collect();

        assert_eq!(graph.nodes.len(), MAX_NODES);
        assert_eq!(
            &node_ids[..PROJECT_FILES.len()],
            &["project/project-dna", "project/bug-patterns"]
        );
        assert!(graph.truncated);
    }

    #[cfg(unix)]
    #[test]
    fn opened_file_must_match_the_previously_validated_identity() {
        let root = TempDir::new().unwrap();
        let path = root.path().join("page.md");
        let original_path = root.path().join("original.md");
        fs::write(&path, "# Expected\n").unwrap();
        let expected_metadata = fs::symlink_metadata(&path).unwrap();

        let (original_handle, _) = open_file_if_unchanged(&path, &expected_metadata).unwrap();
        drop(original_handle);

        // Keep the original file alive so its OS identity cannot be recycled, then replace its
        // pathname exactly as a validation-to-open race would.
        fs::rename(&path, &original_path).unwrap();
        fs::write(&path, "# Replacement\n").unwrap();

        assert!(open_file_if_unchanged(&path, &expected_metadata).is_none());
    }

    #[cfg(windows)]
    #[test]
    fn opened_handle_must_resolve_to_the_validated_windows_path() {
        let root = TempDir::new().unwrap();
        let expected_path = root.path().join("expected.md");
        let other_path = root.path().join("other.md");
        fs::write(&expected_path, "# Expected\n").unwrap();
        fs::write(&other_path, "# Other\n").unwrap();
        let expected_path = fs::canonicalize(expected_path).unwrap();
        let expected_metadata = fs::symlink_metadata(&expected_path).unwrap();
        let other_file = File::open(other_path).unwrap();
        let other_metadata = other_file.metadata().unwrap();

        assert!(!opened_file_matches(
            &expected_path,
            &expected_metadata,
            &other_file,
            &other_metadata,
        ));
    }

    fn test_node(id: &str, title: &str, folder: KnowledgeFolder) -> KnowledgeNode {
        KnowledgeNode {
            id: id.to_string(),
            title: title.to_string(),
            folder,
            path: format!("{id}.md"),
            last_updated: String::new(),
            in_degree: 0,
            out_degree: 0,
        }
    }

    /// The entity nodes below must actually exist for this test to mean anything. With a
    /// patterns-only fixture the `clients/*` assertions passed for the wrong reason — the
    /// exact-ID lookup simply missed and the explicit-path guard blocked the basename fallback —
    /// so they would have stayed green even if the allowlist gate had been deleted entirely.
    #[test]
    fn relative_allowlisted_hints_resolve_without_disallowed_basename_fallback() {
        let nodes = vec![
            test_node("patterns/cross", "Pattern Cross", KnowledgeFolder::Patterns),
            test_node("clients/cross", "Client Cross", KnowledgeFolder::Clients),
            test_node("partners/cross", "Partner Cross", KnowledgeFolder::Partners),
            test_node("vendors/cross", "Vendor Cross", KnowledgeFolder::Vendors),
            test_node(
                "operations/cross",
                "Operations Cross",
                KnowledgeFolder::Operations,
            ),
            test_node(
                "clients/acme/dashboard",
                "Acme Dashboard",
                KnowledgeFolder::Clients,
            ),
        ];
        let lookup = NodeLookup::from_nodes(&nodes);

        for (hint, expected) in [
            ("patterns/cross.md", "patterns/cross"),
            ("../patterns/cross.md", "patterns/cross"),
            (
                "../../wiki/patterns/cross.md (corpus annotation)",
                "patterns/cross",
            ),
            // The entity roots resolve now, including through a parent-relative hint and into a
            // nested `clients/<slug>/dashboard.md` page.
            ("clients/cross.md", "clients/cross"),
            ("../clients/cross.md", "clients/cross"),
            ("partners/cross.md", "partners/cross"),
            ("../partners/cross.md", "partners/cross"),
            ("vendors/cross.md", "vendors/cross"),
            ("operations/cross.md", "operations/cross"),
            ("clients/acme/dashboard.md", "clients/acme/dashboard"),
            ("../clients/acme/dashboard.md", "clients/acme/dashboard"),
        ] {
            assert_eq!(
                resolve_hint(hint, &lookup).as_deref(),
                Some(expected),
                "hint {hint} did not resolve"
            );
        }

        // `agents/` is never allow-listed, and an explicit path must not fall back by basename to
        // one of the five genuinely-present `*/cross` nodes.
        for rejected in [
            "agents/cross.md",
            "../agents/cross.md",
            "agents/recruiting/candidate.md",
            "../project/project-dna.md",
            "../patterns/../agents/cross.md",
        ] {
            assert_eq!(resolve_hint(rejected, &lookup), None, "accepted {rejected}");
        }

        // Ambiguity guard: the bare basename `cross` now maps to five nodes across five folders,
        // so `unique_lookup` declines rather than inventing an edge to whichever one hashed first.
        // No node is titled plain "Cross", so the title fallback cannot rescue it either -- this is
        // exactly the duplicate-basename failure mode that keeps `agents/` out of `WIKI_FOLDERS`.
        assert_eq!(resolve_hint("cross", &lookup), None);
        // An unambiguous title still resolves, so the guard above is narrow, not a blanket block.
        assert_eq!(
            resolve_hint("Acme Dashboard", &lookup).as_deref(),
            Some("clients/acme/dashboard")
        );
    }

    /// `as_str()` (directory name + node-ID prefix) and the `serde(rename_all = "snake_case")`
    /// wire tag are independent mechanisms that only happen to agree. Adding a variant that
    /// updates one but not the other would ship a graph whose folder tags do not match the IDs
    /// the frontend filters on, so pin them to each other.
    #[test]
    fn as_str_matches_the_serialized_folder_tag() {
        for folder in WIKI_FOLDERS
            .into_iter()
            .chain(std::iter::once(KnowledgeFolder::Project))
        {
            let serialized = serde_json::to_string(&folder).unwrap();
            assert_eq!(
                serialized,
                format!("\"{}\"", folder.as_str()),
                "serde tag and as_str disagree for {folder:?}"
            );
        }

        assert_eq!(
            WIKI_FOLDERS.map(KnowledgeFolder::as_str),
            [
                "patterns",
                "practices",
                "research",
                "clients",
                "partners",
                "vendors",
                "operations",
            ]
        );
    }

    #[test]
    fn id_prefix_gate_admits_entity_roots_and_still_rejects_agents() {
        for allowed in [
            "patterns",
            "practices",
            "research",
            "clients",
            "partners",
            "vendors",
            "operations",
            "project",
        ] {
            assert!(
                KnowledgeFolder::from_id_prefix(allowed).is_some(),
                "{allowed} should be an allow-listed ID prefix"
            );
        }
        for rejected in ["agents", "", "..", "Clients", "clients/acme", "etc"] {
            assert!(
                KnowledgeFolder::from_id_prefix(rejected).is_none(),
                "{rejected} should not be an allow-listed ID prefix"
            );
        }
    }

    /// Operator config may only narrow or re-select within `WIKI_FOLDERS`. It must never be able to
    /// widen the scan back to `agents/`, nor turn a string into a filesystem path.
    #[test]
    fn configured_folders_can_only_narrow_the_compiled_allowlist() {
        assert_eq!(resolve_wiki_folders_from(None), WIKI_FOLDERS.to_vec());

        // Absent-equivalent inputs fall back to the documented default rather than scanning
        // nothing, so a typo degrades gracefully instead of silently emptying the Atlas.
        assert_eq!(resolve_wiki_folders_from(Some(&[])), WIKI_FOLDERS.to_vec());
        assert_eq!(
            resolve_wiki_folders_from(Some(&[String::new()])),
            WIKI_FOLDERS.to_vec()
        );
        assert_eq!(
            resolve_wiki_folders_from(Some(&["   ".to_string()])),
            WIKI_FOLDERS.to_vec()
        );

        // Narrowing: a privacy-conscious machine drops the entity roots.
        assert_eq!(
            resolve_wiki_folders_from(Some(&[
                "patterns".to_string(),
                "practices".to_string()
            ])),
            vec![KnowledgeFolder::Patterns, KnowledgeFolder::Practices]
        );

        // Case and surrounding whitespace are tolerated; duplicates collapse.
        assert_eq!(
            resolve_wiki_folders_from(Some(&[
                "  Clients ".to_string(),
                "CLIENTS".to_string(),
                "clients".to_string(),
            ])),
            vec![KnowledgeFolder::Clients]
        );

        // Widening attempts are dropped, not honored, and never become paths.
        assert_eq!(
            resolve_wiki_folders_from(Some(&[
                "agents".to_string(),
                "../../../etc".to_string(),
                "/etc/passwd".to_string(),
                "C:\\Windows\\System32".to_string(),
                "project".to_string(),
                "..".to_string(),
                "clients/../agents".to_string(),
                "research".to_string(),
            ])),
            vec![KnowledgeFolder::Research],
            "config must select among compiled-in variants, never construct new ones"
        );

        // Even an entirely hostile list cannot widen: it collapses to the built-in default, which
        // still excludes `agents` and `project`.
        let effective = resolve_wiki_folders_from(Some(&[
            "agents".to_string(),
            "../../../etc".to_string(),
        ]));
        assert_eq!(effective, WIKI_FOLDERS.to_vec());
        assert!(!effective.contains(&KnowledgeFolder::Project));
    }

    /// End-to-end proof that narrowing is enforced where it matters — the filesystem walk — and
    /// that an excluded folder yields no nodes and no preview.
    #[test]
    fn configured_narrowing_is_enforced_at_enumeration() {
        let wiki = TempDir::new().unwrap();
        write_page(
            wiki.path(),
            "patterns/kept.md",
            "---\ntitle: Kept\n---\n# Kept\n",
        );
        write_page(
            wiki.path(),
            "clients/acme/dashboard.md",
            "---\ntitle: Acme\n---\n# Acme\n",
        );
        write_page(
            wiki.path(),
            "agents/recruiting/candidate.md",
            "---\ntitle: Candidate\n---\n# Candidate\n",
        );

        let full = resolve_wiki_folders_from(None);
        let graph = scan_knowledge(wiki.path(), &full, None, MAX_NODES);
        let ids: HashSet<_> = graph.nodes.iter().map(|node| node.id.as_str()).collect();
        assert!(ids.contains("patterns/kept"));
        assert!(ids.contains("clients/acme/dashboard"));
        assert!(!ids.iter().any(|id| id.starts_with("agents/")));

        // A machine that narrows to `patterns` loses the entity nodes...
        let narrowed = resolve_wiki_folders_from(Some(&["patterns".to_string()]));
        let narrowed_graph = scan_knowledge(wiki.path(), &narrowed, None, MAX_NODES);
        let narrowed_ids: HashSet<_> = narrowed_graph
            .nodes
            .iter()
            .map(|node| node.id.as_str())
            .collect();
        assert_eq!(narrowed_ids, HashSet::from(["patterns/kept"]));
        assert!(
            preview_knowledge_page(wiki.path(), &narrowed, None, "clients/acme/dashboard")
                .is_none(),
            "a config-excluded folder must not be previewable"
        );

        // ...and a config naming `agents` cannot get it back.
        let hostile =
            resolve_wiki_folders_from(Some(&["agents".to_string(), "patterns".to_string()]));
        let hostile_graph = scan_knowledge(wiki.path(), &hostile, None, MAX_NODES);
        assert!(!hostile_graph
            .nodes
            .iter()
            .any(|node| node.id.starts_with("agents/")));
        assert!(
            preview_knowledge_page(wiki.path(), &hostile, None, "agents/recruiting/candidate")
                .is_none()
        );
    }

    /// Nested entity pages and the operational/relationship cross-half edges the Atlas exists to
    /// show. `clients/<slug>/dashboard.md` is a future corpus shape, so it only exists here.
    #[test]
    fn nested_entity_pages_scan_and_cross_link_between_graph_halves() {
        let wiki = TempDir::new().unwrap();
        write_page(
            wiki.path(),
            "clients/acme/dashboard.md",
            "---\ntitle: Acme Dashboard\nlast_updated: 2026-07-18\n---\n# Acme\n\
             [[partners/beta]]\n",
        );
        write_page(
            wiki.path(),
            "partners/beta.md",
            "---\ntitle: Beta Partner\n---\n# Beta\n",
        );
        write_page(
            wiki.path(),
            "patterns/delivery.md",
            "---\ntitle: Delivery\ncross_refs:\n  - clients/acme/dashboard.md\n---\n# Delivery\n\
             [[partners/beta]]\n",
        );

        let folders = resolve_wiki_folders_from(None);
        let graph = scan_knowledge(wiki.path(), &folders, None, MAX_NODES);
        let ids: HashSet<_> = graph.nodes.iter().map(|node| node.id.as_str()).collect();
        assert!(ids.contains("clients/acme/dashboard"));
        assert!(ids.contains("partners/beta"));

        let dashboard = graph
            .nodes
            .iter()
            .find(|node| node.id == "clients/acme/dashboard")
            .unwrap();
        assert_eq!(dashboard.title, "Acme Dashboard");
        assert_eq!(dashboard.folder, KnowledgeFolder::Clients);
        assert_eq!(dashboard.path, "clients/acme/dashboard.md");

        let has_edge = |source: &str, target: &str, kind: KnowledgeEdgeKind| {
            graph
                .edges
                .iter()
                .any(|edge| edge.source == source && edge.target == target && edge.kind == kind)
        };
        // Entity <-> entity.
        assert!(has_edge(
            "clients/acme/dashboard",
            "partners/beta",
            KnowledgeEdgeKind::Wikilink
        ));
        // Operational -> entity, joining the two halves of the graph.
        assert!(has_edge(
            "patterns/delivery",
            "clients/acme/dashboard",
            KnowledgeEdgeKind::CrossRef
        ));
        assert!(has_edge(
            "patterns/delivery",
            "partners/beta",
            KnowledgeEdgeKind::Wikilink
        ));
        assert_eq!(dashboard.in_degree, 1);
        assert_eq!(dashboard.out_degree, 1);

        assert!(!serde_json::to_string(&graph)
            .unwrap()
            .contains(&wiki.path().to_string_lossy().to_string()));

        let page =
            preview_knowledge_page(wiki.path(), &folders, None, "clients/acme/dashboard").unwrap();
        assert_eq!(page.path, "clients/acme/dashboard.md");
        assert_eq!(page.folder, KnowledgeFolder::Clients);
        assert!(!serde_json::to_string(&page)
            .unwrap()
            .contains(&wiki.path().to_string_lossy().to_string()));
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

        let page = preview_knowledge_page(wiki.path(), &WIKI_FOLDERS, None, "patterns/preview").unwrap();
        assert_eq!(page.id, "patterns/preview");
        assert_eq!(page.path, "patterns/preview.md");
        assert_eq!(page.title, "Preview Title");
        assert!(!page.content.contains("last_updated:"));
        assert!(page.truncated);
        assert!(page.content.len() <= MAX_PREVIEW_BYTES);
        assert!(page.content.is_char_boundary(page.content.len()));
        assert!(preview_knowledge_page(wiki.path(), &WIKI_FOLDERS, None, "patterns/missing").is_none());
    }

    #[test]
    fn preview_id_validation_rejects_traversal_and_absolute_forms() {
        for invalid in [
            "",
            // `agents/` replaces the old `clients/` cases: `clients` is allow-listed now, so
            // reusing it here would assert nothing about the folder gate.
            "../agents/secret",
            "patterns/../agents/secret",
            "clients/../agents/secret",
            "/patterns/secret",
            "C:/patterns/secret",
            "\\\\server\\patterns\\secret",
            "patterns\\secret",
            "clients\\acme\\dashboard",
            "agents/secret",
            "agents/recruiting/candidate",
            "clients",
            "project/architecture",
        ] {
            assert!(validate_knowledge_id(invalid).is_err(), "accepted {invalid}");
        }
        for valid in [
            "patterns/nested/page",
            "project/project-dna",
            "clients/acme",
            "clients/acme/dashboard",
            "partners/beta",
            "vendors/gamma",
            "operations/runbook",
        ] {
            assert!(validate_knowledge_id(valid).is_ok(), "rejected {valid}");
        }
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

        let graph = scan_knowledge(wiki.path(), &WIKI_FOLDERS, None, 2);
        assert_eq!(graph.nodes.len(), 2);
        assert!(graph.truncated);

        let uncapped = scan_knowledge(wiki.path(), &WIKI_FOLDERS, None, MAX_NODES);
        assert!(uncapped.truncated);
        assert!(!uncapped
            .nodes
            .iter()
            .any(|node| node.id.ends_with("oversize")));

        let missing = scan_knowledge(&wiki.path().join("missing"), &WIKI_FOLDERS, None, MAX_NODES);
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

        let graph = scan_knowledge(wiki.path(), &WIKI_FOLDERS, None, MAX_NODES);
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

        let page = preview_knowledge_page(wiki.path(), &WIKI_FOLDERS, None, "patterns/metadata").unwrap();
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

        let graph = scan_knowledge(wiki.path(), &WIKI_FOLDERS, None, MAX_NODES);
        assert!(graph.nodes.is_empty());
    }
}
