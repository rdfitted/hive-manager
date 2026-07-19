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

const PROJECT_FILES: [&str; 2] = ["project-dna.md", "bug-patterns.md"];
const DEFAULT_WIKI_ROOT: &str = "~/.ai-docs/wiki";

pub(crate) const MAX_NODES: usize = 5_000;
pub(crate) const MAX_EDGES: usize = 20_000;
pub(crate) const MAX_FILE_BYTES: usize = 1024 * 1024;
const MAX_PREVIEW_BYTES: usize = 20 * 1024;
const MAX_TITLE_BYTES: usize = 512;
const MAX_LAST_UPDATED_BYTES: usize = 128;
const MAX_EDGE_HINT_BYTES: usize = 4 * 1024;
const MAX_DISCOVERED_FILES: usize = MAX_NODES * 3;
const MAX_RAW_EDGES: usize = MAX_EDGES * 8;
const MAX_SCAN_ENTRIES: usize = 100_000;
const MAX_DIRECTORY_DEPTH: usize = 32;
/// The one cap that stays deliberately tight. Everything above bounds *work*; this bounds the
/// single JSON document the browser must parse and hold before the force simulation can start, and
/// no amount of "show me everything" makes an unbounded response a good idea in a webview. 8 MiB
/// comfortably holds `MAX_NODES` nodes plus `MAX_EDGES` edges (~3 MiB at observed item sizes), so
/// in practice the node and edge caps bind first and this only ever catches a pathological corpus.
const MAX_GRAPH_RESPONSE_BYTES: usize = 8 * 1024 * 1024;
const GRAPH_RESPONSE_OVERHEAD_RESERVE: usize = 256 * 1024;
const MAX_GRAPH_ITEM_BYTES: usize = MAX_GRAPH_RESPONSE_BYTES - GRAPH_RESPONSE_OVERHEAD_RESERVE;
const MAX_CONCURRENT_SCANS: usize = 2;
const MAX_FOLDER_NAME_BYTES: usize = 128;
const MAX_OMISSION_EXAMPLES: usize = 5;

static KNOWLEDGE_SCAN_LIMITER: OnceLock<Semaphore> = OnceLock::new();

/// A top-level folder of the corpus, identified by its directory name.
///
/// This used to be a closed enum of seven blessed folder names, which meant the operator's own
/// `agents/` subtree (63 of their 163 pages) was invisible with no way to opt in. The trust
/// boundary now sits where it actually belongs — the configured wiki root — and the folder set is
/// discovered from disk by `discover_wiki_folders`. A `KnowledgeFolder` is therefore only ever
/// constructed from (a) a directory entry that already passed canonicalization and containment
/// checks under the root, (b) the reserved `project` tag, or (c) the reserved `root` tag. It is
/// never built from a request parameter, so it can never carry a caller-chosen path fragment.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize)]
#[serde(transparent)]
pub(crate) struct KnowledgeFolder(String);

impl KnowledgeFolder {
    /// Session-scoped project knowledge. Reserved and *unforgeable*: the leading dot is
    /// load-bearing, because `is_valid_folder_name` rejects dot-prefixed names and
    /// `discover_wiki_folders` skips dot-directories. A real `<wiki>/project/` directory therefore
    /// keeps its own `project/` namespace instead of colliding with this tag.
    ///
    /// The bare name `project` used to be both, which meant a wiki `project/` directory produced
    /// graph nodes the preview endpoint answered with 400, and a `<wiki>/project/project-dna.md`
    /// silently displaced the session's own `.ai-docs/project-dna.md`.
    const PROJECT: &'static str = ".project";
    /// Markdown sitting loose at the wiki root, in no folder at all — the operator's `index.md`,
    /// `schema.md`, and `log.md` live here and were previously unreachable by construction.
    const ROOT: &'static str = "root";

    fn new(name: impl Into<String>) -> Self {
        Self(name.into())
    }

    fn project() -> Self {
        Self::new(Self::PROJECT)
    }

    fn wiki_root() -> Self {
        Self::new(Self::ROOT)
    }

    /// Directory name on disk and the stable node-ID prefix carried over the API. Because the type
    /// is `#[serde(transparent)]`, the wire encoding *is* this string — the two can no longer drift
    /// apart the way the old enum's `as_str` and `rename_all` tag could.
    fn as_str(&self) -> &str {
        &self.0
    }

    fn is_project(&self) -> bool {
        self.0 == Self::PROJECT
    }

    /// True for the synthetic root-level bucket, which is scanned non-recursively so its pages
    /// cannot duplicate the pages of the real folders beside it.
    fn is_wiki_root(&self) -> bool {
        self.0 == Self::ROOT
    }
}

/// Shape gate for a name that may become a folder tag or an ID prefix.
///
/// This is a *syntactic* check, not an allowlist: it rejects anything that could act as a path
/// fragment or escape a directory level. Containment itself is enforced by canonicalization at
/// enumeration, not here.
fn is_valid_folder_name(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= MAX_FOLDER_NAME_BYTES
        // Also excludes "." and "..", and skips editor/VCS state such as `.obsidian` and `.git`.
        && !name.starts_with('.')
        && !name.contains(['/', '\\', ':', '\0'])
}

/// Discover the corpus folder set by reading the wiki root.
///
/// The security argument moved, it did not weaken. Previously a compiled-in name list bounded the
/// scan; now the *canonical wiki root* bounds it, and every level is still checked:
///   - the root itself is canonicalized once, up front;
///   - each entry's type comes from `read_dir`'s `file_type`, which is `lstat`-based, so a symlink
///     is identified as a symlink and refused outright rather than followed;
///   - each surviving directory is canonicalized and must still `starts_with` the canonical root,
///     so a junction/reparse point aimed outside the wiki is dropped even on Windows;
///   - names are shape-checked by `is_valid_folder_name`.
/// Dot-directories are skipped: `.git` and `.obsidian` are version-control and editor state, not
/// memory, and walking `.git` would add thousands of entries for zero recall. That is a content
/// judgement the operator can reverse by moving a folder, not a privacy boundary.
fn discover_wiki_folders(wiki_root: &Path) -> DiscoveredFolders {
    let Ok(canonical_root) = fs::canonicalize(wiki_root) else {
        return DiscoveredFolders::default();
    };
    let Ok(read_dir) = fs::read_dir(&canonical_root) else {
        return DiscoveredFolders::default();
    };

    let mut folders: Vec<KnowledgeFolder> = Vec::new();
    let mut refusals: Vec<FolderRefusal> = Vec::new();
    let mut has_root_markdown = false;
    for entry in read_dir.take(MAX_SCAN_ENTRIES).flatten() {
        let raw_name = entry.file_name();
        let Some(name) = raw_name.to_str() else {
            // No UTF-8 name exists to use as an example, so the reason travels without one.
            refusals.push((KnowledgeOmissionReason::FolderNameRejected, None));
            continue;
        };
        let Ok(file_type) = entry.file_type() else {
            continue;
        };
        if file_type.is_symlink() {
            refusals.push((
                KnowledgeOmissionReason::FolderLinkRefused,
                Some(bounded_folder_label(name)),
            ));
            continue;
        }
        if file_type.is_file() {
            if !name.starts_with('.') && has_markdown_extension(Path::new(name)) {
                has_root_markdown = true;
            }
            continue;
        }
        // The dot-directory skip stays silent: `.git` and `.obsidian` are editor and VCS state, a
        // documented content judgement, not a loss the operator needs told about.
        if name.starts_with('.') || !file_type.is_dir() {
            continue;
        }
        if !is_valid_folder_name(name) {
            refusals.push((
                KnowledgeOmissionReason::FolderNameRejected,
                Some(bounded_folder_label(name)),
            ));
            continue;
        }
        let Ok(canonical) = fs::canonicalize(entry.path()) else {
            refusals.push((
                KnowledgeOmissionReason::FolderLinkRefused,
                Some(bounded_folder_label(name)),
            ));
            continue;
        };
        if !canonical.starts_with(&canonical_root) {
            refusals.push((
                KnowledgeOmissionReason::FolderLinkRefused,
                Some(bounded_folder_label(name)),
            ));
            continue;
        }
        folders.push(KnowledgeFolder::new(name));
    }
    folders.sort();
    folders.dedup();

    // A real directory named `root` and the synthetic bucket share the `root/` tag, and
    // `enumerate_sources` walks *both* subtrees under it, so emitting the tag once is enough. If a
    // real `root/` directory was discovered it is already in `folders`; only add the synthetic tag
    // when loose markdown exists and nothing claimed the tag yet. Should the two mint the same node
    // ID (`<wiki>/index.md` and `<wiki>/root/index.md` both give `root/index`), the collision is
    // reported as `DuplicateId` rather than dropped in silence.
    if has_root_markdown && !folders.iter().any(KnowledgeFolder::is_wiki_root) {
        folders.insert(0, KnowledgeFolder::wiki_root());
    }
    DiscoveredFolders { folders, refusals }
}

/// A top-level directory discovery refused, plus the folder-relative name to show as an example.
/// Safe by construction: a bare directory name can never be an absolute path.
type FolderRefusal = (KnowledgeOmissionReason, Option<String>);

/// What `discover_wiki_folders` found, and what it turned away.
///
/// Discovery used to return a bare `Vec` with no reporting channel, so every directory it refused
/// vanished while the response still said `truncated=false, omissions=[]`. An entire top-level
/// subtree could disappear with no signal anywhere — not even in the UI folder filter, which is
/// derived from node folders. The refusals ride along so the scan can replay them into its report.
#[derive(Debug, Default)]
struct DiscoveredFolders {
    folders: Vec<KnowledgeFolder>,
    refusals: Vec<FolderRefusal>,
}

/// Bound a folder name before it is echoed as an omission example, so an adversarially long
/// directory name cannot inflate the response.
fn bounded_folder_label(name: &str) -> String {
    truncate_utf8(name.to_string(), MAX_FOLDER_NAME_BYTES).0
}

/// Resolve the effective folder set: everything discovered under the root, optionally narrowed by
/// operator config.
///
/// Security property is unchanged in kind: entries are *matched against* the discovered folders.
/// They select, they never construct. An entry naming a folder that does not exist on disk —
/// `"../../../etc"`, `/etc/passwd`, `C:\Windows\System32` — matches nothing and is dropped, so
/// config can still only ever narrow or reorder the scan, never aim it outside the wiki root.
///
/// Fail closed on a non-empty-but-unrecognized list: `None` (the key absent, which is what every
/// pre-existing `config.json` deserializes to) means "no operator opinion" and selects everything
/// discovered. `Some(_)` is an operator-supplied boundary and is never widened past what it names,
/// so a typo empties the Atlas — loud, obvious, self-correcting — instead of silently restoring
/// folders the operator believed they had excluded.
fn resolve_wiki_folders_from(
    configured: Option<&[String]>,
    discovered: Vec<KnowledgeFolder>,
) -> Vec<KnowledgeFolder> {
    let Some(configured) = configured else {
        return discovered;
    };

    let mut selected: Vec<KnowledgeFolder> = Vec::new();
    for raw in configured {
        let name = raw.trim();
        if name.is_empty() {
            continue;
        }
        match discovered
            .iter()
            .find(|folder| folder.as_str().eq_ignore_ascii_case(name))
        {
            Some(folder) => {
                if !selected.contains(folder) {
                    selected.push(folder.clone());
                }
            }
            None => tracing::warn!(
                entry = %raw,
                "ignoring unrecognized knowledge_wiki_folders entry; the Knowledge Atlas folder \
                 set is bounded to the folders discovered under the configured wiki root"
            ),
        }
    }

    if selected.is_empty() {
        // `error!` rather than `warn!`: `lib.rs` initializes `EnvFilter::from_default_env()` with no
        // fallback directive, so with `RUST_LOG` unset — the normal case for a GUI-launched desktop
        // build — anything below ERROR is filtered out and this failure would be fully silent.
        tracing::error!(
            configured_entries = configured.len(),
            "knowledge_wiki_folders named no folder that exists under the wiki root; the Knowledge \
             Atlas will scan no global-wiki folders. Remove the key entirely to scan everything."
        );
    }
    selected
}

/// Why something the operator might have expected to see is not in the response.
///
/// A single `truncated: bool` was the original sin here: it was assigned in a dozen places, so an
/// amber banner meant any one of "your node cap was hit", "a page was too big", "a link hint was
/// long", or "a title got shortened" — with no way to tell which. Every one of those is now a
/// distinct, countable reason.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum KnowledgeOmissionReason {
    /// Whole pages dropped because the requested/absolute node cap filled up.
    NodeCapReached,
    /// Whole pages dropped because the serialized response would have exceeded its byte budget.
    ResponseSizeCapReached,
    /// A page exists but exceeds `MAX_FILE_BYTES` and was not parsed.
    FileTooLarge,
    /// A page was enumerated but could not be read back (removed mid-scan, non-UTF-8, or it failed
    /// the open-handle identity re-check).
    FileUnreadable,
    /// The directory walk hit its global entry ceiling, so some directories went unvisited.
    ScanEntryCapReached,
    /// A directory nested deeper than `MAX_DIRECTORY_DEPTH` was not descended into.
    DirectoryDepthExceeded,
    /// More markdown files exist under the root than the enumerator will collect.
    DiscoveredFileCapReached,
    /// Resolved links dropped at the edge cap or the edge byte budget. Pages are unaffected.
    EdgeCapReached,
    /// Unresolved link candidates dropped before resolution. Pages are unaffected.
    RawEdgeCapReached,
    /// A link *hint* was longer than `MAX_EDGE_HINT_BYTES` and was not considered as a link.
    EdgeHintTooLong,
    /// A page title was shortened for display. The page itself is present.
    TitleTruncated,
    /// A `last_updated` value was shortened for display. The page itself is present.
    LastUpdatedTruncated,
    /// A page preview was cut at `MAX_PREVIEW_BYTES`. The page itself is present.
    ContentTruncated,
    /// Two sources minted the same node ID, so only the first is in the graph. Reachable when a
    /// real `root/` directory sits beside loose root-level markdown (`<wiki>/index.md` and
    /// `<wiki>/root/index.md` both mint `root/index`). One page was kept; the other is gone.
    DuplicateId,
    /// A top-level directory was a link or reparse point and was not followed, so its whole subtree
    /// is absent. Containment, not a cap — but the operator still lost pages.
    FolderLinkRefused,
    /// A top-level directory name could not be used as a folder tag (too long, or not UTF-8).
    FolderNameRejected,
}

impl KnowledgeOmissionReason {
    /// One sentence, safe to render verbatim in the UI. Written to say plainly whether the operator
    /// lost a *page* or merely some *decoration*, because that is the distinction the old single
    /// boolean destroyed.
    fn detail(self) -> &'static str {
        match self {
            Self::NodeCapReached => "pages omitted: the node cap for this request was reached",
            Self::ResponseSizeCapReached => {
                "pages omitted: the response byte budget was reached"
            }
            Self::FileTooLarge => "pages omitted: the file is larger than the read limit",
            Self::FileUnreadable => {
                "pages omitted: the file could not be read back after it was found"
            }
            Self::ScanEntryCapReached => {
                "directories skipped: the scan reached its entry ceiling"
            }
            Self::DirectoryDepthExceeded => {
                "directories skipped: nested deeper than the depth limit"
            }
            Self::DiscoveredFileCapReached => {
                "pages omitted: more markdown files exist than the scan collects"
            }
            Self::EdgeCapReached => "links omitted: the link cap was reached (all pages are shown)",
            Self::RawEdgeCapReached => {
                "link candidates dropped before resolution (all pages are shown)"
            }
            Self::EdgeHintTooLong => {
                "link hints ignored: longer than the hint limit, usually prose after a `<- from:` \
                 marker (all pages are shown)"
            }
            Self::TitleTruncated => "page titles shortened for display (all pages are shown)",
            Self::LastUpdatedTruncated => {
                "page dates shortened for display (all pages are shown)"
            }
            Self::ContentTruncated => "preview cut at the size limit",
            Self::DuplicateId => "pages omitted: another page already claimed this ID",
            Self::FolderLinkRefused => {
                "top-level folders skipped: a link or reparse point is not followed out of the \
                 wiki root"
            }
            Self::FolderNameRejected => {
                "top-level folders skipped: the directory name cannot be used as a folder tag"
            }
        }
    }
}

/// One reason, how many items it accounts for, and a few concrete examples.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub(crate) struct KnowledgeOmission {
    pub reason: KnowledgeOmissionReason,
    pub count: usize,
    pub detail: &'static str,
    /// Up to `MAX_OMISSION_EXAMPLES` node IDs (`folder/relative-stem`). These are folder-relative
    /// by construction — an absolute filesystem path can never appear here.
    pub examples: Vec<String>,
}

/// Accumulates omissions during a scan. Insertion-ordered, and the walk itself is
/// deterministic (directory entries are sorted), so the reported order is stable run to run.
#[derive(Debug, Default)]
struct TruncationReport {
    omissions: Vec<KnowledgeOmission>,
}

impl TruncationReport {
    fn record(&mut self, reason: KnowledgeOmissionReason, example: Option<&str>) {
        let slot = match self
            .omissions
            .iter_mut()
            .position(|omission| omission.reason == reason)
        {
            Some(index) => &mut self.omissions[index],
            None => {
                self.omissions.push(KnowledgeOmission {
                    reason,
                    count: 0,
                    detail: reason.detail(),
                    examples: Vec::new(),
                });
                self.omissions
                    .last_mut()
                    .expect("omission was just pushed")
            }
        };
        slot.count = slot.count.saturating_add(1);
        if let Some(example) = example {
            if slot.examples.len() < MAX_OMISSION_EXAMPLES
                && !slot.examples.iter().any(|seen| seen == example)
            {
                slot.examples.push(example.to_string());
            }
        }
    }

    /// Derived, never stored: `truncated` is true exactly when at least one omission was recorded.
    fn truncated(&self) -> bool {
        !self.omissions.is_empty()
    }

    fn into_omissions(self) -> Vec<KnowledgeOmission> {
        self.omissions
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
    /// Derived from `omissions` being non-empty. Retained so existing clients keep working; new
    /// clients should read `omissions` instead, which says *what* and *why*.
    pub truncated: bool,
    pub omissions: Vec<KnowledgeOmission>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub(crate) struct KnowledgePageResponse {
    pub id: String,
    pub title: String,
    pub folder: KnowledgeFolder,
    pub path: String,
    pub content: String,
    pub last_updated: String,
    /// Derived from `omissions`; see `KnowledgeGraphResponse::truncated`.
    pub truncated: bool,
    pub omissions: Vec<KnowledgeOmission>,
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
}

#[derive(Debug)]
struct ParsedSource {
    node: KnowledgeNode,
    raw_edges: Vec<RawEdge>,
    body: String,
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
    /// Folder tags actually present in this graph. With a discovered folder set there is no
    /// compiled-in list to gate explicit-path hints against, so the graph's own contents are the
    /// gate: `foo/bar.md` is only treated as an explicit path when `foo` is a real folder here.
    folders: HashSet<String>,
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
    let (wiki_folders, folder_refusals) = resolve_wiki_folders(&state, &wiki_root).await;
    let project_root = resolve_project_root(&state, query.session_id.as_deref())?;
    let Ok(_permit) = knowledge_scan_limiter().acquire().await else {
        return Ok(Json(KnowledgeGraphResponse::default()));
    };

    let graph = tokio::task::spawn_blocking(move || {
        scan_knowledge(
            &wiki_root,
            &wiki_folders,
            project_root.as_deref(),
            max_nodes,
            &folder_refusals,
        )
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
    // The page response describes *this page*, not the corpus, so discovery refusals are
    // deliberately not replayed here — they are a graph-level fact and ride the graph response.
    let (wiki_folders, _folder_refusals) = resolve_wiki_folders(&state, &wiki_root).await;
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

async fn resolve_wiki_folders(
    state: &AppState,
    wiki_root: &Path,
) -> (Vec<KnowledgeFolder>, Vec<FolderRefusal>) {
    let configured = state.config.read().await.knowledge_wiki_folders.clone();
    let discovered = discover_wiki_folders(wiki_root);
    (
        resolve_wiki_folders_from(configured.as_deref(), discovered.folders),
        discovered.refusals,
    )
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

/// Build a bounded graph from the wiki corpus and the session project's allowlisted documents.
///
/// `wiki_folders` is the effective folder set, already resolved against the folders discovered
/// under `wiki_root` by `resolve_wiki_folders_from`. Callers must never synthesize it from raw
/// operator input — a folder name that never went through `discover_wiki_folders` has not been
/// canonicalized or containment-checked.
/// `folder_refusals` carries the top-level directories `discover_wiki_folders` turned away.
/// Discovery runs in the async handler, before `spawn_blocking`, while the `TruncationReport` lives
/// in here — so its refusals cannot record themselves and have to be replayed. Without this the
/// response claims `truncated=false, omissions=[]` while a whole top-level subtree is missing.
/// Callers with nothing to replay pass `&[]`.
pub(crate) fn scan_knowledge(
    wiki_root: &Path,
    wiki_folders: &[KnowledgeFolder],
    project_root: Option<&Path>,
    max_nodes: usize,
    folder_refusals: &[FolderRefusal],
) -> KnowledgeGraphResponse {
    scan_knowledge_within(
        wiki_root,
        wiki_folders,
        project_root,
        max_nodes,
        folder_refusals,
        MAX_GRAPH_ITEM_BYTES,
    )
}

/// `scan_knowledge` with the node byte budget injected, so a test can make the byte budget bind
/// before the node cap without a multi-megabyte fixture. `build_edges` already takes its budget the
/// same way for the same reason.
fn scan_knowledge_within(
    wiki_root: &Path,
    wiki_folders: &[KnowledgeFolder],
    project_root: Option<&Path>,
    max_nodes: usize,
    folder_refusals: &[FolderRefusal],
    item_byte_budget: usize,
) -> KnowledgeGraphResponse {
    let node_cap = max_nodes.clamp(1, MAX_NODES);
    let mut report = TruncationReport::default();
    // Replayed before the walk so the reasons appear in the order the operator meets them: what was
    // never opened, then what was opened and did not fit.
    for (reason, example) in folder_refusals {
        report.record(*reason, example.as_deref());
    }
    let enumeration = enumerate_sources(wiki_root, wiki_folders, project_root, &mut report);
    let mut nodes = Vec::new();
    let mut raw_edges = Vec::new();
    let mut seen_ids = HashSet::new();
    let mut graph_item_bytes = 0usize;
    let mut remaining_sources = enumeration.sources.into_iter().peekable();
    // Which cap actually stopped the walk. The tail below is attributed to *this*, not to the node
    // cap unconditionally — a byte-budget stop leaves `nodes.len()` strictly under `node_cap`, so
    // blaming the node cap would send the operator to raise a `?max_nodes=` that changes nothing.
    let mut tail_reason = KnowledgeOmissionReason::NodeCapReached;

    while let Some(source) = remaining_sources.next() {
        if nodes.len() >= node_cap {
            report.record(
                KnowledgeOmissionReason::NodeCapReached,
                node_id_for(&source).as_deref(),
            );
            break;
        }
        match read_source(&source, &mut report) {
            SourceRead::Parsed(parsed) => {
                if seen_ids.contains(&parsed.node.id) {
                    // Two sources minted the same ID. Keep the first, but say so: dropping a page
                    // with a bare `continue` is exactly the silent loss the omission taxonomy
                    // exists to eliminate.
                    report.record(
                        KnowledgeOmissionReason::DuplicateId,
                        Some(parsed.node.id.as_str()),
                    );
                    continue;
                }
                let node_bytes = serialized_item_bytes(&parsed.node);
                if graph_item_bytes.saturating_add(node_bytes) > item_byte_budget {
                    report.record(
                        KnowledgeOmissionReason::ResponseSizeCapReached,
                        Some(parsed.node.id.as_str()),
                    );
                    tail_reason = KnowledgeOmissionReason::ResponseSizeCapReached;
                    break;
                }
                graph_item_bytes += node_bytes;
                seen_ids.insert(parsed.node.id.clone());
                nodes.push(parsed.node);
                for edge in parsed.raw_edges {
                    if raw_edges.len() >= MAX_RAW_EDGES {
                        report.record(
                            KnowledgeOmissionReason::RawEdgeCapReached,
                            Some(edge.source.as_str()),
                        );
                        break;
                    }
                    raw_edges.push(edge);
                }
            }
            SourceRead::TooLarge | SourceRead::Unavailable => {}
        }
    }

    // Everything the cap-break above did not even look at is still an omission, and the operator is
    // owed the count. Reporting only the one source we broke on would understate it by the length
    // of the tail.
    for skipped in remaining_sources {
        report.record(tail_reason, node_id_for(&skipped).as_deref());
    }

    let lookup = NodeLookup::from_nodes(&nodes);
    let edge_byte_budget = item_byte_budget.saturating_sub(graph_item_bytes);
    let edges = build_edges(raw_edges, &lookup, edge_byte_budget, &mut report);

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
        truncated: report.truncated(),
        omissions: report.into_omissions(),
    };
    debug_assert!(
        serde_json::to_vec(&response)
            .map(|serialized| serialized.len() <= MAX_GRAPH_RESPONSE_BYTES)
            .unwrap_or(false),
        "knowledge graph response exceeded its serialized size cap"
    );
    response
}

/// Resolve a page ID against the enumerated corpus and return its bounded Markdown preview.
pub(crate) fn preview_knowledge_page(
    wiki_root: &Path,
    wiki_folders: &[KnowledgeFolder],
    project_root: Option<&Path>,
    id: &str,
) -> Option<KnowledgePageResponse> {
    if validate_knowledge_id(id).is_err() {
        return None;
    }

    // The ID is *matched against* enumerated sources, never joined onto a path. Every candidate has
    // already been canonicalized and containment-checked under its folder root, so a syntactically
    // valid but hostile ID simply matches nothing.
    let mut report = TruncationReport::default();
    let source = enumerate_sources(wiki_root, wiki_folders, project_root, &mut report)
        .sources
        .into_iter()
        .find(|source| node_id_for(source).as_deref() == Some(id))?;
    // Restart the report so the page response describes *this page*, not the enumeration of the
    // whole corpus that was needed to find it.
    let mut report = TruncationReport::default();
    let SourceRead::Parsed(parsed) = read_source(&source, &mut report) else {
        return None;
    };
    let (content, content_truncated) = truncate_utf8(parsed.body, MAX_PREVIEW_BYTES);
    if content_truncated {
        report.record(
            KnowledgeOmissionReason::ContentTruncated,
            Some(parsed.node.id.as_str()),
        );
    }

    Some(KnowledgePageResponse {
        id: parsed.node.id,
        title: parsed.node.title,
        folder: parsed.node.folder,
        path: parsed.node.path,
        content,
        last_updated: parsed.node.last_updated,
        truncated: report.truncated(),
        omissions: report.into_omissions(),
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

    // Shape-only validation. The folder set is discovered from disk, so there is no compiled-in
    // list to check the prefix against, and checking against the *live* set here would be worse
    // than useless: it would cost a filesystem walk before rejecting garbage, and it would make the
    // 400-vs-404 status leak which folders exist. Containment is enforced where it is real — in
    // `preview_knowledge_page`, which matches this ID against already-canonicalized sources rather
    // than joining it onto a path.
    let components: Vec<&str> = id.split('/').collect();
    if components.len() < 2
        || components
            .iter()
            .any(|component| component.is_empty() || *component == "." || *component == "..")
    {
        return Err(ApiError::bad_request("Invalid knowledge page ID"));
    }
    let Some(prefix) = components.first() else {
        return Err(ApiError::bad_request("Invalid knowledge page ID"));
    };
    // The reserved session prefix is checked *before* the shape gate: `.project` deliberately fails
    // `is_valid_folder_name` (that is what makes it unforgeable by discovery), so gating first
    // would reject the session's own pages. It stays a closed two-file allowlist because it is
    // session-scoped and points outside the wiki root — the one prefix that must not become a
    // general subtree.
    if *prefix == KnowledgeFolder::PROJECT {
        return if matches!(id, ".project/project-dna" | ".project/bug-patterns") {
            Ok(())
        } else {
            Err(ApiError::bad_request("Invalid knowledge page ID"))
        };
    }
    if !is_valid_folder_name(prefix) {
        return Err(ApiError::bad_request("Invalid knowledge page ID"));
    }
    Ok(())
}

fn enumerate_sources(
    wiki_root: &Path,
    wiki_folders: &[KnowledgeFolder],
    project_root: Option<&Path>,
    report: &mut TruncationReport,
) -> SourceEnumeration {
    let mut enumeration = SourceEnumeration::default();
    let mut entries_seen = 0;
    let canonical_wiki_root = fs::canonicalize(wiki_root).ok();

    for folder in wiki_folders {
        // `root` is overloaded: it is the synthetic bucket for loose root-level markdown AND a
        // perfectly legal directory name. Both must be walked under the `root/` tag, or a real
        // `<wiki>/root/` has every page dropped — silently, since discovery already emitted the
        // folder. The synthetic walk is non-recursive so it collects only the loose files that
        // belong to no folder; the real directory is walked recursively beside it.
        //
        // The synthetic walk starts from the *canonical* root, not the configured path. If the
        // operator points the wiki at a junction or symlink (`~/.ai-docs/wiki` -> a checkout
        // elsewhere is a normal setup for a synced repo), lstat on the configured path reports
        // `is_dir=false, is_symlink=true` and the link guard below would drop this whole bucket.
        // The root *is* the containment boundary, so resolving it cannot widen the scan. Sibling
        // folders never hit this because `wiki_root.join(name)` ends in a real directory component.
        let subtrees: Vec<(PathBuf, bool)> = if folder.is_wiki_root() {
            let resolved_root = canonical_wiki_root
                .clone()
                .unwrap_or_else(|| wiki_root.to_path_buf());
            let real_root_dir = resolved_root.join(KnowledgeFolder::ROOT);
            let mut walks = vec![(resolved_root, false)];
            if real_root_dir.is_dir() {
                walks.push((real_root_dir, true));
            }
            walks
        } else {
            // Every other folder is `wiki_root` joined with a name that `discover_wiki_folders`
            // already canonicalized and proved to live under the root; a name that never went
            // through discovery cannot get here.
            vec![(wiki_root.join(folder.as_str()), true)]
        };

        for (subtree, recurse) in subtrees {
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
                report.record(KnowledgeOmissionReason::DiscoveredFileCapReached, None);
                break;
            }
            let mut walk = DirectoryWalk {
                canonical_root: canonical_root.clone(),
                file_limit: remaining,
                recurse,
                entries_seen: &mut entries_seen,
                output: Vec::new(),
            };
            walk.visit(&canonical_root, 0, report);
            let found = walk.output;
            enumeration
                .sources
                .extend(found.into_iter().map(|path| SourceFile {
                    folder: folder.clone(),
                    path,
                    root: canonical_root.clone(),
                }));
        }
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
                folder: KnowledgeFolder::project(),
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
        .sort_by_key(|source| !source.folder.is_project());

    enumeration
}

/// Bounded, containment-checked markdown walk of one folder root.
///
/// There is deliberately no filename denylist here any more. The old `is_forbidden_name` hid
/// `index.md`, `log.md`, `log-archive*.md`, `learnings.jsonl`, `.env`, and `agent-os.db*` — but
/// this is the operator's own memory system on their own machine, and hiding their index page from
/// them was never a security control. Three of those entries were never reachable in the first
/// place: `has_markdown_extension` already admits only `*.md`, so `learnings.jsonl`, `.env`, and
/// `agent-os.db{,-shm,-wal}` cannot be enumerated regardless — and `.env` is additionally excluded
/// by the dot-prefix skip below. Removing the list therefore only un-hides markdown.
struct DirectoryWalk<'a> {
    canonical_root: PathBuf,
    file_limit: usize,
    /// The synthetic `root` bucket sets this false so it collects only the wiki root's own loose
    /// files; recursing would re-collect every folder's pages under a second ID prefix.
    recurse: bool,
    entries_seen: &'a mut usize,
    output: Vec<PathBuf>,
}

impl DirectoryWalk<'_> {
    fn visit(&mut self, directory: &Path, depth: usize, report: &mut TruncationReport) {
        if depth > MAX_DIRECTORY_DEPTH {
            report.record(
                KnowledgeOmissionReason::DirectoryDepthExceeded,
                self.relative_label(directory).as_deref(),
            );
            return;
        }
        if *self.entries_seen >= MAX_SCAN_ENTRIES {
            report.record(
                KnowledgeOmissionReason::ScanEntryCapReached,
                self.relative_label(directory).as_deref(),
            );
            return;
        }
        if self.output.len() >= self.file_limit {
            report.record(
                KnowledgeOmissionReason::DiscoveredFileCapReached,
                self.relative_label(directory).as_deref(),
            );
            return;
        }
        let Ok(read_dir) = fs::read_dir(directory) else {
            return;
        };
        // Bound collection itself before sorting. `ReadDir` is lazy, but collecting it wholesale
        // would otherwise let one enormous directory bypass MAX_SCAN_ENTRIES before the loop runs.
        let remaining_entries = MAX_SCAN_ENTRIES.saturating_sub(*self.entries_seen);
        let mut enumerated: Vec<_> = read_dir.take(remaining_entries.saturating_add(1)).collect();
        if enumerated.len() > remaining_entries {
            enumerated.truncate(remaining_entries);
            report.record(
                KnowledgeOmissionReason::ScanEntryCapReached,
                self.relative_label(directory).as_deref(),
            );
        }
        // Count raw iterator results, including errors, so repeated ReadDir failures cannot evade
        // the global traversal-work ceiling.
        *self.entries_seen += enumerated.len();
        let mut entries: Vec<_> = enumerated.into_iter().filter_map(Result::ok).collect();
        entries.sort_by_key(|entry| entry.file_name());

        for entry in entries {
            if self.output.len() >= self.file_limit {
                report.record(
                    KnowledgeOmissionReason::DiscoveredFileCapReached,
                    self.relative_label(directory).as_deref(),
                );
                return;
            }

            let raw_name = entry.file_name();
            if raw_name.to_string_lossy().starts_with('.') {
                continue;
            }
            let Ok(file_type) = entry.file_type() else {
                continue;
            };
            // `read_dir`'s file_type is lstat-based, so a symlink is refused here without ever
            // being followed — the check that keeps a link pointing outside the root from
            // smuggling content in.
            if file_type.is_symlink() {
                continue;
            }
            let Ok(canonical_path) = fs::canonicalize(entry.path()) else {
                continue;
            };
            // Second line of defence for reparse points/junctions, which are not reported as
            // symlinks on every platform: the resolved path must still be inside the folder root.
            if !canonical_path.starts_with(&self.canonical_root) {
                continue;
            }

            if file_type.is_dir() {
                if !self.recurse {
                    continue;
                }
                self.visit(&canonical_path, depth + 1, report);
                if self.output.len() >= self.file_limit {
                    return;
                }
            } else if file_type.is_file() && has_markdown_extension(&canonical_path) {
                self.output.push(canonical_path);
            }
        }
    }

    /// A root-relative label for omission examples. Returns `None` rather than ever falling back to
    /// the absolute path, so an absolute filesystem path cannot reach the API through a report.
    fn relative_label(&self, directory: &Path) -> Option<String> {
        let relative = directory.strip_prefix(&self.canonical_root).ok()?;
        let mut components = Vec::new();
        for component in relative.iter() {
            components.push(component.to_str()?);
        }
        Some(if components.is_empty() {
            ".".to_string()
        } else {
            components.join("/")
        })
    }
}

fn has_markdown_extension(path: &Path) -> bool {
    path.extension()
        .and_then(|extension| extension.to_str())
        .is_some_and(|extension| extension.eq_ignore_ascii_case("md"))
}

fn read_source(source: &SourceFile, report: &mut TruncationReport) -> SourceRead {
    let unavailable = |report: &mut TruncationReport| {
        report.record(
            KnowledgeOmissionReason::FileUnreadable,
            node_id_for(source).as_deref(),
        );
        SourceRead::Unavailable
    };
    let too_large = |report: &mut TruncationReport| {
        report.record(
            KnowledgeOmissionReason::FileTooLarge,
            node_id_for(source).as_deref(),
        );
        SourceRead::TooLarge
    };

    let Ok(source_metadata) = fs::symlink_metadata(&source.path) else {
        return unavailable(report);
    };
    if !source_metadata.is_file() || source_metadata.file_type().is_symlink() {
        return unavailable(report);
    }
    let Ok(canonical_path) = fs::canonicalize(&source.path) else {
        return unavailable(report);
    };
    if !canonical_path.starts_with(&source.root) {
        return unavailable(report);
    }
    let Ok(target_metadata) = fs::symlink_metadata(&canonical_path) else {
        return unavailable(report);
    };
    if !target_metadata.is_file()
        || target_metadata.file_type().is_symlink()
        || !validated_path_matches(&source_metadata, &target_metadata)
    {
        return unavailable(report);
    }

    let Some((mut file, open_metadata)) = open_file_if_unchanged(&canonical_path, &target_metadata)
    else {
        return unavailable(report);
    };
    if open_metadata.len() > MAX_FILE_BYTES as u64 {
        return too_large(report);
    }

    let mut bytes = Vec::with_capacity(open_metadata.len() as usize);
    if file
        .by_ref()
        .take((MAX_FILE_BYTES + 1) as u64)
        .read_to_end(&mut bytes)
        .is_err()
    {
        return unavailable(report);
    }
    if bytes.len() > MAX_FILE_BYTES {
        return too_large(report);
    }
    let Ok(text) = String::from_utf8(bytes) else {
        return unavailable(report);
    };
    let Some(id) = node_id_for(source) else {
        return unavailable(report);
    };
    if id.len() > 512 {
        return unavailable(report);
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
    if title_truncated {
        report.record(KnowledgeOmissionReason::TitleTruncated, Some(id.as_str()));
    }
    let last_updated = frontmatter_field(frontmatter, "last_updated").unwrap_or_default();
    let (last_updated, last_updated_truncated) =
        truncate_utf8(last_updated, MAX_LAST_UPDATED_BYTES);
    if last_updated_truncated {
        report.record(
            KnowledgeOmissionReason::LastUpdatedTruncated,
            Some(id.as_str()),
        );
    }
    let node = KnowledgeNode {
        path: format!("{id}.md"),
        id: id.clone(),
        title,
        folder: source.folder.clone(),
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
    for _ in raw_edges.len()..original_edge_count {
        report.record(KnowledgeOmissionReason::EdgeHintTooLong, Some(id.as_str()));
    }

    SourceRead::Parsed(ParsedSource {
        node,
        raw_edges,
        body: body.to_string(),
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
            lookup
                .folders
                .insert(normalize_lookup(node.folder.as_str()));
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
    slug = normalize_parent_relative_hint(slug, lookup)?;
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
        if !lookup.folders.contains(&normalize_lookup(prefix)) {
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

/// Corpus pages sometimes refer to a sibling subtree with `../patterns/foo.md`. These strings are
/// lookup hints only (never filesystem paths), but parent components are still normalized narrowly:
/// after removing leading parents, the hint must name a folder that actually exists in this graph.
///
/// The gate used to be the compiled-in folder list; it is now the graph's own folder set, which is
/// the same guarantee expressed against the discovered corpus. `project` stays excluded by name: it
/// is session-scoped and rooted outside the wiki, so a global-wiki page must not be able to reach
/// it by walking upward.
fn normalize_parent_relative_hint(mut slug: String, lookup: &NodeLookup) -> Option<String> {
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
        || prefix == KnowledgeFolder::PROJECT
        || !lookup.folders.contains(&normalize_lookup(prefix))
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
    report: &mut TruncationReport,
) -> Vec<KnowledgeEdge> {
    let mut edges = Vec::new();
    let mut seen = HashSet::new();
    let mut item_bytes = 0usize;

    for raw in raw_edges {
        let Some(target) = resolve_hint(&raw.target_hint, lookup) else {
            // An unresolvable hint is not an omission: prose annotations and links to pages that do
            // not exist are normal corpus content, and reporting them would put the banner straight
            // back to meaning nothing.
            continue;
        };
        if target == raw.source {
            continue;
        }
        // Deduplicate *before* the caps so a dropped edge is counted once no matter how many times
        // the corpus repeats it. Over-counting duplicates would inflate the number the banner
        // shows the operator.
        if !seen.insert((raw.source.clone(), target.clone(), raw.kind)) {
            continue;
        }
        if edges.len() >= MAX_EDGES {
            report.record(
                KnowledgeOmissionReason::EdgeCapReached,
                Some(raw.source.as_str()),
            );
            continue;
        }
        let edge = KnowledgeEdge {
            source: raw.source,
            target,
            kind: raw.kind,
        };
        let edge_bytes = serialized_item_bytes(&edge);
        if item_bytes.saturating_add(edge_bytes) > max_item_bytes {
            report.record(
                KnowledgeOmissionReason::ResponseSizeCapReached,
                Some(edge.source.as_str()),
            );
            continue;
        }
        item_bytes += edge_bytes;
        edges.push(edge);
    }
    edges
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

    /// The effective folder set exactly as the HTTP handler builds it: discovered from disk, with
    /// no operator narrowing. Tests must go through this rather than hand-rolling a folder list, or
    /// they would stop exercising discovery.
    fn discovered(root: &Path) -> Vec<KnowledgeFolder> {
        resolve_wiki_folders_from(None, discover_wiki_folders(root).folders)
    }

    fn scan(root: &Path) -> KnowledgeGraphResponse {
        scan_knowledge(root, &discovered(root), None, MAX_NODES, &[])
    }

    /// The full handler path: discovery's refusals are replayed into the scan's report, exactly as
    /// `get_knowledge_graph` does. `scan` above drops them, so a test that needs to see a refused
    /// top-level folder in `omissions` must use this.
    fn scan_reporting_refusals(root: &Path) -> KnowledgeGraphResponse {
        let discovery = discover_wiki_folders(root);
        let folders = resolve_wiki_folders_from(None, discovery.folders);
        scan_knowledge(root, &folders, None, MAX_NODES, &discovery.refusals)
    }

    fn ids(graph: &KnowledgeGraphResponse) -> HashSet<&str> {
        graph.nodes.iter().map(|node| node.id.as_str()).collect()
    }

    fn omission(
        graph: &KnowledgeGraphResponse,
        reason: KnowledgeOmissionReason,
    ) -> Option<&KnowledgeOmission> {
        graph
            .omissions
            .iter()
            .find(|omission| omission.reason == reason)
    }

    fn reasons(graph: &KnowledgeGraphResponse) -> Vec<KnowledgeOmissionReason> {
        graph
            .omissions
            .iter()
            .map(|omission| omission.reason)
            .collect()
    }

    // ---------------------------------------------------------------------------------------
    // 1. Every folder is shown.
    // ---------------------------------------------------------------------------------------

    /// The headline behaviour change: the folder set is discovered from the wiki root, so folders
    /// that no compiled-in enum ever listed produce nodes. `agents/` is the operator's real case —
    /// 63 of their 163 pages — and the arbitrary names beside it prove nothing was swapped for a
    /// longer hardcoded list.
    #[test]
    fn folders_absent_from_every_former_enum_still_yield_nodes() {
        let wiki = TempDir::new().unwrap();
        write_page(wiki.path(), "patterns/known.md", "# Known\n");
        write_page(wiki.path(), "agents/recruiting/candidate.md", "# Candidate\n");
        write_page(wiki.path(), "zettelkasten/202607190001.md", "# Zettel\n");
        write_page(wiki.path(), "Field Notes/site-visit.md", "# Site Visit\n");
        write_page(wiki.path(), "meta/deep/nested/leaf.md", "# Leaf\n");

        let graph = scan(wiki.path());
        let seen = ids(&graph);
        for expected in [
            "patterns/known",
            "agents/recruiting/candidate",
            "zettelkasten/202607190001",
            "Field Notes/site-visit",
            "meta/deep/nested/leaf",
        ] {
            assert!(seen.contains(expected), "missing {expected} in {seen:?}");
        }

        // The folder tag on the wire is the directory name itself, so the frontend filter and the
        // node ID prefix cannot drift apart.
        let candidate = graph
            .nodes
            .iter()
            .find(|node| node.id == "agents/recruiting/candidate")
            .expect("agents node");
        assert_eq!(candidate.folder.as_str(), "agents");
        assert_eq!(
            serde_json::to_string(&candidate.folder).unwrap(),
            "\"agents\""
        );
        assert_eq!(candidate.path, "agents/recruiting/candidate.md");
    }

    /// Dot-directories are editor/VCS state, not memory. `.obsidian` and `.git` stay out; nothing
    /// else does.
    #[test]
    fn dot_directories_are_skipped_but_ordinary_ones_are_not() {
        let wiki = TempDir::new().unwrap();
        write_page(wiki.path(), ".obsidian/workspace.md", "# Editor State\n");
        write_page(wiki.path(), ".git/description.md", "# VCS State\n");
        write_page(wiki.path(), "patterns/.hidden/secret.md", "# Nested Hidden\n");
        write_page(wiki.path(), "patterns/kept.md", "# Kept\n");
        write_page(wiki.path(), "obsidian-notes/kept.md", "# Not A Dot Dir\n");

        let graph = scan(wiki.path());
        let seen = ids(&graph);
        assert!(seen.contains("patterns/kept"));
        assert!(
            seen.contains("obsidian-notes/kept"),
            "a folder that merely resembles a dot-dir must still be scanned"
        );
        for hidden in ["Editor State", "VCS State", "Nested Hidden"] {
            assert!(
                !serde_json::to_string(&graph).unwrap().contains(hidden),
                "{hidden} leaked from a dot-directory"
            );
        }
        assert!(!discover_wiki_folders(wiki.path())
            .folders
            .iter()
            .any(|folder| folder.as_str().starts_with('.')));
    }

    /// `is_forbidden_name` used to hide these from the operator's own memory system. They are
    /// ordinary pages and must now appear — including the root-level `index.md`, which was doubly
    /// unreachable because it sits in no folder at all.
    #[test]
    fn previously_forbidden_filenames_and_root_level_pages_now_appear() {
        let wiki = TempDir::new().unwrap();
        write_page(wiki.path(), "index.md", "# Wiki Index\n");
        write_page(wiki.path(), "schema.md", "# Schema\n");
        write_page(wiki.path(), "log.md", "# Running Log\n");
        write_page(wiki.path(), "patterns/index.md", "# Patterns Index\n");
        write_page(wiki.path(), "patterns/log.md", "# Patterns Log\n");
        write_page(
            wiki.path(),
            "patterns/log-archive-2026Q2.md",
            "# Archived Log\n",
        );

        let graph = scan(wiki.path());
        let seen = ids(&graph);
        for expected in [
            "root/index",
            "root/schema",
            "root/log",
            "patterns/index",
            "patterns/log",
            "patterns/log-archive-2026Q2",
        ] {
            assert!(seen.contains(expected), "missing {expected} in {seen:?}");
        }

        // ...and they are previewable, not just countable.
        let page = preview_knowledge_page(
            wiki.path(),
            &discovered(wiki.path()),
            None,
            "root/index",
        )
        .expect("root index preview");
        assert_eq!(page.path, "root/index.md");
        assert_eq!(page.folder.as_str(), "root");
        assert!(page.content.contains("Wiki Index"));
    }

    /// The synthetic `root` bucket is non-recursive, so root-level pages never duplicate the pages
    /// of the folders beside them under a second ID prefix.
    #[test]
    fn root_bucket_does_not_re_collect_the_folders_beside_it() {
        let wiki = TempDir::new().unwrap();
        write_page(wiki.path(), "index.md", "# Index\n");
        write_page(wiki.path(), "patterns/one.md", "# One\n");
        write_page(wiki.path(), "patterns/nested/two.md", "# Two\n");

        let graph = scan(wiki.path());
        let seen = ids(&graph);
        assert_eq!(
            seen,
            HashSet::from(["root/index", "patterns/one", "patterns/nested/two"])
        );
        assert!(!seen.iter().any(|id| id.starts_with("root/patterns")));
    }


    /// A wiki root with no loose markdown gets no synthetic bucket at all.
    #[test]
    fn root_bucket_appears_only_when_loose_markdown_exists() {
        let wiki = TempDir::new().unwrap();
        write_page(wiki.path(), "patterns/one.md", "# One\n");
        assert!(!discover_wiki_folders(wiki.path())
            .folders
            .iter()
            .any(KnowledgeFolder::is_wiki_root));

        write_page(wiki.path(), "loose.md", "# Loose\n");
        assert!(discover_wiki_folders(wiki.path())
            .folders
            .iter()
            .any(KnowledgeFolder::is_wiki_root));
    }

    /// `root` is overloaded: the synthetic loose-file bucket *and* a legal directory name. Both
    /// walks must run under the `root/` tag. This used to route a real `<wiki>/root/` to the
    /// synthetic non-recursive walk of the wiki root, so every page inside it vanished — with
    /// `truncated=false` and `omissions=[]`, the silent loss the taxonomy exists to prevent.
    ///
    /// Note `root_bucket_appears_only_when_loose_markdown_exists` asserts only on the folder
    /// *list* and passes either way; it is not coverage for this.
    #[test]
    fn a_real_directory_named_root_still_yields_its_pages() {
        let wiki = TempDir::new().unwrap();
        write_page(wiki.path(), "root/archive.md", "# Archive\n");
        write_page(wiki.path(), "root/deep/nested.md", "# Nested\n");
        write_page(wiki.path(), "index.md", "# Index\n");
        write_page(wiki.path(), "patterns/one.md", "# One\n");

        let graph = scan(wiki.path());
        assert_eq!(
            ids(&graph),
            HashSet::from([
                "root/archive",
                "root/deep/nested",
                "root/index",
                "patterns/one"
            ]),
            "a real root/ directory and the loose root markdown must both be served"
        );
        assert!(
            graph.omissions.is_empty(),
            "nothing was dropped, so nothing may be reported: {:?}",
            graph.omissions
        );

        // The page endpoint resolves against the same enumeration, so it must agree with the graph.
        let folders = discovered(wiki.path());
        for id in ["root/archive", "root/deep/nested", "root/index"] {
            assert!(
                preview_knowledge_page(wiki.path(), &folders, None, id).is_some(),
                "the graph advertises {id}, so the preview endpoint must serve it"
            );
        }
    }

    /// The strictly worse variant: with no loose markdown the synthetic bucket is never inserted,
    /// discovery still emits the `root` tag for the real directory, and the non-recursive walk of
    /// the wiki root yields nothing — so the whole subtree collapsed to zero pages.
    #[test]
    fn a_real_root_directory_is_served_without_any_loose_markdown() {
        let wiki = TempDir::new().unwrap();
        write_page(wiki.path(), "root/archive.md", "# Archive\n");
        write_page(wiki.path(), "root/deep/nested.md", "# Nested\n");
        write_page(wiki.path(), "patterns/one.md", "# One\n");

        let graph = scan(wiki.path());
        assert_eq!(
            ids(&graph),
            HashSet::from(["root/archive", "root/deep/nested", "patterns/one"]),
            "a real root/ directory must be walked even with no loose markdown beside it"
        );
        assert!(graph.omissions.is_empty(), "{:?}", graph.omissions);
    }

    /// Serving both walks under one tag makes an ID collision possible: `<wiki>/index.md` and
    /// `<wiki>/root/index.md` both mint `root/index`. One page must win, but the loser must be
    /// *named*, never dropped with a bare `continue`.
    #[test]
    fn a_root_id_collision_is_reported_rather_than_dropped_in_silence() {
        let wiki = TempDir::new().unwrap();
        write_page(wiki.path(), "index.md", "# Loose Index\n");
        write_page(wiki.path(), "root/index.md", "# Real Directory Index\n");
        write_page(wiki.path(), "root/unique.md", "# Unique\n");

        let graph = scan(wiki.path());
        assert!(
            ids(&graph).contains("root/unique"),
            "the non-colliding page must still be served"
        );
        assert_eq!(
            graph.nodes.iter().filter(|n| n.id == "root/index").count(),
            1,
            "exactly one page may hold a given ID"
        );
        let duplicate = omission(&graph, KnowledgeOmissionReason::DuplicateId)
            .expect("the dropped page must be reported, not silently discarded");
        assert_eq!(duplicate.count, 1);
        assert_eq!(duplicate.examples, vec!["root/index".to_string()]);
        assert!(graph.truncated, "a dropped page must not report truncated=false");
    }

    /// The removed denylist also named `.env`, `learnings.jsonl`, and `agent-os.db*`. Those were
    /// never reachable: the walk only ever collects `*.md`. This pins that, so removing the
    /// denylist cannot be mistaken for having exposed them.
    #[test]
    fn only_markdown_is_ever_enumerated() {
        let wiki = TempDir::new().unwrap();
        write_page(wiki.path(), "patterns/kept.md", "# Kept\n");
        for non_markdown in [
            "patterns/.env",
            "patterns/agent-os.db",
            "patterns/agent-os.db-shm",
            "patterns/agent-os.db-wal",
            "patterns/learnings.jsonl",
            "patterns/notes.txt",
            "patterns/config.yaml",
        ] {
            write_page(wiki.path(), non_markdown, "SECRET_TOKEN=hunter2\n");
        }

        let graph = scan(wiki.path());
        assert_eq!(ids(&graph), HashSet::from(["patterns/kept"]));
        let serialized = serde_json::to_string(&graph).unwrap();
        for leaked in ["SECRET_TOKEN", "agent-os", "learnings", ".env"] {
            assert!(!serialized.contains(leaked), "{leaked} leaked into the graph");
        }
    }

    // ---------------------------------------------------------------------------------------
    // 2. Containment still holds.
    // ---------------------------------------------------------------------------------------

    /// Discovery moved the trust boundary to the wiki root; it did not remove it. A symlinked
    /// top-level folder pointing outside the root must not become a discovered folder, and a
    /// symlinked file inside a real folder must not become a page.
    #[cfg(unix)]
    #[test]
    fn symlinks_escaping_the_root_are_refused_at_every_level() {
        use std::os::unix::fs::symlink;

        let wiki = TempDir::new().unwrap();
        let private = TempDir::new().unwrap();
        write_page(private.path(), "secret.md", "# Private Elsewhere\n");
        write_page(wiki.path(), "patterns/kept.md", "# Kept\n");

        // A whole top-level folder aimed outside the root.
        symlink(private.path(), wiki.path().join("escaped")).unwrap();
        // A single file aimed outside the root, inside a genuine folder.
        symlink(
            private.path().join("secret.md"),
            wiki.path().join("patterns/linked.md"),
        )
        .unwrap();

        assert!(
            !discover_wiki_folders(wiki.path())
                .folders
                .iter()
                .any(|folder| folder.as_str() == "escaped"),
            "a symlinked top-level folder must not be discovered"
        );

        let graph = scan(wiki.path());
        assert_eq!(ids(&graph), HashSet::from(["patterns/kept"]));
        assert!(!serde_json::to_string(&graph)
            .unwrap()
            .contains("Private Elsewhere"));
        assert!(preview_knowledge_page(
            wiki.path(),
            &discovered(wiki.path()),
            None,
            "patterns/linked"
        )
        .is_none());
    }

    /// Create an NTFS directory junction, which needs no elevation (unlike `symlink_dir`) and so
    /// can run on the `windows-latest` CI runner. Panics rather than returning an error: a
    /// containment test that silently skips itself is worse than no test at all.
    #[cfg(windows)]
    fn create_junction(link: &Path, target: &Path) {
        let status = std::process::Command::new("cmd")
            .args(["/C", "mklink", "/J"])
            .arg(link)
            .arg(target)
            .stdout(std::process::Stdio::null())
            .status()
            .expect("failed to invoke mklink");
        assert!(status.success(), "mklink /J failed for {link:?} -> {target:?}");
        assert!(
            fs::symlink_metadata(link)
                .expect("junction metadata")
                .file_type()
                .is_symlink()
                || fs::canonicalize(link).expect("junction canonicalizes")
                    != link.to_path_buf(),
            "the junction fixture is inert — it is neither a symlink nor a redirect"
        );
    }

    /// Windows equivalent of the symlink tests below. This matters more than it looks: the
    /// symlink coverage in this file is `#[cfg(unix)]`, and this project's CI runs on
    /// `windows-latest`, so without this test the containment checks are exercised on no runner at
    /// all — deleting them outright kept the whole suite green.
    ///
    /// Which line of defence this pins, measured rather than assumed: `read_dir`'s `file_type` is
    /// lstat-based and reports `is_symlink() == true` for an NTFS directory junction, so the
    /// fixture is caught at the *first* check. (An earlier version of this comment claimed it
    /// pinned the post-canonicalization `starts_with` check; it does not reach it.) That second
    /// check remains as defence in depth for reparse kinds lstat does not tag.
    #[cfg(windows)]
    #[test]
    fn junctions_pointing_outside_the_root_are_refused_on_windows() {
        let wiki = TempDir::new().unwrap();
        let private = TempDir::new().unwrap();
        write_page(private.path(), "secret.md", "# Private Elsewhere\n");
        write_page(wiki.path(), "patterns/kept.md", "# Kept\n");

        // A whole top-level folder aimed outside the root...
        create_junction(&wiki.path().join("escaped"), private.path());
        // ...and a nested directory inside a genuine folder aimed outside the root. Join
        // component-wise: `mklink` rejects a path with mixed separators.
        create_junction(&wiki.path().join("patterns").join("nested"), private.path());

        assert!(
            !discover_wiki_folders(wiki.path())
                .folders
                .iter()
                .any(|folder| folder.as_str() == "escaped"),
            "a junction aimed outside the root must not become a discovered folder"
        );

        let graph = scan(wiki.path());
        assert_eq!(ids(&graph), HashSet::from(["patterns/kept"]));
        assert!(
            !serde_json::to_string(&graph)
                .unwrap()
                .contains("Private Elsewhere"),
            "content from outside the wiki root leaked into the graph"
        );
        assert!(preview_knowledge_page(
            wiki.path(),
            &discovered(wiki.path()),
            None,
            "patterns/nested/secret"
        )
        .is_none());
        assert!(
            preview_knowledge_page(wiki.path(), &discovered(wiki.path()), None, "escaped/secret")
                .is_none()
        );

        // Refusing the folder is correct; refusing it *silently* is not. Discovery used to have no
        // reporting channel at all, so an entire top-level subtree vanished under
        // `truncated=false, omissions=[]` — invisible even in the UI folder filter, which is
        // derived from node folders.
        let reported = scan_reporting_refusals(wiki.path());
        assert!(
            reported.truncated,
            "a refused top-level folder must not report truncated=false"
        );
        let refused = omission(&reported, KnowledgeOmissionReason::FolderLinkRefused)
            .expect("the refused junction must be reported");
        assert_eq!(refused.count, 1);
        assert_eq!(refused.examples, vec!["escaped".to_string()]);
    }

    /// A wiki folder literally named `project` is the operator's own memory and must work like any
    /// other folder. It used to collide with the reserved session tag, with three consequences:
    /// its pages were advertised in the graph but answered 400 by the preview endpoint (the closed
    /// two-file allowlist); a `<wiki>/project/project-dna.md` silently displaced the session's own
    /// `.ai-docs/project-dna.md` via the `seen_ids` dedup; and wiki pages were hoisted ahead of the
    /// session allowlist in the cap sort. Only the conjunction is load-bearing, so assert all four.
    #[test]
    fn a_wiki_folder_named_project_coexists_with_session_project_knowledge() {
        let wiki = TempDir::new().unwrap();
        let project = TempDir::new().unwrap();
        write_page(wiki.path(), "project/project-dna.md", "# WIKI DNA\n");
        write_page(wiki.path(), "project/notes.md", "# WIKI NOTES\n");
        write_page(wiki.path(), "patterns/known.md", "# Known\n");
        write_page(project.path(), ".ai-docs/project-dna.md", "# SESSION DNA\n");

        let folders = discovered(wiki.path());
        let graph = scan_knowledge(
            wiki.path(),
            &folders,
            Some(project.path()),
            MAX_NODES,
            &[],
        );
        let node_ids = ids(&graph);

        // 1. Both namespaces are present; neither displaces the other.
        assert!(
            node_ids.contains("project/project-dna"),
            "the wiki's own project page must be served: {node_ids:?}"
        );
        assert!(
            node_ids.contains(".project/project-dna"),
            "the session's project knowledge must survive a wiki `project/` folder: {node_ids:?}"
        );
        assert!(node_ids.contains("project/notes"));

        // 2. Nothing the graph advertises may be rejected by the preview endpoint.
        assert!(
            validate_knowledge_id("project/notes").is_ok(),
            "a node the graph serves must not be answered with 400"
        );
        let wiki_notes = preview_knowledge_page(
            wiki.path(),
            &folders,
            Some(project.path()),
            "project/notes",
        )
        .expect("the wiki project page must be previewable");
        assert!(wiki_notes.content.contains("WIKI NOTES"));

        // 3. The two same-named pages return their own distinct bodies.
        let wiki_dna = preview_knowledge_page(
            wiki.path(),
            &folders,
            Some(project.path()),
            "project/project-dna",
        )
        .expect("wiki project-dna");
        let session_dna = preview_knowledge_page(
            wiki.path(),
            &folders,
            Some(project.path()),
            ".project/project-dna",
        )
        .expect("session project-dna");
        assert!(wiki_dna.content.contains("WIKI DNA"), "{:?}", wiki_dna.content);
        assert!(
            session_dna.content.contains("SESSION DNA"),
            "the session's own DNA must not be shadowed by the wiki copy: {:?}",
            session_dna.content
        );

        // 4. Nothing was dropped, so nothing may be reported.
        assert!(
            graph.omissions.is_empty(),
            "no page was lost, so omissions must be empty: {:?}",
            graph.omissions
        );
        assert!(!graph.truncated);
    }

    /// Reaching the wiki root *through* a junction must not cost the operator the root bucket.
    /// Junctioning `~/.ai-docs/wiki` at a synced checkout elsewhere is a normal setup, and the
    /// synthetic bucket used to walk the configured path — on which lstat reports
    /// `is_dir=false, is_symlink=true`, so the link guard dropped the whole bucket. Silently:
    /// discovery canonicalizes first and so still emitted the `root` tag.
    ///
    /// The control scan of the same fixture through the real path is what makes this load-bearing
    /// — it fails if the fix is reverted AND if the walk itself regresses.
    #[cfg(windows)]
    #[test]
    fn a_junctioned_wiki_root_still_serves_its_loose_markdown() {
        let base = TempDir::new().unwrap();
        let real = base.path().join("real");
        fs::create_dir_all(&real).unwrap();
        write_page(&real, "index.md", "# Loose Index\n");
        write_page(&real, "patterns/known.md", "# Known\n");

        let link = base.path().join("link");
        create_junction(&link, &real);

        let through_link = scan(&link);
        let control = scan(&real);
        assert_eq!(
            ids(&through_link),
            ids(&control),
            "a junctioned wiki root must serve exactly what the real path serves"
        );
        assert!(
            through_link.nodes.iter().any(|node| node.id == "root/index"),
            "the loose root markdown must survive a junctioned wiki root"
        );
    }

    /// The Unix twin of the junction case above: `symlink_metadata` on a symlink reports
    /// `is_dir() == false`, so the configured-path walk lost the bucket the same way.
    #[cfg(unix)]
    #[test]
    fn a_symlinked_wiki_root_still_serves_its_loose_markdown() {
        let base = TempDir::new().unwrap();
        let real = base.path().join("real");
        fs::create_dir_all(&real).unwrap();
        write_page(&real, "index.md", "# Loose Index\n");
        write_page(&real, "patterns/known.md", "# Known\n");

        let link = base.path().join("link");
        std::os::unix::fs::symlink(&real, &link).expect("symlink the wiki root");

        let through_link = scan(&link);
        let control = scan(&real);
        assert_eq!(
            ids(&through_link),
            ids(&control),
            "a symlinked wiki root must serve exactly what the real path serves"
        );
        assert!(
            through_link.nodes.iter().any(|node| node.id == "root/index"),
            "the loose root markdown must survive a symlinked wiki root"
        );
    }

    /// A top-level directory whose name cannot become a folder tag is also a whole subtree lost,
    /// and must be named for the same reason.
    #[test]
    fn an_unusable_top_level_folder_name_is_reported_rather_than_dropped_in_silence() {
        let wiki = TempDir::new().unwrap();
        write_page(wiki.path(), "patterns/kept.md", "# Kept\n");
        let overlong = "n".repeat(MAX_FOLDER_NAME_BYTES + 1);
        write_page(wiki.path(), &format!("{overlong}/buried.md"), "# Buried\n");

        let graph = scan_reporting_refusals(wiki.path());
        assert_eq!(ids(&graph), HashSet::from(["patterns/kept"]));
        assert!(graph.truncated);
        let refused = omission(&graph, KnowledgeOmissionReason::FolderNameRejected)
            .expect("an unusable folder name must be reported");
        assert_eq!(refused.count, 1);
        assert_eq!(
            refused.examples,
            vec![overlong[..MAX_FOLDER_NAME_BYTES].to_string()],
            "the example must be bounded so a hostile name cannot inflate the response"
        );
    }

    /// A dot-directory is a documented *content* judgement (`.git`, `.obsidian` are editor and VCS
    /// state, not memory), so it stays silent. Reporting it would make the banner cry wolf on every
    /// scan of a normal wiki checkout.
    #[test]
    fn skipping_a_dot_directory_is_not_reported_as_an_omission() {
        let wiki = TempDir::new().unwrap();
        write_page(wiki.path(), "patterns/kept.md", "# Kept\n");
        write_page(wiki.path(), ".obsidian/workspace.md", "# Editor State\n");
        write_page(wiki.path(), ".git/notes.md", "# VCS State\n");

        let graph = scan_reporting_refusals(wiki.path());
        assert_eq!(ids(&graph), HashSet::from(["patterns/kept"]));
        assert!(
            graph.omissions.is_empty(),
            "a deliberate content skip must not be reported as a loss: {:?}",
            graph.omissions
        );
    }

    /// A symlink that stays *inside* the root is still refused, so the rule is "no symlinks in the
    /// walk", not "no symlinks that happen to escape" — the weaker rule would depend on resolving
    /// a target that can change between the check and the read.
    #[cfg(unix)]
    #[test]
    fn symlinks_are_refused_even_when_they_stay_inside_the_root() {
        use std::os::unix::fs::symlink;

        let wiki = TempDir::new().unwrap();
        write_page(wiki.path(), "patterns/real.md", "# Real\n");
        symlink(
            wiki.path().join("patterns/real.md"),
            wiki.path().join("patterns/alias.md"),
        )
        .unwrap();

        assert_eq!(ids(&scan(wiki.path())), HashSet::from(["patterns/real"]));
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

    /// No response — graph, page, or omission report — may ever carry an absolute filesystem path.
    /// The omission examples are the new surface here: they name pages, and this pins that they
    /// name them by node ID.
    #[test]
    fn no_response_contains_an_absolute_filesystem_path() {
        let wiki = TempDir::new().unwrap();
        write_page(wiki.path(), "index.md", "# Root Index\n");
        write_page(wiki.path(), "patterns/a.md", "# A\n");
        write_page(wiki.path(), "patterns/b.md", "# B\n");
        write_page(wiki.path(), "agents/c.md", "# C\n");
        write_page(
            wiki.path(),
            "patterns/oversize.md",
            &"x".repeat(MAX_FILE_BYTES + 1),
        );

        // A node cap plus an oversized file guarantees at least two distinct omissions carrying
        // examples, so this is not vacuously passing over an empty report.
        let graph = scan_knowledge(wiki.path(), &discovered(wiki.path()), None, 2, &[]);
        assert!(!graph.omissions.is_empty());
        assert!(graph
            .omissions
            .iter()
            .any(|omission| !omission.examples.is_empty()));

        let root_text = wiki.path().to_string_lossy().to_string();
        let alternate = root_text.replace('\\', "/");
        for serialized in [
            serde_json::to_string(&graph).unwrap(),
            serde_json::to_string(
                &preview_knowledge_page(wiki.path(), &discovered(wiki.path()), None, "root/index")
                    .expect("root index preview"),
            )
            .unwrap(),
        ] {
            assert!(
                !serialized.contains(&root_text) && !serialized.contains(&alternate),
                "an absolute path leaked into {serialized}"
            );
        }
    }

    // ---------------------------------------------------------------------------------------
    // 3. Honest truncation.
    // ---------------------------------------------------------------------------------------

    /// The regression test for the bug that started this: an ordinary, fully-scanned corpus must
    /// report *nothing*. The operator's live Atlas showed `truncated: true` over 100 of 100 pages
    /// because two `<- from:` prose annotations exceeded the link-hint limit, and a single boolean
    /// could not say so.
    #[test]
    fn a_fully_scanned_corpus_reports_no_omissions() {
        let wiki = TempDir::new().unwrap();
        write_page(
            wiki.path(),
            "patterns/source.md",
            "---\ntitle: Source\nlast_updated: 2026-07-19\n---\n# Source\n[[Target]]\n",
        );
        write_page(wiki.path(), "practices/target.md", "---\ntitle: Target\n---\n");
        write_page(wiki.path(), "agents/dossier.md", "# Dossier\n");
        write_page(wiki.path(), "index.md", "# Index\n");

        let graph = scan(wiki.path());
        assert_eq!(graph.nodes.len(), 4);
        assert_eq!(
            graph.omissions,
            Vec::new(),
            "a complete scan must report no omissions"
        );
        assert!(!graph.truncated);
    }

    /// A long prose annotation after a `<- from:` marker is the exact shape that used to raise the
    /// amber banner across the operator's whole graph. It resolves to nothing, it costs no page,
    /// and under the raised hint limit it is not an omission at all.
    #[test]
    fn long_provenance_prose_does_not_truncate_the_graph() {
        let wiki = TempDir::new().unwrap();
        let annotation = "session aecf4c89 backend-only PR; ".repeat(19);
        assert!(annotation.len() > 512, "fixture must exceed the old limit");
        assert!(
            annotation.len() < MAX_EDGE_HINT_BYTES,
            "fixture must fit the current limit"
        );
        write_page(
            wiki.path(),
            "practices/workflow.md",
            &format!("# Workflow\n<- from: TOG mainbrain ({annotation})\n"),
        );
        write_page(wiki.path(), "patterns/other.md", "# Other\n");

        let graph = scan(wiki.path());
        assert_eq!(graph.nodes.len(), 2);
        assert!(
            !graph.truncated,
            "prose provenance must not flag the corpus as truncated, got {:?}",
            graph.omissions
        );
    }

    /// Each distinct cause reports itself by name, with a count and page-ID examples — the whole
    /// point of replacing the boolean. Every assertion here is on scan output.
    #[test]
    fn the_truncation_report_names_the_correct_cause_for_each_trigger() {
        // (a) Node cap: whole pages dropped, and the count covers the untouched tail, not just the
        // one source the loop broke on.
        let capped = TempDir::new().unwrap();
        for index in 0..5 {
            write_page(
                capped.path(),
                &format!("patterns/page-{index}.md"),
                &format!("# Page {index}\n"),
            );
        }
        let graph = scan_knowledge(capped.path(), &discovered(capped.path()), None, 2, &[]);
        assert_eq!(graph.nodes.len(), 2);
        let node_cap = omission(&graph, KnowledgeOmissionReason::NodeCapReached)
            .expect("node cap omission");
        assert_eq!(node_cap.count, 3, "all three skipped pages must be counted");
        assert!(node_cap.examples.contains(&"patterns/page-2".to_string()));
        assert_eq!(reasons(&graph), vec![KnowledgeOmissionReason::NodeCapReached]);
        assert!(graph.truncated);

        // (b) File too large: one page dropped, the rest unaffected, and the reason is not
        // confusable with the node cap.
        let oversized = TempDir::new().unwrap();
        write_page(oversized.path(), "patterns/small.md", "# Small\n");
        write_page(
            oversized.path(),
            "patterns/huge.md",
            &"x".repeat(MAX_FILE_BYTES + 1),
        );
        let graph = scan(oversized.path());
        assert_eq!(ids(&graph), HashSet::from(["patterns/small"]));
        let too_large =
            omission(&graph, KnowledgeOmissionReason::FileTooLarge).expect("file-too-large");
        assert_eq!(too_large.count, 1);
        assert_eq!(too_large.examples, vec!["patterns/huge".to_string()]);
        assert_eq!(reasons(&graph), vec![KnowledgeOmissionReason::FileTooLarge]);

        // (c) Over-long link hint: reported distinctly, and the page itself is still present.
        let hinted = TempDir::new().unwrap();
        write_page(
            hinted.path(),
            "patterns/chatty.md",
            &format!("# Chatty\n<- from: {}\n", "y".repeat(MAX_EDGE_HINT_BYTES + 1)),
        );
        let graph = scan(hinted.path());
        assert_eq!(ids(&graph), HashSet::from(["patterns/chatty"]));
        let hint = omission(&graph, KnowledgeOmissionReason::EdgeHintTooLong)
            .expect("edge-hint omission");
        assert_eq!(hint.count, 1);
        assert_eq!(hint.examples, vec!["patterns/chatty".to_string()]);
        assert!(hint.detail.contains("all pages are shown"));

        // (d) Shortened metadata: two separate reasons, neither of which costs a page.
        let metadata = TempDir::new().unwrap();
        write_page(
            metadata.path(),
            "patterns/verbose.md",
            &format!(
                "---\ntitle: {}\nlast_updated: {}\n---\n# Body\n",
                format!("x{}", "é".repeat(MAX_TITLE_BYTES)),
                "u".repeat(MAX_LAST_UPDATED_BYTES + 1)
            ),
        );
        let graph = scan(metadata.path());
        assert_eq!(ids(&graph), HashSet::from(["patterns/verbose"]));
        assert_eq!(
            reasons(&graph),
            vec![
                KnowledgeOmissionReason::TitleTruncated,
                KnowledgeOmissionReason::LastUpdatedTruncated,
            ]
        );
        assert_eq!(
            omission(&graph, KnowledgeOmissionReason::TitleTruncated)
                .unwrap()
                .examples,
            vec!["patterns/verbose".to_string()]
        );

        // The report is not the guard. A pre-existing test asserted these bounds directly and was
        // deleted in favour of the report assertions above — but reporting a truncation while
        // emitting the untruncated value passes every assertion up to here. The fixture is
        // multibyte on purpose: with a leading ASCII byte, byte `MAX_TITLE_BYTES` lands mid-char,
        // so this also exercises `truncate_utf8`'s char-boundary walk-back.
        let node = graph
            .nodes
            .iter()
            .find(|node| node.id == "patterns/verbose")
            .expect("the page itself is still served");
        assert!(
            node.title.len() <= MAX_TITLE_BYTES,
            "title must be truncated, not merely reported: {} bytes",
            node.title.len()
        );
        assert!(
            node.title.is_char_boundary(node.title.len()),
            "title must be cut on a char boundary"
        );
        assert!(
            node.last_updated.len() <= MAX_LAST_UPDATED_BYTES,
            "last_updated must be truncated, not merely reported: {} bytes",
            node.last_updated.len()
        );
        assert!(
            serde_json::to_vec(&graph).unwrap().len() <= MAX_GRAPH_RESPONSE_BYTES,
            "the serialized graph must stay inside its response budget"
        );

        // (e) Depth ceiling: a directory nested past the limit is named as skipped, by a
        // root-relative label.
        let deep = TempDir::new().unwrap();
        let mut nested = String::from("patterns");
        for _ in 0..=MAX_DIRECTORY_DEPTH {
            nested.push_str("/d");
        }
        write_page(deep.path(), &format!("{nested}/leaf.md"), "# Leaf\n");
        write_page(deep.path(), "patterns/shallow.md", "# Shallow\n");
        let graph = scan(deep.path());
        assert!(ids(&graph).contains("patterns/shallow"));
        let depth = omission(&graph, KnowledgeOmissionReason::DirectoryDepthExceeded)
            .expect("depth omission");
        assert!(depth.count >= 1);
        assert!(
            depth.examples.iter().all(|example| !example.contains(':')),
            "depth examples must be root-relative, got {:?}",
            depth.examples
        );
    }


    /// Edge-side caps report as link omissions, never as missing pages, and `build_edges` is
    /// driven directly so the byte budget can be exercised without a multi-megabyte fixture.
    #[test]
    fn edge_caps_report_as_link_omissions() {
        let nodes: Vec<_> = (0..150)
            .map(|index| test_node(&format!("patterns/page-{index}"), &format!("Page {index}")))
            .collect();
        let lookup = NodeLookup::from_nodes(&nodes);
        let dense: Vec<RawEdge> = nodes
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
        let dense_len = dense.len();
        assert!(dense_len > MAX_EDGES, "fixture must exceed the edge cap");

        let mut report = TruncationReport::default();
        let edges = build_edges(dense, &lookup, MAX_GRAPH_ITEM_BYTES, &mut report);
        assert_eq!(edges.len(), MAX_EDGES);
        let omissions = report.into_omissions();
        let capped = omissions
            .iter()
            .find(|omission| omission.reason == KnowledgeOmissionReason::EdgeCapReached)
            .expect("edge cap omission");
        assert_eq!(
            capped.count,
            dense_len - MAX_EDGES,
            "the cap omission must pin an exact number, not merely a non-zero one"
        );
        assert!(capped.detail.contains("all pages are shown"));

        // The byte budget is a separate reason from the count cap.
        let mut report = TruncationReport::default();
        let edges = build_edges(
            vec![RawEdge {
                source: "patterns/page-0".to_string(),
                target_hint: "patterns/page-1".to_string(),
                kind: KnowledgeEdgeKind::CrossRef,
            }],
            &lookup,
            1,
            &mut report,
        );
        assert!(edges.is_empty());
        assert_eq!(
            report
                .into_omissions()
                .iter()
                .map(|omission| omission.reason)
                .collect::<Vec<_>>(),
            vec![KnowledgeOmissionReason::ResponseSizeCapReached]
        );
    }

    /// When the byte budget is what stopped the walk, the unscanned tail must be attributed to the
    /// byte budget — not to the node cap. Blaming the node cap sends the operator to raise a
    /// `?max_nodes=` that changes nothing, which is exactly the untrustworthy banner the omission
    /// taxonomy replaced.
    #[test]
    fn a_byte_budget_stop_does_not_blame_the_node_cap_for_the_tail() {
        let wiki = TempDir::new().unwrap();
        for index in 0..6 {
            write_page(
                wiki.path(),
                &format!("patterns/page-{index}.md"),
                "# Page\n",
            );
        }

        // A budget large enough for the first node but not the corpus, so the walk stops on bytes
        // while `nodes.len()` is still far below the node cap.
        let graph = scan_knowledge_within(
            wiki.path(),
            &discovered(wiki.path()),
            None,
            MAX_NODES,
            &[],
            220,
        );

        assert!(
            graph.nodes.len() < MAX_NODES,
            "the node cap must provably not be what stopped this scan"
        );
        assert!(
            !graph.nodes.is_empty(),
            "the fixture must scan something before the budget binds"
        );
        assert_eq!(
            reasons(&graph),
            vec![KnowledgeOmissionReason::ResponseSizeCapReached],
            "a byte-budget stop must report only the byte budget"
        );
        let reported = omission(&graph, KnowledgeOmissionReason::ResponseSizeCapReached)
            .expect("byte budget omission");
        assert_eq!(
            reported.count,
            6 - graph.nodes.len(),
            "every unscanned page must be counted under the reason that actually stopped the walk"
        );
    }

    /// A repeated link is one link. `build_edges` deduplicates *before* the cap so the banner
    /// counts distinct dropped edges; counting after the cap would multiply a single `[[Target]]`
    /// repeated in a long page into that many phantom omissions.
    ///
    /// `edge_caps_report_as_link_omissions` above cannot see this: its fixture is the full
    /// cross-product of distinct IDs, so every triple is unique and `seen.insert` is a no-op filter
    /// — swapping a no-op filter with the cap check cannot change its output.
    #[test]
    fn repeated_links_are_counted_once_against_the_edge_cap() {
        let nodes: Vec<_> = (0..150)
            .map(|index| test_node(&format!("patterns/page-{index}"), &format!("Page {index}")))
            .collect();
        let lookup = NodeLookup::from_nodes(&nodes);
        let mut dense: Vec<RawEdge> = nodes
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
        let distinct = dense.len();
        assert!(distinct > MAX_EDGES, "fixture must exceed the edge cap");

        // One page repeats a single link 500 times, the way a long `log.md` repeats `[[Target]]`.
        // `parse_body_edges` emits one RawEdge per occurrence, and nothing between there and here
        // deduplicates, so all 500 really do arrive.
        const REPEATS: usize = 500;
        dense.extend((0..REPEATS).map(|_| RawEdge {
            source: "patterns/page-0".to_string(),
            target_hint: "patterns/page-1".to_string(),
            kind: KnowledgeEdgeKind::CrossRef,
        }));

        let mut report = TruncationReport::default();
        let edges = build_edges(dense, &lookup, MAX_GRAPH_ITEM_BYTES, &mut report);
        assert_eq!(edges.len(), MAX_EDGES);
        let capped = report
            .into_omissions()
            .into_iter()
            .find(|omission| omission.reason == KnowledgeOmissionReason::EdgeCapReached)
            .expect("edge cap omission");
        // Exactly the distinct edges that did not fit. The 500 repeats are already represented by
        // an edge that WAS emitted, so they are not omissions at all and must not be counted.
        assert_eq!(
            capped.count,
            distinct - MAX_EDGES,
            "repeated links inflated the omission count by {}",
            capped.count as i64 - (distinct - MAX_EDGES) as i64
        );
    }

    /// A page preview reports its own omissions, not the corpus-wide enumeration's.
    #[test]
    fn preview_reports_only_its_own_omissions() {
        let wiki = TempDir::new().unwrap();
        write_page(
            wiki.path(),
            "patterns/preview.md",
            &format!(
                "---\ntitle: Preview Title\nlast_updated: 2026-07-19\n---\n# Preview\n{}",
                "é".repeat(MAX_PREVIEW_BYTES)
            ),
        );
        // An unrelated oversized page would flip a corpus-wide report, and must not appear here.
        write_page(
            wiki.path(),
            "patterns/unrelated-huge.md",
            &"x".repeat(MAX_FILE_BYTES + 1),
        );

        let page =
            preview_knowledge_page(wiki.path(), &discovered(wiki.path()), None, "patterns/preview")
                .expect("preview");
        assert_eq!(page.title, "Preview Title");
        assert!(!page.content.contains("last_updated:"));
        assert!(page.content.len() <= MAX_PREVIEW_BYTES);
        assert!(page.content.is_char_boundary(page.content.len()));
        assert!(page.truncated);
        assert_eq!(
            page.omissions
                .iter()
                .map(|omission| omission.reason)
                .collect::<Vec<_>>(),
            vec![KnowledgeOmissionReason::ContentTruncated],
            "the preview must not inherit an unrelated page's omission"
        );

        let short = preview_knowledge_page(
            wiki.path(),
            &discovered(wiki.path()),
            None,
            "patterns/unrelated-huge",
        );
        assert!(short.is_none(), "an oversized page has no preview");
        assert!(preview_knowledge_page(
            wiki.path(),
            &discovered(wiki.path()),
            None,
            "patterns/missing"
        )
        .is_none());
    }

    /// `truncated` stays exactly the derived "is anything in the report", so a client that only
    /// reads the old boolean keeps behaving the way it always did.
    #[test]
    fn truncated_is_derived_from_the_omission_report() {
        let clean = TempDir::new().unwrap();
        write_page(clean.path(), "patterns/a.md", "# A\n");
        let graph = scan(clean.path());
        assert_eq!(graph.truncated, !graph.omissions.is_empty());
        assert!(!graph.truncated);

        let dirty = TempDir::new().unwrap();
        write_page(dirty.path(), "patterns/a.md", "# A\n");
        write_page(dirty.path(), "patterns/b.md", "# B\n");
        let graph = scan_knowledge(dirty.path(), &discovered(dirty.path()), None, 1, &[]);
        assert_eq!(graph.truncated, !graph.omissions.is_empty());
        assert!(graph.truncated);
    }

    // ---------------------------------------------------------------------------------------
    // 4. Operator narrowing, IDs, and hints.
    // ---------------------------------------------------------------------------------------

    /// Config may only select among folders that actually exist under the root. It cannot construct
    /// a path, and it cannot name a folder into existence.
    #[test]
    fn configured_folders_can_only_narrow_the_discovered_set() {
        let wiki = TempDir::new().unwrap();
        write_page(wiki.path(), "patterns/kept.md", "# Kept\n");
        write_page(wiki.path(), "agents/dossier.md", "# Dossier\n");
        write_page(wiki.path(), "clients/acme/dashboard.md", "# Acme\n");
        let found = discover_wiki_folders(wiki.path()).folders;
        assert_eq!(
            found.iter().map(KnowledgeFolder::as_str).collect::<Vec<_>>(),
            vec!["agents", "clients", "patterns"]
        );

        // Absent key: everything discovered.
        assert_eq!(resolve_wiki_folders_from(None, found.clone()), found);

        // Narrowing, with case and whitespace tolerated and duplicates collapsed.
        assert_eq!(
            resolve_wiki_folders_from(
                Some(&[
                    "  Patterns ".to_string(),
                    "PATTERNS".to_string(),
                    "patterns".to_string()
                ]),
                found.clone()
            )
            .iter()
            .map(KnowledgeFolder::as_str)
            .collect::<Vec<_>>(),
            vec!["patterns"]
        );

        // Path-shaped and non-existent entries select nothing and never become paths.
        assert_eq!(
            resolve_wiki_folders_from(
                Some(&[
                    "../../../etc".to_string(),
                    "/etc/passwd".to_string(),
                    "C:\\Windows\\System32".to_string(),
                    "..".to_string(),
                    "clients/../agents".to_string(),
                    "patterns".to_string(),
                ]),
                found.clone()
            )
            .iter()
            .map(KnowledgeFolder::as_str)
            .collect::<Vec<_>>(),
            vec!["patterns"]
        );

        // Fail closed: a typo empties the Atlas rather than silently restoring everything.
        let typo = resolve_wiki_folders_from(Some(&["pattrens".to_string()]), found.clone());
        assert!(typo.is_empty());
        for empty in [
            resolve_wiki_folders_from(Some(&[]), found.clone()),
            resolve_wiki_folders_from(Some(&[String::new()]), found.clone()),
            resolve_wiki_folders_from(Some(&["   ".to_string()]), found),
        ] {
            assert!(empty.is_empty());
        }
    }

    /// End-to-end proof that narrowing is enforced at the filesystem walk, not merely in the
    /// resolver: an excluded folder yields no nodes and no preview.
    #[test]
    fn configured_narrowing_is_enforced_at_enumeration() {
        let wiki = TempDir::new().unwrap();
        write_page(wiki.path(), "patterns/kept.md", "# Kept\n");
        write_page(wiki.path(), "agents/dossier.md", "---\ntitle: Dossier\n---\n");

        let narrowed = resolve_wiki_folders_from(
            Some(&["patterns".to_string()]),
            discover_wiki_folders(wiki.path()).folders,
        );
        let graph = scan_knowledge(wiki.path(), &narrowed, None, MAX_NODES, &[]);
        assert_eq!(ids(&graph), HashSet::from(["patterns/kept"]));
        assert!(!serde_json::to_string(&graph).unwrap().contains("Dossier"));
        assert!(
            preview_knowledge_page(wiki.path(), &narrowed, None, "agents/dossier").is_none(),
            "a config-excluded folder must not be previewable"
        );

        let typo =
            resolve_wiki_folders_from(Some(&["pattrens".to_string()]), discover_wiki_folders(wiki.path()).folders);
        let empty = scan_knowledge(wiki.path(), &typo, None, MAX_NODES, &[]);
        assert!(empty.nodes.is_empty());
        assert!(preview_knowledge_page(wiki.path(), &typo, None, "patterns/kept").is_none());
    }

    #[test]
    fn project_nodes_are_reserved_ahead_of_a_saturated_global_corpus() {
        let wiki = TempDir::new().unwrap();
        let project = TempDir::new().unwrap();
        for index in 0..10 {
            write_page(
                wiki.path(),
                &format!("patterns/global-{index:02}.md"),
                &format!("# Global {index}\n"),
            );
        }
        write_page(project.path(), ".ai-docs/project-dna.md", "# Project DNA\n");
        write_page(project.path(), ".ai-docs/bug-patterns.md", "# Bug Patterns\n");
        write_page(project.path(), ".ai-docs/architecture.md", "# Not Scanned\n");

        let graph = scan_knowledge(
            wiki.path(),
            &discovered(wiki.path()),
            Some(project.path()),
            3,
            &[],
        );
        let node_ids: Vec<_> = graph.nodes.iter().map(|node| node.id.as_str()).collect();
        assert_eq!(
            &node_ids[..2],
            &[".project/project-dna", ".project/bug-patterns"],
            "project knowledge must survive a saturated global corpus"
        );
        assert!(!node_ids.contains(&".project/architecture"));
        assert!(graph.truncated);
    }

    #[test]
    fn knowledge_ids_reject_traversal_and_absolute_forms() {
        for invalid in [
            "",
            "patterns",
            "../agents/secret",
            "patterns/../agents/secret",
            "/patterns/secret",
            "C:/patterns/secret",
            "\\\\server\\patterns\\secret",
            "patterns\\secret",
            "patterns/./secret",
            "patterns//secret",
            ".hidden/secret",
            ".project/architecture",
            ".project/../patterns/page",
        ] {
            assert!(validate_knowledge_id(invalid).is_err(), "accepted {invalid}");
        }
        // Folders are discovered now, so an arbitrary-but-well-shaped prefix is syntactically
        // valid. It only ever resolves if enumeration actually produced that exact node.
        for valid in [
            "patterns/nested/page",
            "agents/recruiting/candidate",
            "zettelkasten/202607190001",
            "root/index",
            ".project/project-dna",
            ".project/bug-patterns",
            // A wiki folder literally named `project` is an ordinary discovered folder now. It
            // used to inherit the session bucket's two-file allowlist, so the graph advertised
            // nodes the preview endpoint answered with 400.
            "project/architecture",
            "project/roadmap",
        ] {
            assert!(validate_knowledge_id(valid).is_ok(), "rejected {valid}");
        }
    }

    /// A syntactically valid ID naming a folder that exists but a page that does not must resolve
    /// to nothing — the enumeration match, not the prefix check, is what actually bounds previews.
    #[test]
    fn a_well_formed_id_still_resolves_only_against_enumerated_sources() {
        let wiki = TempDir::new().unwrap();
        write_page(wiki.path(), "patterns/real.md", "# Real\n");
        let folders = discovered(wiki.path());

        assert!(preview_knowledge_page(wiki.path(), &folders, None, "patterns/real").is_some());
        for absent in [
            "patterns/imaginary",
            "imaginary/page",
            "patterns/nested/imaginary",
            "root/index",
        ] {
            assert!(
                preview_knowledge_page(wiki.path(), &folders, None, absent).is_none(),
                "resolved a page that was never enumerated: {absent}"
            );
        }
    }

    fn test_node(id: &str, title: &str) -> KnowledgeNode {
        let folder = id.split('/').next().unwrap_or("patterns");
        KnowledgeNode {
            id: id.to_string(),
            title: title.to_string(),
            folder: KnowledgeFolder::new(folder),
            path: format!("{id}.md"),
            last_updated: String::new(),
            in_degree: 0,
            out_degree: 0,
        }
    }

    /// Hints resolve against folders that exist *in this graph*, which is the discovered-corpus
    /// form of the old compiled-in gate. `agents/` now resolves; `project/` still must not be
    /// reachable by walking upward from a wiki page.
    #[test]
    fn hints_resolve_against_graph_folders_without_disallowed_fallback() {
        let nodes = vec![
            test_node("patterns/cross", "Pattern Cross"),
            test_node("clients/cross", "Client Cross"),
            test_node("agents/cross", "Agent Cross"),
            test_node("clients/acme/dashboard", "Acme Dashboard"),
            test_node(".project/project-dna", "Project DNA"),
        ];
        let lookup = NodeLookup::from_nodes(&nodes);

        for (hint, expected) in [
            ("patterns/cross.md", "patterns/cross"),
            ("../patterns/cross.md", "patterns/cross"),
            ("../../wiki/patterns/cross.md (annotation)", "patterns/cross"),
            ("clients/acme/dashboard.md", "clients/acme/dashboard"),
            ("../clients/acme/dashboard.md", "clients/acme/dashboard"),
            // The folder that used to be unreachable by construction.
            ("agents/cross.md", "agents/cross"),
            ("../agents/cross.md", "agents/cross"),
        ] {
            assert_eq!(
                resolve_hint(hint, &lookup).as_deref(),
                Some(expected),
                "hint {hint} did not resolve"
            );
        }

        for rejected in [
            // Session-scoped project knowledge is not reachable by walking up from a wiki page.
            "../project/project-dna.md",
            // A folder that is not in this graph cannot anchor an explicit path...
            "nonexistent/cross.md",
            "../nonexistent/cross.md",
            // ...and an explicit path must never fall back to a same-basename page elsewhere.
            "../patterns/../nonexistent/cross.md",
        ] {
            assert_eq!(resolve_hint(rejected, &lookup), None, "accepted {rejected}");
        }

        // Ambiguity guard: `cross` names three nodes, so no edge is invented.
        assert_eq!(resolve_hint("cross", &lookup), None);
        assert_eq!(
            resolve_hint("Acme Dashboard", &lookup).as_deref(),
            Some("clients/acme/dashboard")
        );
    }

    #[test]
    fn scanner_builds_all_five_edge_kinds_across_discovered_folders() {
        let wiki = TempDir::new().unwrap();
        write_page(
            wiki.path(),
            "patterns/source.md",
            "---\ntitle: Source #153\nlast_updated: 2026-07-18\ncross_refs:\n  - wiki/practices/cross.md\n  - agents/cross.md\nrelated: [research/frontmatter-related.md]\n---\n# Source\n[[Wiki Target]]\n-> global: patterns/global.md\n-> related: [[body-related]] (strong match)\n<-> related: wiki/research/frontmatter-related.md (§details)\n<- from: research/from.md\n",
        );
        write_page(wiki.path(), "practices/cross.md", "---\ntitle: Cross\n---\n");
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
        // `agents/cross.md` is now a real, reachable node, so the `agents/cross.md` cross-ref above
        // resolves instead of being dropped by an allowlist.
        write_page(wiki.path(), "agents/cross.md", "---\ntitle: Agent Cross\n---\n");

        let graph = scan(wiki.path());
        let seen = ids(&graph);
        assert!(seen.contains("patterns/source"));
        assert!(seen.contains("agents/cross"));

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
            seen.contains(edge.source.as_str()) && seen.contains(edge.target.as_str())
        }));
        assert!(graph.edges.iter().any(|edge| {
            edge.kind == KnowledgeEdgeKind::CrossRef && edge.target == "agents/cross"
        }));
        assert_eq!(
            graph
                .nodes
                .iter()
                .find(|node| node.id == "patterns/source")
                .unwrap()
                .title,
            "Source #153"
        );
        assert!(!graph.truncated, "unexpected omissions: {:?}", graph.omissions);
    }

    #[test]
    fn nested_entity_pages_scan_and_cross_link_between_graph_halves() {
        let wiki = TempDir::new().unwrap();
        write_page(
            wiki.path(),
            "clients/acme/dashboard.md",
            "---\ntitle: Acme Dashboard\nlast_updated: 2026-07-18\n---\n# Acme\n[[partners/beta]]\n",
        );
        write_page(
            wiki.path(),
            "partners/beta.md",
            "---\ntitle: Beta Partner\n---\n# Beta\n",
        );
        write_page(
            wiki.path(),
            "patterns/delivery.md",
            "---\ntitle: Delivery\ncross_refs:\n  - clients/acme/dashboard.md\n---\n# Delivery\n[[partners/beta]]\n",
        );

        let graph = scan(wiki.path());
        let dashboard = graph
            .nodes
            .iter()
            .find(|node| node.id == "clients/acme/dashboard")
            .expect("dashboard node");
        assert_eq!(dashboard.title, "Acme Dashboard");
        assert_eq!(dashboard.folder.as_str(), "clients");
        assert_eq!(dashboard.path, "clients/acme/dashboard.md");

        let has_edge = |source: &str, target: &str, kind: KnowledgeEdgeKind| {
            graph
                .edges
                .iter()
                .any(|edge| edge.source == source && edge.target == target && edge.kind == kind)
        };
        assert!(has_edge(
            "clients/acme/dashboard",
            "partners/beta",
            KnowledgeEdgeKind::Wikilink
        ));
        assert!(has_edge(
            "patterns/delivery",
            "clients/acme/dashboard",
            KnowledgeEdgeKind::CrossRef
        ));
        assert_eq!(dashboard.in_degree, 1);
        assert_eq!(dashboard.out_degree, 1);
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

        // Keeping the old graph's session id after the live session switches cannot resolve the
        // replacement project's identically-named `project/project-dna` node.
        let switched = [("session-b", second)];
        let error =
            match resolve_project_root_from_sessions(Some("session-a"), switched.iter().copied()) {
                Ok(_) => panic!("stale session unexpectedly resolved a project"),
                Err(error) => error,
            };
        assert_eq!(error.status, axum::http::StatusCode::NOT_FOUND);

        let invalid =
            match resolve_project_root_from_sessions(Some("../session-a"), sessions.iter().copied())
            {
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
    fn directory_entry_collection_is_bounded_before_sorting() {
        let root = TempDir::new().unwrap();
        for index in 0..12 {
            write_page(root.path(), &format!("page-{index:02}.md"), "# Page\n");
        }
        let canonical_root = fs::canonicalize(root.path()).unwrap();
        let mut entries_seen = MAX_SCAN_ENTRIES - 3;
        let mut report = TruncationReport::default();
        let mut walk = DirectoryWalk {
            canonical_root: canonical_root.clone(),
            file_limit: MAX_DISCOVERED_FILES,
            recurse: true,
            entries_seen: &mut entries_seen,
            output: Vec::new(),
        };
        walk.visit(&canonical_root, 0, &mut report);

        assert_eq!(walk.output.len(), 3);
        assert_eq!(entries_seen, MAX_SCAN_ENTRIES);
        assert_eq!(
            report
                .into_omissions()
                .iter()
                .map(|omission| omission.reason)
                .collect::<Vec<_>>(),
            vec![KnowledgeOmissionReason::ScanEntryCapReached]
        );
    }

    #[test]
    fn folder_name_shape_gate_rejects_path_fragments() {
        for valid in ["patterns", "Field Notes", "agents", "a", "202607-notes"] {
            assert!(is_valid_folder_name(valid), "rejected {valid}");
        }
        for invalid in [
            "",
            ".",
            "..",
            ".obsidian",
            ".git",
            "a/b",
            "a\\b",
            "C:",
            "with\0null",
            &"x".repeat(MAX_FOLDER_NAME_BYTES + 1),
        ] {
            assert!(!is_valid_folder_name(invalid), "accepted {invalid}");
        }
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

    #[test]
    fn a_missing_wiki_root_degrades_to_an_empty_graph() {
        let wiki = TempDir::new().unwrap();
        let missing = wiki.path().join("does-not-exist");
        let discovered = discover_wiki_folders(&missing);
        assert!(discovered.folders.is_empty());
        assert!(discovered.refusals.is_empty());
        let graph = scan_knowledge(&missing, &[], None, MAX_NODES, &[]);
        assert!(graph.nodes.is_empty());
        assert!(graph.edges.is_empty());
        assert!(!graph.truncated);
    }
}
