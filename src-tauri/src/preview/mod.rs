use tauri::Url;

#[cfg(not(test))]
use crate::http::state::AppState;
#[cfg(not(test))]
use parking_lot::Mutex;
// Unconditional: the persisted preview STATE MACHINE below is plain data and is
// compiled in test builds too, so its transitions can be driven directly.
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
#[cfg(not(test))]
use std::path::PathBuf;
#[cfg(not(test))]
use std::sync::atomic::{AtomicBool, Ordering};
#[cfg(not(test))]
use std::sync::{Arc, OnceLock};
#[cfg(not(test))]
use tauri::{
    webview::NewWindowResponse, AppHandle, Emitter, LogicalSize, Manager, PhysicalPosition,
    PhysicalSize, State, WebviewUrl, WebviewWindow, WebviewWindowBuilder, WindowEvent,
};

#[cfg(not(test))]
pub const PREVIEW_WINDOW_LABEL: &str = "operator-preview";
#[cfg(not(test))]
const MAIN_WINDOW_LABEL: &str = "main";
const LOCAL_API_PORT: u16 = 18_800;
const MAIN_DEV_SERVER_PORT: u16 = 1_420;

// ---------------------------------------------------------------------------
// URL entry (issue #157 §1)
// ---------------------------------------------------------------------------

/// True when a scheme candidate is spelled like a HOSTNAME rather than a scheme.
///
/// This is the tie-breaker for the one genuinely ambiguous input shape,
/// `word:digits` - which is both a legal RFC 3986 `scheme:opaque` (`tel:12345`)
/// and the `host:port` an operator types (`localhost:5173`).
fn looks_like_hostname(candidate: &str) -> bool {
    if candidate.is_empty() {
        return false;
    }

    // RFC 3986 permits `+` in a scheme (`svn+ssh:`); a hostname never contains
    // one, so its presence settles the ambiguity in favour of "scheme".
    if !candidate
        .bytes()
        .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'.'))
    {
        return false;
    }

    // A dot makes it a multi-label name (`example.com`, `foo.localhost`, the
    // FQDN-absolute `localhost.`). Single-label names are ambiguous with real
    // schemes - `tel`, `mailto`, `vscode`, `steam` are all bare words - so only
    // the well-known local one is accepted. This deliberately gives up on
    // single-label intranet hosts (`myserver:8080`); typing the scheme is the
    // documented workaround, and it is the only rule that also rejects
    // `tel:12345`.
    //
    // IP literals need no case of their own here: `has_explicit_scheme` has
    // already classified anything not starting with an ASCII letter as
    // scheme-less, which covers IPv4 (`127.0.0.1:3000`, leading digit) and
    // bracketed IPv6 (`[::1]:8080`, leading `[`).
    candidate.contains('.') || candidate.eq_ignore_ascii_case("localhost")
}

/// Returns true when `input` already carries an explicit `scheme:` prefix.
///
/// This is a deliberate LEXICAL scan, not a parse. Neither parse success nor
/// parse failure can classify scheme-less operator input, because it fails in
/// two opposite ways:
///   * `Url::parse("localhost:5173")` SUCCEEDS with the bogus scheme `localhost`
///     and only dies later at the scheme allowlist.
///   * `Url::parse("127.0.0.1:5173")` FAILS outright.
/// Only a lexical scan puts both on the same path.
///
/// # Security
///
/// This must NOT key off `://`. Requiring the double slash classified every
/// opaque and single-slash scheme as scheme-less, so `normalize_preview_input`
/// prepended `https://` and the result sailed through the scheme allowlist as
/// "https" - while the untrusted navigation gate then let wry navigate to the
/// ORIGINAL string. `vscode:/x`, `steam:/run/1`, `ms-settings:/`, `tel:12345`,
/// `javascript:0` and `ms-msdt:/id ...` all reached OS protocol handlers that
/// way, and `http:/localhost:18800` flipped the reserved-API-port guard from
/// reject to accept (WHATWG accepts any number of slashes after a special
/// scheme, so the guard saw host `http`, port 443, path `/localhost:18800`).
///
/// Accepting any `scheme:` is equally wrong - it re-breaks `localhost:5173`,
/// which is the entire point of forgiving entry. So a valid scheme prefix wins
/// UNLESS the input is unambiguously `host:port`:
///   (a) the body up to the first `/ ? # \` is non-empty and all ASCII digits, and
///   (b) the scheme candidate is spelled like a hostname ([`looks_like_hostname`]).
/// `explicit_scheme_detection_requires_a_host_port_shape` regression-locks the
/// full trace table.
fn has_explicit_scheme(input: &str) -> bool {
    let Some(colon) = input.find(':') else {
        return false;
    };

    let scheme = &input[..colon];

    // A ':' that appears after a path separator is not a scheme delimiter
    // (`example.com/a:b`).
    if scheme.contains(['/', '?', '#', '\\']) {
        return false;
    }

    // RFC 3986 scheme grammar: ALPHA *( ALPHA / DIGIT / "+" / "-" / "." )
    if !scheme
        .as_bytes()
        .first()
        .is_some_and(u8::is_ascii_alphabetic)
    {
        return false;
    }
    if !scheme
        .bytes()
        .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'+' | b'-' | b'.'))
    {
        return false;
    }

    // A real scheme. Only the `host:port` shape outranks it.
    let body = &input[colon + 1..];
    let port_end = body.find(['/', '?', '#', '\\']).unwrap_or(body.len());
    let port = &body[..port_end];

    let looks_like_host_port = !port.is_empty()
        && port.bytes().all(|byte| byte.is_ascii_digit())
        && looks_like_hostname(scheme);

    !looks_like_host_port
}

/// Extracts the host from scheme-less operator input, purely to choose between
/// `http` and `https`.
///
/// The result is NEVER reassembled back into a URL. See
/// [`normalize_preview_input`] for why that distinction is load-bearing.
fn schemeless_host(input: &str) -> &str {
    let rest = input.strip_prefix("//").unwrap_or(input);
    let authority_end = rest.find(['/', '?', '#', '\\']).unwrap_or(rest.len());
    let authority = &rest[..authority_end];

    // Any userinfo is stripped so classification looks at the real host; the
    // credentials themselves are rejected later by `validate_preview_url`.
    let authority = match authority.rfind('@') {
        Some(at) => &authority[at + 1..],
        None => authority,
    };

    // IPv6 literals are bracketed, and the colons inside them are not port
    // delimiters.
    if let Some(after_bracket) = authority.strip_prefix('[') {
        return match after_bracket.find(']') {
            Some(end) => &after_bracket[..end],
            None => after_bracket,
        };
    }

    match authority.rfind(':') {
        // An EMPTY port segment is vacuously all-digits, and stripping it is
        // what makes `//localhost:/dashboard` classify as localhost-ish. Keeping
        // the trailing colon yielded the host `localhost:`, which
        // `is_localhostish_host` trims `.`/`[`/`]` from but not `:` - so the
        // operator's plain-HTTP dev server was scheme-selected as `https` and
        // failed the TLS handshake.
        Some(colon) if authority[colon + 1..].bytes().all(|b| b.is_ascii_digit()) => {
            &authority[..colon]
        }
        _ => authority,
    }
}

/// Hosts that should default to `http://` rather than `https://`, because a
/// local dev server almost never has a certificate.
fn is_localhostish_host(host: &str) -> bool {
    // A trailing dot is the FQDN-absolute spelling of the same name.
    let host = host
        .trim_end_matches('.')
        .trim_matches(|c| c == '[' || c == ']')
        .to_ascii_lowercase();

    host == "localhost" || host.ends_with(".localhost") || host == "127.0.0.1" || host == "::1"
}

/// Make forgiving operator input parseable: `localhost:5173` becomes
/// `http://localhost:5173`, `github.com/owner/repo` becomes
/// `https://github.com/owner/repo`. Input that already has a scheme is returned
/// unchanged (trimmed).
///
/// # Security
///
/// When a scheme is missing this performs a PURE STRING PREPEND onto the
/// trimmed raw input. It must never decompose the input into host + path and
/// reassemble it. A reassembling normalizer turns `localhost:18800` into
/// `http://localhost/18800`, which relocates the reserved Hive API port into
/// the PATH: the parsed port becomes 80/443, the reserved-port check passes,
/// and the local API is opened as untrusted preview content.
/// `normalization_never_relocates_the_port` regression-locks this.
pub(crate) fn normalize_preview_input(input: &str) -> String {
    let trimmed = input.trim();
    if trimmed.is_empty() || has_explicit_scheme(trimmed) {
        return trimmed.to_string();
    }

    let host = schemeless_host(trimmed);
    if host.is_empty() {
        // Path-only input (`/relative/path`, `?q=1`) has no authority to attach a
        // scheme to. It is returned unchanged so `Url::parse` rejects it. Prepending
        // anyway would produce `https:///relative/path`, and the WHATWG
        // "special authority ignore slashes" rule collapses the extra slash and
        // promotes `relative` to the HOST - silently inventing an origin the
        // operator never typed.
        return trimmed.to_string();
    }

    let scheme = if is_localhostish_host(host) {
        "http"
    } else {
        "https"
    };

    format!("{scheme}://{trimmed}")
}

/// Validate an operator-TYPED preview URL: normalize the forgiving entry forms
/// first, then apply the real rules.
///
/// # Security
///
/// This is the ONLY entry point permitted to normalize, and the ONLY caller is
/// the `open_preview_window` command. Normalization is an operator-input
/// affordance for humans who type `localhost:5173`; it is pure downside anywhere
/// else. A URL arriving from a live page is already absolute, so prepending a
/// scheme to it can only ever LAUNDER a scheme the allowlist would otherwise
/// reject. Keeping the navigation gate on [`validate_preview_url`] - which never
/// normalizes - retires that whole class of bypass rather than patching its
/// instances. `the_navigation_gate_never_normalizes` regression-locks it.
pub(crate) fn validate_operator_preview_input(
    input: &str,
    configured_api_port: u16,
) -> Result<Url, String> {
    let trimmed = input.trim();
    if trimmed.is_empty() {
        return Err(
            "Enter a preview URL. The scheme is optional: localhost:5173, \
             github.com/owner/repo, or https://example.com"
                .to_string(),
        );
    }

    validate_preview_url(&normalize_preview_input(trimmed), configured_api_port)
}

/// Parse and validate a preview URL before it reaches a webview. The single
/// source of truth for what preview content is allowed to be.
///
/// Preview content is deliberately limited to ordinary web origins. The local Hive
/// API is excluded because loading it as the top-level origin would bypass browser
/// CORS protections for subsequent same-origin requests.
///
/// Takes the input EXACTLY as given - see
/// [`validate_operator_preview_input`] for why normalization is quarantined to
/// the operator path.
pub(crate) fn validate_preview_url(input: &str, configured_api_port: u16) -> Result<Url, String> {
    let trimmed = input.trim();
    if trimmed.is_empty() {
        return Err(
            "Enter a preview URL. The scheme is optional: localhost:5173, \
             github.com/owner/repo, or https://example.com"
                .to_string(),
        );
    }

    let url = Url::parse(trimmed).map_err(|_| {
        "Enter a valid preview URL, such as localhost:5173, 127.0.0.1:3000/x, \
         or github.com/owner/repo"
            .to_string()
    })?;

    if !matches!(url.scheme(), "http" | "https") {
        return Err(
            "Preview URLs must be http or https. Try localhost:5173 or example.com/path"
                .to_string(),
        );
    }

    url.host_str()
        .ok_or_else(|| "Preview URL must include a host".to_string())?;

    if !url.username().is_empty() || url.password().is_some() {
        return Err("Preview URLs cannot contain embedded credentials".to_string());
    }

    // The API port is reserved regardless of hostname. Blocking the port rather
    // than a hostname list also covers localhost aliases and DNS rebinding.
    if url
        .port_or_known_default()
        .is_some_and(|port| port == LOCAL_API_PORT || port == configured_api_port)
    {
        return Err("The local Hive API cannot be opened as preview content".to_string());
    }

    if is_trusted_main_window_origin(&url) {
        return Err(
            "Hive Manager's trusted app origin cannot be opened as preview content".to_string(),
        );
    }

    Ok(url)
}

fn is_trusted_main_window_origin(url: &Url) -> bool {
    let Some(host) = url.host_str() else {
        return false;
    };

    // The url crate PRESERVES a trailing FQDN dot, so `tauri.localhost.`,
    // `localhost.:1420` and even `tauri.localhost..` reach the very same origin
    // as the bare spellings while comparing unequal to them. Every trailing dot
    // is folded, matching what `is_localhostish_host` already does.
    let host = host.trim_end_matches('.');

    host.eq_ignore_ascii_case("tauri.localhost")
        || (host.eq_ignore_ascii_case("localhost")
            && url.port_or_known_default() == Some(MAIN_DEV_SERVER_PORT))
}

/// wry's `on_navigation` gate for UNTRUSTED remote preview content.
///
/// # Security
///
/// A `true` here lets wry navigate to the ORIGINAL url, so this must call
/// [`validate_preview_url`] and never
/// [`validate_operator_preview_input`]. Normalizing a navigation URL cannot
/// help - it is already absolute - and can only launder a rejected scheme into
/// an accepted one.
fn preview_navigation_allowed(url: &Url, configured_api_port: u16) -> bool {
    validate_preview_url(url.as_str(), configured_api_port).is_ok()
}

// ---------------------------------------------------------------------------
// Dock geometry (issue #157 §2)
// ---------------------------------------------------------------------------

/// Mirrors `main.minWidth` in tauri.conf.json. Docking must never ask the main
/// window for a width the OS will refuse, because the refusal shows up as the
/// two windows silently overlapping - the exact failure tiling exists to avoid.
const MAIN_MIN_LOGICAL_WIDTH: f64 = 800.0;
/// Narrowest useful docked preview column. Also the minimum size temporarily
/// applied to the preview window while docked: its configured 640 logical
/// minimum would fight the split on a 1080p display at 150% scale, which only
/// has 1280 logical px of work area.
const DOCKED_PREVIEW_MIN_LOGICAL_WIDTH: f64 = 420.0;
#[cfg(not(test))]
const DOCKED_PREVIEW_MIN_LOGICAL_HEIGHT: f64 = 360.0;
/// Mirrors `operator-preview.minWidth` / `minHeight` in tauri.conf.json, so the
/// constraint relaxed for docking is put back verbatim on undock.
#[cfg(not(test))]
const FLOATING_PREVIEW_MIN_LOGICAL_WIDTH: f64 = 640.0;
#[cfg(not(test))]
const FLOATING_PREVIEW_MIN_LOGICAL_HEIGHT: f64 = 480.0;
/// Share of the monitor work area handed to the docked preview.
const DOCKED_PREVIEW_WIDTH_FRACTION: f64 = 0.40;

/// Split a monitor work area into two flush, non-overlapping columns:
/// `(main_width, preview_width)` in physical pixels.
///
/// The preview width is subtracted from the total rather than computed
/// independently, so the two columns provably sum to the work area exactly -
/// no rounding gap, no rounding overlap.
///
/// Returns `Err` when the display simply cannot host both windows at their
/// minimum widths. Refusing is deliberate: a "best effort" split that overlaps
/// puts a native WebView2 HWND on top of the main window's HTML, and no CSS
/// `z-index` in the app can ever paint over it.
fn dock_split(work_area_width: u32, scale_factor: f64) -> Result<(u32, u32), String> {
    let scale = if scale_factor.is_finite() && scale_factor > 0.0 {
        scale_factor
    } else {
        1.0
    };

    let total = f64::from(work_area_width);
    // Rounded UP so the integer columns below can never land a pixel under the
    // minimum the OS will actually honour.
    let main_min = (MAIN_MIN_LOGICAL_WIDTH * scale).ceil() as u32;
    let preview_min = (DOCKED_PREVIEW_MIN_LOGICAL_WIDTH * scale).ceil() as u32;

    if work_area_width < main_min.saturating_add(preview_min) {
        return Err(format!(
            "This display is too narrow to dock the preview beside Hive Manager. \
             Docking needs about {needed} logical px of work area and this monitor has {have}. \
             Pop the preview out instead.",
            needed = (MAIN_MIN_LOGICAL_WIDTH + DOCKED_PREVIEW_MIN_LOGICAL_WIDTH).round() as i64,
            have = (total / scale).round() as i64,
        ));
    }

    // Clamped in integer pixels, then subtracted from the total. Both columns
    // therefore respect their minimum AND sum to the work area exactly - no
    // rounding gap, and more importantly no rounding overlap.
    let preview_width = ((total * DOCKED_PREVIEW_WIDTH_FRACTION).round() as u32)
        .clamp(preview_min, work_area_width - main_min);
    let main_width = work_area_width - preview_width;

    Ok((main_width, preview_width))
}

/// True when `(x, y)` - a window's top-left corner - falls inside one of the
/// supplied monitor work areas, given as `(x, y, width, height)`.
///
/// Guards restoring a remembered position onto a monitor that no longer exists.
/// A laptop undocked from an external display would otherwise reopen the preview
/// at coordinates no screen covers, and the operator sees nothing at all.
fn position_is_on_a_monitor(x: i32, y: i32, work_areas: &[(i32, i32, u32, u32)]) -> bool {
    work_areas.iter().any(|&(monitor_x, monitor_y, width, height)| {
        x >= monitor_x
            && y >= monitor_y
            && x < monitor_x.saturating_add(width as i32)
            && y < monitor_y.saturating_add(height as i32)
    })
}

// ---------------------------------------------------------------------------
// Preview window runtime (issue #157 §2)
// ---------------------------------------------------------------------------

/// Emitted to the main window on every allowed preview navigation, so the
/// address bar can show where in-page navigation actually went.
#[cfg(not(test))]
const PREVIEW_NAVIGATED_EVENT: &str = "preview-navigated";
/// Emitted whenever the preview opens, closes, docks or undocks.
#[cfg(not(test))]
const PREVIEW_STATUS_EVENT: &str = "preview-status";
#[cfg(not(test))]
const PREVIEW_STATE_FILE: &str = "preview-window-state.json";
/// Coalescing window for the resize/move storm produced by a window drag.
#[cfg(not(test))]
const RETILE_DEBOUNCE_MS: u64 = 60;

/// A window rectangle as persisted between runs.
///
/// The position is PHYSICAL (absolute desktop coordinates, so it identifies the
/// monitor) while the size is LOGICAL (DPI-independent, so restoring onto a
/// differently scaled monitor preserves apparent size instead of halving or
/// doubling it).
#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
struct WindowGeometry {
    x: i32,
    y: i32,
    width: f64,
    height: f64,
}

#[derive(Debug, Default, Clone, Serialize, Deserialize)]
struct PersistedPreviewState {
    #[serde(default)]
    docked: bool,
    #[serde(default)]
    floating: Option<WindowGeometry>,
    #[serde(default)]
    main_restore: Option<WindowGeometry>,
    #[serde(default)]
    main_was_maximized: bool,
    /// True while a docked LAYOUT is applied, i.e. `main_restore` holds a
    /// snapshot that has NOT been consumed yet.
    ///
    /// Deliberately not the same bit as `docked`: that is the operator
    /// PREFERENCE and survives a teardown so the next open re-tiles. Conflating
    /// the two made a reopened dock skip the pre-dock snapshot, leaving the
    /// undock/close after it with nothing to restore and Hive Manager stranded
    /// in the left-hand column.
    #[serde(default)]
    layout_applied: bool,
    #[serde(default)]
    session_urls: BTreeMap<String, String>,
}

impl PersistedPreviewState {
    /// A snapshot is owed whenever no live layout is applied - including the
    /// reopen after a docked close, which keeps `docked` but has already had its
    /// snapshot consumed.
    fn needs_main_snapshot(&self) -> bool {
        !self.layout_applied
    }

    fn record_main_snapshot(&mut self, geometry: Option<WindowGeometry>, maximized: bool) {
        self.main_restore = geometry;
        self.main_was_maximized = maximized;
        self.layout_applied = true;
    }

    /// Consumes the snapshot. Clearing the flag alongside the `take` is what
    /// keeps "a layout is live" and "a snapshot exists" from ever disagreeing.
    fn take_main_restore(&mut self) -> (Option<WindowGeometry>, bool) {
        self.layout_applied = false;
        (
            self.main_restore.take(),
            std::mem::take(&mut self.main_was_maximized),
        )
    }

    fn dock_fields(&self) -> DockFields {
        DockFields {
            docked: self.docked,
            floating: self.floating,
            main_restore: self.main_restore,
            main_was_maximized: self.main_was_maximized,
            layout_applied: self.layout_applied,
        }
    }

    /// Prepare a dock attempt from reads that must happen BEFORE any window
    /// moves, without touching a single field.
    ///
    /// Taking `&self` is the load-bearing part: it makes "commit the ACTIVE
    /// state before the tiling is known to have worked" un-writable rather than
    /// merely discouraged.
    fn stage_dock(
        &self,
        main_geometry: Option<WindowGeometry>,
        main_was_maximized: bool,
        preview_geometry: Option<WindowGeometry>,
    ) -> StagedDock {
        StagedDock {
            previous: self.dock_fields(),
            snapshot_owed: self.needs_main_snapshot(),
            main_geometry,
            main_was_maximized,
            preview_geometry,
        }
    }

    /// Apply the outcome of the tiling attempt - the one place `docked` is ever
    /// set.
    ///
    /// `docked` is what `status_for` reports AND what the main-window follow
    /// handler gates its retile on, so setting it before the geometry calls
    /// returned left a failed dock reporting docked, keeping the docked minimum
    /// size applied, and letting the next drag snap the preview into a column
    /// for a dock that had errored. Committing here, off the tiling result,
    /// is what makes that unrepresentable.
    fn settle_dock<E>(&mut self, staged: StagedDock, tiled: &Result<(), E>) {
        if tiled.is_ok() {
            self.commit_dock(staged);
        } else {
            self.rollback_dock(staged);
        }
    }

    fn commit_dock(&mut self, staged: StagedDock) {
        // Gated on the LAYOUT bit, not the preference - see `needs_main_snapshot`.
        if staged.snapshot_owed {
            self.record_main_snapshot(staged.main_geometry, staged.main_was_maximized);
        }
        // The FLOATING snapshot keeps its gate on the PREFERENCE as it stood
        // before the attempt: a reopened preview is a brand-new window at
        // config-default geometry, and re-snapshotting it there would clobber
        // the operator's remembered floating position.
        //
        // Writing the staged read HERE rather than at stage time also closes a
        // race the old ordering had no answer for: the preview's own
        // `Moved`/`Resized` handler records `floating` whenever `!docked`, so
        // the tiling of the preview into its column can land in `floating`
        // while the dock is still in flight. This overwrites that with the
        // geometry actually read before the window moved.
        if !staged.previous.docked {
            if let Some(geometry) = staged.preview_geometry {
                self.floating = Some(geometry);
            }
        }
        self.docked = true;
    }

    /// Put every dock field back exactly as it was.
    ///
    /// Restoring `floating` matters even though `stage_dock` never wrote it:
    /// the preview's window-event handler may have recorded the half-tiled
    /// column into it while the dock was in flight.
    fn rollback_dock(&mut self, staged: StagedDock) {
        let DockFields {
            docked,
            floating,
            main_restore,
            main_was_maximized,
            layout_applied,
        } = staged.previous;
        self.docked = docked;
        self.floating = floating;
        self.main_restore = main_restore;
        self.main_was_maximized = main_was_maximized;
        self.layout_applied = layout_applied;
    }
}

/// The dock-mode fields of [`PersistedPreviewState`], captured before a dock
/// attempt so a failed tiling can put every one of them back.
#[derive(Debug, Clone, Copy)]
struct DockFields {
    docked: bool,
    floating: Option<WindowGeometry>,
    main_restore: Option<WindowGeometry>,
    main_was_maximized: bool,
    layout_applied: bool,
}

/// A dock attempt that has been PREPARED but not applied.
///
/// Carries the geometry reads that only make sense before any window moves,
/// alongside the field values to restore if the tiling fails.
#[derive(Debug, Clone, Copy)]
struct StagedDock {
    previous: DockFields,
    /// True when this attempt is the one establishing the docked layout, so it
    /// owes the pre-dock snapshot - and, on failure, owes the physical undo.
    /// False for a redundant re-tile of a layout that is already live and
    /// already owns its snapshot.
    snapshot_owed: bool,
    main_geometry: Option<WindowGeometry>,
    main_was_maximized: bool,
    preview_geometry: Option<WindowGeometry>,
}

#[cfg(not(test))]
struct PreviewRuntime {
    path: PathBuf,
    state: PersistedPreviewState,
    current_url: Option<String>,
    events_wired: bool,
}

#[cfg(not(test))]
static PREVIEW_RUNTIME: OnceLock<Mutex<PreviewRuntime>> = OnceLock::new();
#[cfg(not(test))]
static RETILE_PENDING: AtomicBool = AtomicBool::new(false);

/// Serializes a whole dock/undock TRANSITION - the pre-dock reads, the geometry
/// calls and the settlement - against another transition running concurrently.
///
/// Deliberately a second mutex rather than [`PREVIEW_RUNTIME`]. The runtime
/// mutex is taken by the event-loop thread in the window-event handlers, and a
/// transition blocks on replies from that same thread, so it cannot be held
/// across the geometry work. This one is taken ONLY by the transition functions
/// - never by the event loop, whose handlers hop off onto a spawned task before
/// touching geometry - so holding it across those blocking reads is safe. It is
/// also strictly OUTERMOST: acquired before the runtime mutex and never while
/// holding it, so the two orders cannot invert.
#[cfg(not(test))]
static DOCK_TRANSITION: Mutex<()> = Mutex::new(());

#[cfg(not(test))]
fn runtime(app_state: &Arc<AppState>) -> &'static Mutex<PreviewRuntime> {
    PREVIEW_RUNTIME.get_or_init(|| {
        let path = app_state.storage.base_dir().join(PREVIEW_STATE_FILE);
        let state = std::fs::read_to_string(&path)
            .ok()
            .and_then(|raw| serde_json::from_str::<PersistedPreviewState>(&raw).ok())
            .unwrap_or_default();
        Mutex::new(PreviewRuntime {
            path,
            state,
            current_url: None,
            events_wired: false,
        })
    })
}

/// Best-effort persistence. A failure here costs the operator a remembered
/// window position, so it is logged rather than surfaced as a command error.
#[cfg(not(test))]
fn persist(runtime: &PreviewRuntime) {
    let Ok(serialized) = serde_json::to_string_pretty(&runtime.state) else {
        return;
    };
    let temp = runtime.path.with_extension("json.tmp");
    if let Err(error) = std::fs::write(&temp, serialized) {
        tracing::warn!("Failed to stage preview window state: {error}");
        return;
    }
    if let Err(error) = std::fs::rename(&temp, &runtime.path) {
        tracing::warn!("Failed to persist preview window state: {error}");
        let _ = std::fs::remove_file(&temp);
    }
}

/// What the main window needs to render the preview affordance.
#[cfg(not(test))]
#[derive(Debug, Clone, Serialize)]
pub struct PreviewStatus {
    /// The preview window currently exists.
    open: bool,
    /// The preview is tiled beside the main window.
    docked: bool,
    /// The URL the preview is showing right now.
    url: Option<String>,
    /// The last preview URL used for the session the caller asked about.
    session_url: Option<String>,
}

#[cfg(not(test))]
fn status_for(
    app: &AppHandle,
    runtime: &PreviewRuntime,
    session_id: Option<&str>,
) -> PreviewStatus {
    let window = app.get_webview_window(PREVIEW_WINDOW_LABEL);
    let open = window.is_some();

    PreviewStatus {
        open,
        docked: open && runtime.state.docked,
        url: if open {
            runtime
                .current_url
                .clone()
                .or_else(|| window.and_then(|w| w.url().ok()).map(|u| u.to_string()))
        } else {
            None
        },
        session_url: session_id.and_then(|id| runtime.state.session_urls.get(id).cloned()),
    }
}

#[cfg(not(test))]
fn emit_status(app: &AppHandle, status: &PreviewStatus) {
    let _ = app.emit_to(MAIN_WINDOW_LABEL, PREVIEW_STATUS_EVENT, status);
}

#[cfg(not(test))]
fn publish_status(
    app: &AppHandle,
    app_state: &Arc<AppState>,
    session_id: Option<&str>,
) -> PreviewStatus {
    let status = {
        let guard = runtime(app_state).lock();
        status_for(app, &guard, session_id)
    };
    emit_status(app, &status);
    status
}

#[cfg(not(test))]
fn require_window(app: &AppHandle, label: &str) -> Result<WebviewWindow, String> {
    app.get_webview_window(label)
        .ok_or_else(|| format!("The {label} window is not open"))
}

#[cfg(not(test))]
fn read_geometry(window: &WebviewWindow) -> Option<WindowGeometry> {
    let position = window.outer_position().ok()?;
    let size = window.outer_size().ok()?;
    let scale = window.scale_factor().ok()?;
    if !scale.is_finite() || scale <= 0.0 {
        return None;
    }
    Some(WindowGeometry {
        x: position.x,
        y: position.y,
        width: f64::from(size.width) / scale,
        height: f64::from(size.height) / scale,
    })
}

/// Apply a geometry that was remembered from an earlier run, but only if some
/// monitor still covers its top-left corner. Otherwise the window keeps whatever
/// position the OS gave it, which is at least visible.
#[cfg(not(test))]
fn apply_remembered_geometry(window: &WebviewWindow, geometry: WindowGeometry) {
    let work_areas: Vec<(i32, i32, u32, u32)> = window
        .available_monitors()
        .unwrap_or_default()
        .iter()
        .map(|monitor| {
            let area = monitor.work_area();
            (
                area.position.x,
                area.position.y,
                area.size.width,
                area.size.height,
            )
        })
        .collect();

    if !position_is_on_a_monitor(geometry.x, geometry.y, &work_areas) {
        tracing::info!(
            "Ignoring remembered window position ({}, {}): no connected monitor covers it",
            geometry.x,
            geometry.y
        );
        return;
    }

    if let Err(error) = apply_geometry(window, geometry) {
        tracing::warn!("Failed to restore remembered window geometry: {error}");
    }
}

#[cfg(not(test))]
fn apply_geometry(window: &WebviewWindow, geometry: WindowGeometry) -> Result<(), String> {
    // Position first: the logical size is resolved against the scale factor of
    // whichever monitor the window is on, so it must land on the target monitor
    // before the size is applied.
    window
        .set_position(PhysicalPosition::new(geometry.x, geometry.y))
        .map_err(|error| format!("Failed to position window: {error}"))?;
    window
        .set_size(LogicalSize::new(geometry.width, geometry.height))
        .map_err(|error| format!("Failed to resize window: {error}"))
}

/// Tile the main window and the preview side by side across the current
/// monitor's work area.
#[cfg(not(test))]
fn enter_docked_mode(app: &AppHandle, app_state: &Arc<AppState>) -> Result<(), String> {
    // Serialize the whole transition, held across the reads, the geometry calls
    // and the settlement. Both callers are async commands on the tokio pool and
    // nothing else serializes them; the state is only ever consistent between
    // one attempt's staging and its own settlement.
    //
    // Without it two overlapping attempts both stage a snapshot debt while no
    // layout is live yet: the loser's rollback persists the un-docked fields
    // over the winner's commit and the physical undo un-tiles it, and a second
    // attempt that succeeds records the already-tiled column as `main_restore`,
    // stranding Hive Manager in the left-hand column on the next teardown.
    //
    // Serializing rather than rejecting is the point. A rejected second attempt
    // returns Err to the reopen path, which reads any Err as "this display
    // cannot host the split" and clears the operator's dock preference -
    // reintroducing the clobber. Serializing instead degrades the loser into an
    // ordinary redundant re-tile, which the staging and commit gates already
    // handle.
    let _transition = DOCK_TRANSITION.lock();

    let main = require_window(app, MAIN_WINDOW_LABEL)?;
    let preview = require_window(app, PREVIEW_WINDOW_LABEL)?;

    // `match` rather than `.or(..)`: `.or` is EAGER, so it would call
    // `primary_monitor()` on every dock even when the current monitor is
    // already in hand - and its `?` would then propagate a teardown-race error
    // out of a dock that had everything it needed. The fallback is a cold path.
    let monitor = match main
        .current_monitor()
        .map_err(|error| format!("Failed to read the current monitor: {error}"))?
    {
        Some(monitor) => Some(monitor),
        None => main
            .primary_monitor()
            .map_err(|error| format!("Failed to read the primary monitor: {error}"))?,
    }
    .ok_or_else(|| "Could not determine which monitor Hive Manager is on".to_string())?;

    let scale = monitor.scale_factor();
    let work_area = *monitor.work_area();

    // Computed before anything is mutated: a display that cannot host the split
    // must leave both windows exactly where they were.
    let (main_width, preview_width) = dock_split(work_area.size.width, scale)?;

    // Every read that only makes sense BEFORE a window moves, taken while the
    // state is still completely untouched.
    //
    // Taken OUTSIDE the lock deliberately: each of these is a blocking
    // round-trip to the event-loop thread (tauri-runtime-wry's `window_getter!`
    // posts a user message and blocks on `rx.recv()` with no timeout), and that
    // same thread takes this mutex in the window-event handlers below. This is
    // an async command, so it never short-circuits onto the main thread -
    // holding the lock across these reads deadlocks the event loop against the
    // command with no way out but killing the process.
    let main_geometry = read_geometry(&main);
    let main_was_maximized = main.is_maximized().unwrap_or(false);
    let preview_geometry = read_geometry(&preview);

    // `stage_dock` borrows the state immutably, so nothing here can commit the
    // docked state early.
    let staged = {
        let guard = runtime(app_state).lock();
        guard
            .state
            .stage_dock(main_geometry, main_was_maximized, preview_geometry)
    };

    let tiled = tile_docked_windows(
        &main,
        &preview,
        work_area.position,
        work_area.size.height,
        main_width,
        preview_width,
    );

    // Commit or roll back off the tiling RESULT. The lock is deliberately not
    // held across the geometry calls above: the preview's own window-event
    // handler takes it too.
    {
        let mut guard = runtime(app_state).lock();
        guard.state.settle_dock(staged, &tiled);
        persist(&guard);
    }

    if let Err(error) = tiled {
        // The state is already back where it started; put the windows back too.
        // Best effort, and deliberately after the rollback so the ORIGINAL
        // cause below is what the operator sees.
        undo_failed_dock(&main, &preview, staged);
        return Err(error);
    }

    Ok(())
}

/// Issue the geometry calls that put the two windows into their columns.
///
/// Split out from [`enter_docked_mode`] so the dock state machine settles on
/// its RESULT: nothing in here touches the persisted state, which is what makes
/// "tile, then commit" an ordering the caller cannot get wrong.
#[cfg(not(test))]
fn tile_docked_windows(
    main: &WebviewWindow,
    preview: &WebviewWindow,
    origin: PhysicalPosition<i32>,
    height: u32,
    main_width: u32,
    preview_width: u32,
) -> Result<(), String> {
    // A maximized window ignores explicit geometry, so drop out of it first.
    if main.is_maximized().unwrap_or(false) {
        let _ = main.unmaximize();
    }
    if preview.is_maximized().unwrap_or(false) {
        let _ = preview.unmaximize();
    }
    let _ = preview.set_min_size(Some(LogicalSize::new(
        DOCKED_PREVIEW_MIN_LOGICAL_WIDTH,
        DOCKED_PREVIEW_MIN_LOGICAL_HEIGHT,
    )));

    main.set_position(PhysicalPosition::new(origin.x, origin.y))
        .map_err(|error| format!("Failed to position Hive Manager: {error}"))?;
    main.set_size(PhysicalSize::new(main_width, height))
        .map_err(|error| format!("Failed to resize Hive Manager: {error}"))?;
    preview
        .set_position(PhysicalPosition::new(origin.x + main_width as i32, origin.y))
        .map_err(|error| format!("Failed to position the preview: {error}"))?;
    preview
        .set_size(PhysicalSize::new(preview_width, height))
        .map_err(|error| format!("Failed to resize the preview: {error}"))
}

/// Best-effort physical undo of a dock whose tiling failed, run after
/// [`PersistedPreviewState::settle_dock`] has already rolled the state back.
///
/// Every step logs and continues rather than propagating: the caller still owes
/// the operator the ORIGINAL tiling error, and a secondary failure in here must
/// never mask it.
#[cfg(not(test))]
fn undo_failed_dock(main: &WebviewWindow, preview: &WebviewWindow, staged: StagedDock) {
    // Gated on the LAYOUT fact rather than the dock PREFERENCE: when a docked
    // layout was already live this attempt was a redundant re-tile, so it owns
    // none of the side effects below and the live layout - along with the
    // restore snapshot that is the only way back out of it - is left alone.
    if !staged.snapshot_owed {
        return;
    }

    if let Err(error) = preview.set_min_size(Some(LogicalSize::new(
        FLOATING_PREVIEW_MIN_LOGICAL_WIDTH,
        FLOATING_PREVIEW_MIN_LOGICAL_HEIGHT,
    ))) {
        tracing::warn!(
            "Failed to relax the docked preview minimum size after a failed dock: {error}"
        );
    }

    if let Some(geometry) = staged.preview_geometry {
        if let Err(error) = apply_geometry(preview, geometry) {
            tracing::warn!("Failed to put the preview back after a failed dock: {error}");
        }
    }

    if staged.main_was_maximized {
        if let Err(error) = main.maximize() {
            tracing::warn!("Failed to re-maximize Hive Manager after a failed dock: {error}");
        }
    } else if let Some(geometry) = staged.main_geometry {
        if let Err(error) = apply_geometry(main, geometry) {
            tracing::warn!("Failed to put Hive Manager back after a failed dock: {error}");
        }
    }
}

/// Give the main window back the geometry it had before docking. Split out from
/// [`leave_docked_mode`] because closing a docked preview must also do it -
/// otherwise Hive Manager is stranded in the left-hand column with nothing
/// beside it.
#[cfg(not(test))]
fn restore_main_window_geometry(
    app: &AppHandle,
    app_state: &Arc<AppState>,
) -> Result<(), String> {
    let (geometry, was_maximized) = {
        let mut guard = runtime(app_state).lock();
        let taken = guard.state.take_main_restore();
        persist(&guard);
        taken
    };

    let Some(main) = app.get_webview_window(MAIN_WINDOW_LABEL) else {
        return Ok(());
    };

    if was_maximized {
        main.maximize()
            .map_err(|error| format!("Failed to restore Hive Manager: {error}"))?;
    } else if let Some(geometry) = geometry {
        apply_remembered_geometry(&main, geometry);
    }

    Ok(())
}

/// Undo the docked LAYOUT on teardown while keeping the dock PREFERENCE, so the
/// next open re-tiles.
///
/// Both teardown paths funnel through here - the `close_preview_window` command
/// AND the native titlebar X, which reaches only `WindowEvent::Destroyed`
/// (there is no `CloseRequested` handler in this crate). Closing a docked
/// preview with the X used to skip the restore entirely, stranding Hive Manager
/// in the left-hand column beside empty desktop - and since the emitted
/// `open: false` hides the dock/pop-out/close cluster, no in-app control was
/// left to undo it.
#[cfg(not(test))]
fn undock_layout_after_teardown(
    app: &AppHandle,
    app_state: &Arc<AppState>,
) -> Result<(), String> {
    // Same serializing lock as the dock, so a teardown cannot interleave with a
    // dock that is mid-transition. Reachable concurrently with one: the titlebar
    // X gets here through a task spawned off `WindowEvent::Destroyed`, which the
    // frontend's dock-button busy flag does not cover. Never held by the event
    // loop itself - that arm spawns before reaching this - and taken here before
    // the runtime mutex, matching the dock's lock order.
    let _transition = DOCK_TRANSITION.lock();

    if !runtime(app_state).lock().state.docked {
        return Ok(());
    }
    restore_main_window_geometry(app, app_state)
}

#[cfg(not(test))]
fn leave_docked_mode(app: &AppHandle, app_state: &Arc<AppState>) -> Result<(), String> {
    // Serialized against the dock for the same reason as the teardown above: an
    // undock that interleaves with a dock in flight consumes the restore
    // snapshot the dock is still staging against. Taken before the runtime
    // mutex; `restore_main_window_geometry` below takes only that one, so the
    // order holds.
    let _transition = DOCK_TRANSITION.lock();

    restore_main_window_geometry(app, app_state)?;

    let floating = {
        let mut guard = runtime(app_state).lock();
        guard.state.docked = false;
        persist(&guard);
        guard.state.floating
    };

    let Some(preview) = app.get_webview_window(PREVIEW_WINDOW_LABEL) else {
        return Ok(());
    };

    let _ = preview.set_min_size(Some(LogicalSize::new(
        FLOATING_PREVIEW_MIN_LOGICAL_WIDTH,
        FLOATING_PREVIEW_MIN_LOGICAL_HEIGHT,
    )));

    if let Some(geometry) = floating {
        apply_remembered_geometry(&preview, geometry);
    }

    Ok(())
}

/// Keep the docked preview flush against the main window after the operator
/// moves, resizes or DPI-shifts it.
///
/// This is a deliberately ONE-WAY follow: the preview tracks the main window and
/// never the other way round. Resizing the main window here would emit another
/// `Resized` and recurse.
#[cfg(not(test))]
fn follow_docked_preview(app: &AppHandle) -> Result<(), String> {
    let main = require_window(app, MAIN_WINDOW_LABEL)?;
    let preview = require_window(app, PREVIEW_WINDOW_LABEL)?;

    let position = main
        .outer_position()
        .map_err(|error| format!("Failed to read the Hive Manager position: {error}"))?;
    let size = main
        .outer_size()
        .map_err(|error| format!("Failed to read the Hive Manager size: {error}"))?;
    let monitor = main
        .current_monitor()
        .map_err(|error| format!("Failed to read the current monitor: {error}"))?
        .ok_or_else(|| "Could not determine which monitor Hive Manager is on".to_string())?;

    let scale = monitor.scale_factor();
    let work_area = *monitor.work_area();

    let floor = (DOCKED_PREVIEW_MIN_LOGICAL_WIDTH * scale).round() as i32;
    let work_right = work_area.position.x + work_area.size.width as i32;
    let main_right = position.x + size.width as i32;
    let available = work_right - main_right;

    let (x, width) = if available >= floor {
        (main_right, available)
    } else {
        // Degenerate case: the operator widened the main window past the work
        // area minus the floor. Keep the preview on-screen at its minimum rather
        // than pushing it off the desktop.
        (work_right - floor, floor)
    };

    preview
        .set_position(PhysicalPosition::new(x, position.y))
        .map_err(|error| format!("Failed to position the preview: {error}"))?;
    preview
        .set_size(PhysicalSize::new(width.max(1) as u32, size.height))
        .map_err(|error| format!("Failed to resize the preview: {error}"))?;

    Ok(())
}

/// Wire the window listeners that keep the dock aligned and the persisted
/// geometry current.
///
/// The main-window listener lives for the whole process, so it is installed at
/// most once. The preview-window listener dies with its window, so it is
/// re-attached to every preview window - a shared "already wired" flag here
/// would silently leave a re-opened preview unwatched.
#[cfg(not(test))]
fn wire_window_events(app: &AppHandle, app_state: &Arc<AppState>, preview: &WebviewWindow) {
    let wire_main = {
        let mut guard = runtime(app_state).lock();
        let first = !guard.events_wired;
        guard.events_wired = true;
        first
    };

    if let Some(main) = app.get_webview_window(MAIN_WINDOW_LABEL).filter(|_| wire_main) {
        let follow_app = app.clone();
        let follow_state = Arc::clone(app_state);
        main.on_window_event(move |event| {
            // `ScaleFactorChanged` is handled alongside `Resized`/`Moved` on
            // purpose. On a mixed-DPI multi-monitor desktop the main window's
            // physical rectangle changes when it crosses monitors without a
            // reliable Moved/Resized pair, and a preview that ignored the DPI
            // event would drift out of alignment or overlap.
            if !matches!(
                event,
                WindowEvent::Resized(_)
                    | WindowEvent::Moved(_)
                    | WindowEvent::ScaleFactorChanged { .. }
            ) {
                return;
            }

            if !runtime(&follow_state).lock().state.docked {
                return;
            }

            // Coalesce the event storm a window drag produces, and get off the
            // window-event callback before touching window geometry.
            if RETILE_PENDING.swap(true, Ordering::SeqCst) {
                return;
            }
            let retile_app = follow_app.clone();
            let retile_state = Arc::clone(&follow_state);
            tauri::async_runtime::spawn(async move {
                tokio::time::sleep(std::time::Duration::from_millis(RETILE_DEBOUNCE_MS)).await;
                RETILE_PENDING.store(false, Ordering::SeqCst);
                if !runtime(&retile_state).lock().state.docked {
                    return;
                }
                if let Err(error) = follow_docked_preview(&retile_app) {
                    tracing::debug!("Preview dock follow skipped: {error}");
                }
            });
        });
    }

    let preview_app = app.clone();
    let preview_state = Arc::clone(app_state);
    preview.on_window_event(move |event| match event {
        WindowEvent::Moved(_) | WindowEvent::Resized(_) => {
            let Some(window) = preview_app.get_webview_window(PREVIEW_WINDOW_LABEL) else {
                return;
            };
            let Some(geometry) = read_geometry(&window) else {
                return;
            };
            // Held in memory and flushed on close/dock/undock rather than
            // written on every frame of a drag.
            let mut guard = runtime(&preview_state).lock();
            if !guard.state.docked {
                guard.state.floating = Some(geometry);
            }
        }
        WindowEvent::Destroyed => {
            let docked = {
                let mut guard = runtime(&preview_state).lock();
                guard.current_url = None;
                persist(&guard);
                guard.state.docked
            };

            // The native titlebar X lands here and nowhere else, so this arm
            // has to mirror `close_preview_window` or a docked close strands
            // the main window. Same discipline as the retile follow above: get
            // off the window-event callback before touching window geometry.
            // Idempotent when the command already restored - the geometry it
            // takes is gone, so this second pass is a no-op.
            if docked {
                let restore_app = preview_app.clone();
                let restore_state = Arc::clone(&preview_state);
                tauri::async_runtime::spawn(async move {
                    let restored = undock_layout_after_teardown(&restore_app, &restore_state);
                    if let Err(error) = restored {
                        tracing::warn!(
                            "Failed to restore Hive Manager after the preview closed: {error}"
                        );
                    }
                });
            }

            emit_status(
                &preview_app,
                &PreviewStatus {
                    open: false,
                    docked: false,
                    url: None,
                    session_url: None,
                },
            );
        }
        _ => {}
    });
}

#[cfg(not(test))]
fn create_preview_window(
    app: &AppHandle,
    app_state: &Arc<AppState>,
    url: Url,
    configured_api_port: u16,
) -> Result<WebviewWindow, String> {
    let mut config = app
        .config()
        .app
        .windows
        .iter()
        .find(|config| config.label == PREVIEW_WINDOW_LABEL)
        .cloned()
        .ok_or_else(|| "Preview window configuration is missing".to_string())?;
    config.url = WebviewUrl::External(url);
    // Realised hidden so the remembered geometry can be applied before the
    // operator sees the window at the configured default position.
    config.visible = false;

    let navigation_app = app.clone();
    let navigation_state = Arc::clone(app_state);

    let mut builder = WebviewWindowBuilder::from_config(app, &config)
        .map_err(|error| format!("Failed to configure preview window: {error}"))?
        .on_navigation(move |url| {
            if !preview_navigation_allowed(url, configured_api_port) {
                return false;
            }
            // Address-bar sync: this closure already sees every navigation, so
            // the main window is told where in-page navigation actually went.
            let current = url.to_string();
            runtime(&navigation_state).lock().current_url = Some(current.clone());
            let _ = navigation_app.emit_to(
                MAIN_WINDOW_LABEL,
                PREVIEW_NAVIGATED_EVENT,
                serde_json::json!({ "url": current }),
            );
            true
        })
        .on_new_window(|_, _| NewWindowResponse::Deny);

    // Owner semantics rather than a child webview. Verified against the
    // `WebviewWindowBuilder::parent` doc contract in tauri 2.10.1: on Windows the
    // parent is set as the OWNER window, so the preview stays above Hive Manager
    // in the z-order, is destroyed automatically when Hive Manager closes, and is
    // hidden when Hive Manager minimizes. No `unstable` cargo feature, no child
    // webview, and no widening of the capability boundary.
    if let Some(main) = app.get_webview_window(MAIN_WINDOW_LABEL) {
        builder = builder
            .parent(&main)
            .map_err(|error| format!("Failed to parent the preview window: {error}"))?;
    }

    let window = builder
        .build()
        .map_err(|error| format!("Failed to open preview window: {error}"))?;

    wire_window_events(app, app_state, &window);

    let floating = { runtime(app_state).lock().state.floating };
    if let Some(geometry) = floating {
        apply_remembered_geometry(&window, geometry);
    }

    window
        .show()
        .map_err(|error| format!("Failed to show preview window: {error}"))?;

    Ok(window)
}

// ---------------------------------------------------------------------------
// Commands
// ---------------------------------------------------------------------------

/// Open the operator preview, or navigate and focus the existing preview window.
///
/// The stable label provides deduplication and is also the ACL boundary referenced by
/// `capabilities/operator-preview.json`. Pop-up windows and non-web navigations are
/// denied so untrusted content cannot escape into a differently labelled webview.
#[cfg(not(test))]
#[tauri::command]
pub async fn open_preview_window(
    app: AppHandle,
    app_state: State<'_, Arc<AppState>>,
    url: String,
    session_id: Option<String>,
) -> Result<PreviewStatus, String> {
    let configured_api_port = app_state.config.read().await.api.port;
    // The one and only normalizing entry point: this URL was typed by a human.
    let url = validate_operator_preview_input(&url, configured_api_port)?;
    let state = Arc::clone(app_state.inner());

    {
        let mut guard = runtime(&state).lock();
        guard.current_url = Some(url.to_string());
        if let Some(session_id) = session_id.as_deref() {
            guard
                .state
                .session_urls
                .insert(session_id.to_string(), url.to_string());
        }
        persist(&guard);
    }

    if let Some(window) = app.get_webview_window(PREVIEW_WINDOW_LABEL) {
        window
            .navigate(url)
            .map_err(|error| format!("Failed to navigate preview window: {error}"))?;
        window
            .unminimize()
            .map_err(|error| format!("Failed to restore preview window: {error}"))?;
        window
            .show()
            .map_err(|error| format!("Failed to show preview window: {error}"))?;
        window
            .set_focus()
            .map_err(|error| format!("Failed to focus preview window: {error}"))?;
    } else {
        let window = create_preview_window(&app, &state, url, configured_api_port)?;

        // The persisted dock preference survives across opens, so a freshly
        // created window re-tiles itself. A display that can no longer host the
        // split downgrades to floating instead of failing the open.
        if runtime(&state).lock().state.docked {
            if let Err(error) = enter_docked_mode(&app, &state) {
                tracing::warn!("Preview opened floating instead of docked: {error}");
                let mut guard = runtime(&state).lock();
                guard.state.docked = false;
                persist(&guard);
            }
        }

        window
            .set_focus()
            .map_err(|error| format!("Failed to focus preview window: {error}"))?;
    }

    Ok(publish_status(&app, &state, session_id.as_deref()))
}

/// Current preview state, plus the last URL used for `session_id` if the caller
/// supplies one. Lets the main window prefill its address bar per session.
#[cfg(not(test))]
#[tauri::command]
pub async fn get_preview_status(
    app: AppHandle,
    app_state: State<'_, Arc<AppState>>,
    session_id: Option<String>,
) -> Result<PreviewStatus, String> {
    let state = Arc::clone(app_state.inner());
    let guard = runtime(&state).lock();
    Ok(status_for(&app, &guard, session_id.as_deref()))
}

/// Tile the preview beside the main window.
#[cfg(not(test))]
#[tauri::command]
pub async fn dock_preview_window(
    app: AppHandle,
    app_state: State<'_, Arc<AppState>>,
    session_id: Option<String>,
) -> Result<PreviewStatus, String> {
    let state = Arc::clone(app_state.inner());
    enter_docked_mode(&app, &state)?;
    Ok(publish_status(&app, &state, session_id.as_deref()))
}

/// Return the preview to a free-floating window at its remembered geometry, and
/// give the main window back the size it had before docking.
#[cfg(not(test))]
#[tauri::command]
pub async fn undock_preview_window(
    app: AppHandle,
    app_state: State<'_, Arc<AppState>>,
    session_id: Option<String>,
) -> Result<PreviewStatus, String> {
    let state = Arc::clone(app_state.inner());
    leave_docked_mode(&app, &state)?;
    Ok(publish_status(&app, &state, session_id.as_deref()))
}

/// Reload whatever the preview is currently showing.
#[cfg(not(test))]
#[tauri::command]
pub async fn reload_preview_window(app: AppHandle) -> Result<(), String> {
    require_window(&app, PREVIEW_WINDOW_LABEL)?
        .reload()
        .map_err(|error| format!("Failed to reload the preview: {error}"))
}

/// Close the preview window, persisting its geometry first.
#[cfg(not(test))]
#[tauri::command]
pub async fn close_preview_window(
    app: AppHandle,
    app_state: State<'_, Arc<AppState>>,
    session_id: Option<String>,
) -> Result<PreviewStatus, String> {
    let state = Arc::clone(app_state.inner());

    if let Some(window) = app.get_webview_window(PREVIEW_WINDOW_LABEL) {
        {
            let mut guard = runtime(&state).lock();
            if !guard.state.docked {
                if let Some(geometry) = read_geometry(&window) {
                    guard.state.floating = Some(geometry);
                }
            }
        }

        // The dock *preference* survives the close (so the next open re-tiles);
        // only the layout is undone here. Shared with the `Destroyed` arm so the
        // titlebar X cannot diverge from this command.
        undock_layout_after_teardown(&app, &state)?;

        window
            .destroy()
            .map_err(|error| format!("Failed to close preview window: {error}"))?;
    }

    {
        let mut guard = runtime(&state).lock();
        guard.current_url = None;
        persist(&guard);
    }

    Ok(publish_status(&app, &state, session_id.as_deref()))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The operator-typed path: normalization THEN the rules.
    fn validate(input: &str) -> Result<Url, String> {
        validate_operator_preview_input(input, LOCAL_API_PORT)
    }

    /// The untrusted navigation gate: the rules, and nothing but the rules.
    fn navigable(input: &str) -> bool {
        let url = Url::parse(input)
            .unwrap_or_else(|error| panic!("{input:?} should parse as a URL, got {error:?}"));
        preview_navigation_allowed(&url, LOCAL_API_PORT)
    }

    /// Every scheme verified to reach an OS protocol handler through the
    /// `://`-only scheme test. `ms-msdt:` is the Follina launcher.
    const OS_PROTOCOL_HANDLER_URLS: &[&str] = &[
        "vscode:/x",
        "steam:/run/1",
        "ms-settings:/",
        "javascript:0",
        "tel:12345",
        "ms-msdt:/id PCWDiagnostic /skip force /param IT_LaunchMethod=ContextMenu",
    ];

    /// Reserved-API-port spellings WHATWG special-scheme parsing accepts.
    const SLASH_VARIANT_API_PORT_URLS: &[&str] = &[
        "http:/localhost:18800",
        "https:/localhost:18800",
        r"http:\\localhost:18800",
        r"http:/\localhost:18800",
        "http:/127.0.0.1:18800",
    ];

    #[test]
    fn accepts_http_and_https_urls_with_hosts() {
        let local = validate("  http://localhost:5173/dashboard?tab=1#agents  ")
            .expect("localhost dev URL should be allowed");
        assert_eq!(
            local.as_str(),
            "http://localhost:5173/dashboard?tab=1#agents"
        );

        let pull_request = validate("https://github.com/acme/repo/pull/42")
            .expect("GitHub pull request URL should be allowed");
        assert_eq!(
            pull_request.as_str(),
            "https://github.com/acme/repo/pull/42"
        );
    }

    #[test]
    fn rejects_empty_relative_and_hostless_urls() {
        for input in ["", "   ", "/relative/path", "http://"] {
            assert!(
                validate(input).is_err(),
                "{input:?} should be rejected"
            );
        }
    }

    #[test]
    fn rejects_non_web_schemes() {
        for input in [
            "file:///tmp/index.html",
            "javascript:alert(1)",
            "data:text/html,<h1>preview</h1>",
            "ftp://example.com/file",
            "about:blank",
        ] {
            assert!(
                validate(input).is_err(),
                "{input:?} should be rejected"
            );
        }
    }

    #[test]
    fn rejects_embedded_credentials() {
        assert!(validate("https://user:secret@example.com/private").is_err());
    }

    #[test]
    fn rejects_local_hive_api_but_allows_other_dev_ports() {
        for input in [
            "http://localhost:18800/api/health",
            "http://127.0.0.1:18800/api/health",
            "http://0.0.0.0:18800/api/health",
            "http://[::1]:18800/api/health",
            "http://[::ffff:127.0.0.1]:18800/api/health",
            "http://localhost.:18800/api/health",
            "http://preview.localhost:18800/api/health",
            "http://example.com:18800/api/health",
        ] {
            assert!(
                validate(input).is_err(),
                "{input:?} should be rejected"
            );
        }

        assert!(validate("http://localhost:3000").is_ok());
        assert!(validate("http://127.0.0.1:5173").is_ok());
    }

    #[test]
    fn rejects_the_configured_api_port() {
        assert!(
            validate_preview_url("http://localhost:19999/health", 19_999).is_err()
        );
        assert!(
            validate_preview_url("https://example.com:19999/", 19_999).is_err()
        );
        assert!(validate_preview_url("http://localhost/health", 80).is_err());
    }

    #[test]
    fn rejects_origins_reserved_for_the_trusted_main_window() {
        for input in [
            "http://localhost:1420",
            "http://tauri.localhost",
            "https://tauri.localhost/app",
        ] {
            assert!(validate(input).is_err(), "{input:?} should be rejected");
        }

        assert!(validate("http://127.0.0.1:1420").is_ok());
        assert!(validate("http://localhost:5173").is_ok());
    }

    #[test]
    fn navigation_policy_reuses_the_same_validation_boundary() {
        let allowed = Url::parse("https://example.com/next").unwrap();
        let blocked_scheme = Url::parse("file:///tmp/secret").unwrap();
        let blocked_api = Url::parse("http://localhost:18800/api/health").unwrap();

        assert!(preview_navigation_allowed(&allowed, LOCAL_API_PORT));
        assert!(!preview_navigation_allowed(
            &blocked_scheme,
            LOCAL_API_PORT
        ));
        assert!(!preview_navigation_allowed(&blocked_api, LOCAL_API_PORT));
    }

    /// Architectural lock for finding 1: the untrusted navigation gate must
    /// never normalize.
    ///
    /// A `true` from `preview_navigation_allowed` lets wry navigate to the
    /// ORIGINAL url, so any scheme the gate accepts is a scheme a page inside
    /// the preview can reach - including OS protocol handlers. This asserts on
    /// the gate itself rather than on `has_explicit_scheme`, so the gate cannot
    /// regain normalization even if the lexical scan is rewritten again.
    #[test]
    fn the_navigation_gate_never_normalizes() {
        for input in OS_PROTOCOL_HANDLER_URLS {
            assert!(
                !navigable(input),
                "{input:?} reaches an OS protocol handler and must be denied \
                 by the navigation gate"
            );
            assert!(
                validate_preview_url(input, LOCAL_API_PORT).is_err(),
                "{input:?} must be rejected by the rules themselves"
            );
        }

        // The operator path normalizes, but must reach the same verdict.
        for input in OS_PROTOCOL_HANDLER_URLS {
            assert!(
                validate(input).is_err(),
                "{input:?} must be rejected on the operator path too"
            );
        }

        // The sharp end of Layer 1, and the part that does NOT depend on the
        // lexical scan being right. These genuinely ARE the `host:port` shape,
        // so the operator path is correct to normalize them - but arriving at
        // the gate the very same text is a real scheme with an opaque body, and
        // a `true` there sends wry to the ORIGINAL string and into whatever
        // handler owns `com.evil.app:`. The two paths must disagree here.
        for input in ["localhost:5173", "example.com:80", "com.evil.app:12345"] {
            let parsed = Url::parse(input)
                .unwrap_or_else(|error| panic!("{input:?} should parse, got {error:?}"));
            assert_eq!(
                parsed.as_str(),
                input,
                "sanity: {input:?} reaches the gate as a scheme, unmodified"
            );
            assert!(
                !navigable(input),
                "{input:?} must be denied by the gate - normalizing there would \
                 launder a custom scheme into https and navigate to the original"
            );
            assert!(
                validate(input).is_ok(),
                "{input:?} is exactly the forgiving entry an operator types, \
                 and must still work on the operator path"
            );
        }

        // Sanity: the gate still passes ordinary web content.
        assert!(navigable("https://example.com/next"));
        assert!(navigable("http://localhost:5173/dashboard"));
    }

    /// Regression lock for finding 2.
    ///
    /// `Url::parse` implements WHATWG special-scheme parsing and accepts a
    /// scheme followed by zero, one or many `/` or `\`. Under the `://` scheme
    /// test `http:/localhost:18800` was classified scheme-less, became
    /// `https://http:/localhost:18800` (host `http`, port 443, path
    /// `/localhost:18800`), and the reserved-API-port guard flipped from reject
    /// to accept.
    #[test]
    fn slash_variant_spellings_still_hit_the_reserved_api_port() {
        for input in SLASH_VARIANT_API_PORT_URLS {
            // Whatever the slashes, this really is the Hive API origin.
            let parsed = Url::parse(input)
                .unwrap_or_else(|error| panic!("{input:?} should parse, got {error:?}"));
            assert_eq!(
                parsed.port_or_known_default(),
                Some(LOCAL_API_PORT),
                "sanity: {input:?} really does resolve to the reserved API port"
            );

            assert!(!navigable(input), "{input:?} must be denied by the gate");
            assert!(
                validate(input).is_err(),
                "{input:?} must be rejected on the operator path"
            );
        }

        // Same trick against the trusted app origin rather than the API port.
        assert!(!navigable("http:/tauri.localhost"));
        assert!(validate("http:/tauri.localhost").is_err());

        // And the plain spellings are unchanged.
        assert!(validate("http://localhost:18800").is_err());
        assert!(!navigable("http://localhost:18800"));
    }

    /// Regression lock for finding 3.
    ///
    /// The url crate PRESERVES a trailing FQDN dot, so these spellings reach the
    /// very same origin as the bare ones while comparing unequal to them.
    #[test]
    fn trusted_main_window_origin_folds_trailing_fqdn_dots() {
        for input in [
            "http://tauri.localhost.",
            "https://tauri.localhost./app",
            "http://tauri.localhost..",
            "http://localhost.:1420",
            "http://LOCALHOST.:1420",
            "http://localhost..:1420",
        ] {
            let url = Url::parse(input)
                .unwrap_or_else(|error| panic!("{input:?} should parse, got {error:?}"));
            assert!(
                is_trusted_main_window_origin(&url),
                "{input:?} is the trusted app origin in FQDN-absolute spelling"
            );
            assert!(
                validate(input).is_err(),
                "{input:?} should be rejected on the operator path"
            );
            assert!(!navigable(input), "{input:?} should be denied by the gate");
        }

        // Sanity: the dot really is preserved, which is what made this a bypass.
        assert_eq!(
            Url::parse("http://tauri.localhost.").unwrap().host_str(),
            Some("tauri.localhost."),
            "sanity: the url crate keeps the trailing dot"
        );

        // Folding dots must not over-match: the dev-server port still gates the
        // bare `localhost` case, and unrelated hosts stay allowed.
        assert!(validate("http://localhost.:5173").is_ok());
        assert!(validate("http://127.0.0.1:1420").is_ok());
    }

    // -- issue #157 §1: forgiving URL entry ---------------------------------

    #[test]
    fn scheme_detection_is_lexical_not_parse_based() {
        // Parses successfully with the bogus scheme `localhost`.
        assert!(!has_explicit_scheme("localhost:5173"));
        // Fails to parse outright.
        assert!(!has_explicit_scheme("127.0.0.1:5173"));
        assert!(!has_explicit_scheme("[::1]:5173"));
        assert!(!has_explicit_scheme("example.com"));
        assert!(!has_explicit_scheme("//localhost:3000/api"));
        // A `:` after a path separator is not a scheme delimiter.
        assert!(!has_explicit_scheme("example.com/a:b//c"));

        assert!(has_explicit_scheme("http://example.com"));
        assert!(has_explicit_scheme("HTTPS://example.com"));
        assert!(has_explicit_scheme("file:///tmp/x"));
        assert!(has_explicit_scheme("ftp://example.com"));
        assert!(has_explicit_scheme("x+y-z.1://example.com"));
    }

    /// Regression lock for the `://` scheme test (findings 1 and 2).
    ///
    /// Requiring a double slash classified every opaque and single-slash scheme
    /// as SCHEME-LESS, so `https://` was prepended and the result passed the
    /// allowlist as "https". Accepting any `scheme:` instead would re-break
    /// `localhost:5173`. A scheme prefix therefore wins unless the input is
    /// unambiguously `host:port`. This is the full trace table.
    #[test]
    fn explicit_scheme_detection_requires_a_host_port_shape() {
        // SCHEME-LESS: the forgiving `host:port` entry the feature exists for.
        for input in [
            "localhost:5173",     // known local single-label name
            "LOCALHOST:18800",    // ... case-insensitively
            "localhost.:18800",   // ... and its FQDN-absolute spelling
            "127.0.0.1:3000/x",   // IPv4: caught by the leading-digit guard
            "[::1]:8080",         // IPv6: caught by the leading-`[` guard
            "example.com:80",     // multi-label name
            "foo.localhost:3000", //
            "example.com:8443/path?q=1#frag",
            "github.com/rdfitted/hive-manager", // no colon at all
            "//localhost:18800/api",            // `/` inside the scheme candidate
        ] {
            assert!(
                !has_explicit_scheme(input),
                "{input:?} is operator `host:port` entry and must be scheme-less"
            );
        }

        // EXPLICIT: a real scheme, handed to `Url::parse` untouched so the
        // allowlist can reject it.
        for input in [
            "about:blank",
            "javascript:alert(1)",
            "javascript:0",   // digit body, but `javascript` is not a hostname
            "tel:12345",      // the shape that forbids "any `scheme:` is explicit"
            "mailto:1",       //
            "svn+ssh:22",     // `+` is legal in a scheme, never in a hostname
            "vscode:/x",      // non-digit body
            "ms-settings:/",  // empty body
            "steam:/run/1",   //
            "data:text/html,<h1>x</h1>",
            "file:///tmp/x",
            "http://localhost:18800",
        ] {
            assert!(
                has_explicit_scheme(input),
                "{input:?} carries a real scheme and must NOT be normalized"
            );
        }

        for input in OS_PROTOCOL_HANDLER_URLS.iter().chain(SLASH_VARIANT_API_PORT_URLS) {
            assert!(
                has_explicit_scheme(input),
                "{input:?} carries a real scheme and must NOT be normalized"
            );
        }
    }

    #[test]
    fn scheme_less_input_opens_with_the_right_scheme() {
        for (input, expected) in [
            ("localhost:5173", "http://localhost:5173/"),
            ("127.0.0.1:3000/x", "http://127.0.0.1:3000/x"),
            (
                "github.com/rdfitted/hive-manager",
                "https://github.com/rdfitted/hive-manager",
            ),
            ("[::1]:8080", "http://[::1]:8080/"),
            ("foo.localhost:3000", "http://foo.localhost:3000/"),
        ] {
            let url = validate(input)
                .unwrap_or_else(|error| panic!("{input:?} should validate, got {error:?}"));
            assert_eq!(url.as_str(), expected, "wrong normalization for {input:?}");
        }
    }

    #[test]
    fn scheme_less_input_still_hits_every_rejection() {
        for input in [
            // Reserved Hive API port, in every spelling that reaches it.
            "localhost:18800",
            "127.0.0.1:18800",
            "LOCALHOST:18800",
            "//localhost:18800/api",
            "localhost:18800/api/health",
            "localhost.:18800",
            // Origins reserved for the trusted main window.
            "tauri.localhost",
            "localhost:1420",
        ] {
            assert!(
                validate(input).is_err(),
                "{input:?} should be rejected after normalization"
            );
        }
    }

    /// Regression lock for a real bypass.
    ///
    /// A normalizer that decomposes input into host + path and reassembles it
    /// turns `localhost:18800` into `http://localhost/18800`: the reserved port
    /// moves into the PATH, `port_or_known_default()` returns 80, and the local
    /// Hive API sails through validation. The only safe implementation is a
    /// string prepend onto the trimmed raw input.
    #[test]
    fn normalization_never_relocates_the_port() {
        for (input, scheme) in [
            ("localhost:18800", "http"),
            ("localhost:5173", "http"),
            ("127.0.0.1:3000/x", "http"),
            ("[::1]:8080", "http"),
            ("foo.localhost:3000", "http"),
            ("example.com:8443/path?q=1#frag", "https"),
            ("github.com/rdfitted/hive-manager", "https"),
            ("//localhost:18800/api", "http"),
        ] {
            let normalized = normalize_preview_input(input);
            assert_eq!(
                normalized,
                format!("{scheme}://{input}"),
                "normalization of {input:?} must be a pure prefix of the raw input"
            );
            assert!(
                normalized.ends_with(input),
                "normalization of {input:?} rewrote the input body"
            );
        }

        // The specific shape of the bypass, named so a future refactor that
        // reintroduces it fails here with an obvious message.
        assert_ne!(
            normalize_preview_input("localhost:18800"),
            "http://localhost/18800",
            "the reserved API port was relocated from the authority into the path"
        );
        assert_eq!(
            Url::parse("http://localhost/18800")
                .unwrap()
                .port_or_known_default(),
            Some(80),
            "sanity: the bypass shape really does present as port 80"
        );
    }

    #[test]
    fn normalization_leaves_scheme_bearing_input_alone() {
        for input in [
            "http://localhost:5173/x",
            "https://github.com/acme/repo",
            "file:///tmp/index.html",
            "ftp://example.com/file",
        ] {
            assert_eq!(normalize_preview_input(input), input);
        }
        assert_eq!(
            normalize_preview_input("  https://example.com/x  "),
            "https://example.com/x"
        );
        assert_eq!(normalize_preview_input("   "), "");
    }

    /// Path-only input must stay path-only.
    ///
    /// `https:///relative/path` is NOT hostless once parsed: the WHATWG
    /// "special authority ignore slashes" rule eats the extra slash and promotes
    /// `relative` to the host, so a naive prepend would turn a clearly invalid
    /// entry into a live navigation to `https://relative/path`.
    #[test]
    fn normalization_refuses_to_invent_a_host_for_path_only_input() {
        for input in ["/relative/path", "///a/b", "?q=1", "#frag", "//"] {
            assert_eq!(
                normalize_preview_input(input),
                input,
                "{input:?} has no authority and must not be given a scheme"
            );
            assert!(validate(input).is_err(), "{input:?} should be rejected");
        }

        // Contrast: protocol-relative input DOES carry an authority, and the
        // pure prepend leaves the redundant slashes for `Url::parse` to collapse.
        assert_eq!(
            normalize_preview_input("//example.com/x"),
            "https:////example.com/x"
        );
        assert_eq!(
            validate("//example.com/x").unwrap().as_str(),
            "https://example.com/x"
        );
    }

    #[test]
    fn localhost_classification_covers_the_documented_host_forms() {
        for host in [
            "localhost",
            "LOCALHOST",
            "localhost.",
            "127.0.0.1",
            "::1",
            "[::1]",
            "foo.localhost",
            "a.b.LOCALHOST",
        ] {
            assert!(is_localhostish_host(host), "{host:?} should be localhost-ish");
        }

        for host in [
            "example.com",
            "localhost.example.com",
            "notlocalhost",
            "127.0.0.2",
            "",
        ] {
            assert!(
                !is_localhostish_host(host),
                "{host:?} should not be localhost-ish"
            );
        }
    }

    #[test]
    fn schemeless_host_extraction_handles_ports_paths_and_ipv6() {
        assert_eq!(schemeless_host("localhost:5173"), "localhost");
        assert_eq!(schemeless_host("127.0.0.1:3000/x"), "127.0.0.1");
        assert_eq!(schemeless_host("github.com/a/b?c#d"), "github.com");
        assert_eq!(schemeless_host("[::1]:8080"), "::1");
        assert_eq!(schemeless_host("[::1]"), "::1");
        assert_eq!(schemeless_host("//localhost:18800/api"), "localhost");
        assert_eq!(schemeless_host("user:pass@example.com:8080"), "example.com");
        // A non-numeric suffix is not a port, so it is left attached and simply
        // fails to classify as localhost-ish.
        assert_eq!(schemeless_host("data:text/html,x"), "data:text");
    }

    /// Regression lock for finding 4.
    ///
    /// The port-strip branch used to require a NON-EMPTY port segment, so an
    /// authority with an empty port kept its trailing colon: `localhost:` is not
    /// recognised by `is_localhostish_host` (which trims `.`, `[` and `]`, but
    /// not `:`), so the operator's plain-HTTP dev server was scheme-selected as
    /// `https` and died on the TLS handshake.
    #[test]
    fn schemeless_host_strips_an_empty_port_segment() {
        assert_eq!(schemeless_host("localhost:/dashboard"), "localhost");
        assert_eq!(schemeless_host("localhost:"), "localhost");
        assert_eq!(schemeless_host("//localhost:/dashboard"), "localhost");
        assert_eq!(schemeless_host("user@localhost:/x"), "localhost");
        assert_eq!(schemeless_host("127.0.0.1:?q=1"), "127.0.0.1");

        // ... and the stripped host now classifies, so the scheme is `http`.
        for input in ["//localhost:/dashboard", "//127.0.0.1:/x"] {
            let normalized = normalize_preview_input(input);
            assert!(
                normalized.starts_with("http://"),
                "{input:?} is a local dev server and must not be given https, \
                 got {normalized:?}"
            );
        }

        // The empty port is NOT the `host:port` shape, so `localhost:/dashboard`
        // is an EXPLICIT `localhost:` scheme and is rejected outright rather
        // than silently navigated to a TLS failure. Pinned so the interaction
        // with `has_explicit_scheme` stays deliberate.
        assert!(has_explicit_scheme("localhost:/dashboard"));
        assert!(validate("localhost:/dashboard").is_err());
    }

    /// Regression lock for finding 5.
    ///
    /// `restore_main_window_geometry` was reachable only from
    /// `leave_docked_mode` and the `close_preview_window` command. There is no
    /// `CloseRequested` handler in this crate, so closing a DOCKED preview with
    /// the native titlebar X ran `WindowEvent::Destroyed` alone: the main window
    /// stayed tiled in the left column beside empty desktop, and because the
    /// emitted `open: false` hides the dock/pop-out/close cluster, no in-app
    /// control was left to undo it.
    ///
    /// The window runtime is `#[cfg(not(test))]` and needs a live `AppHandle`,
    /// so this asserts on the source text - the only way to keep the two
    /// teardown paths in step.
    #[test]
    fn the_destroyed_arm_restores_the_main_window_like_the_close_command() {
        // This file is checked out with CRLF on Windows, so a `\n`-based
        // delimiter search silently misses. That is not cosmetic: the
        // command-path check below paired a missed delimiter with
        // `unwrap_or(len())`, which widened the slice to the whole rest of the
        // file - including this test's own assertion strings - so it passed by
        // matching itself. Normalize once, and make every delimiter fatal
        // rather than falling back to a wider slice.
        let normalized = include_str!("mod.rs").replace("\r\n", "\n");
        let source: &str = &normalized;

        let arm_start = source
            .find("WindowEvent::Destroyed =>")
            .expect("the preview window event handler must still match Destroyed");
        let arm = &source[arm_start..];
        let arm_end = arm
            .find("_ => {}")
            .expect("the Destroyed arm must still be followed by the catch-all arm");
        let arm = &arm[..arm_end];

        assert!(
            arm.contains("undock_layout_after_teardown"),
            "closing a docked preview with the titlebar X must put the main \
             window back, exactly like close_preview_window does"
        );
        assert!(
            arm.contains("state.docked"),
            "the Destroyed arm must consult the dock state to decide whether \
             to restore the main window layout"
        );
        assert!(
            arm.contains("async_runtime::spawn"),
            "window geometry must not be touched from inside the window-event \
             callback - get off it first, as the retile follow does"
        );

        // The command path must keep using the shared helper, so the two
        // teardown routes cannot drift apart again.
        let command = source
            .find("pub async fn close_preview_window")
            .map(|start| &source[start..])
            .expect("close_preview_window must still exist");
        let body_end = command
            .find("\n}\n")
            .expect("close_preview_window must be terminated by a column-0 closing brace");
        assert!(
            command[..body_end].contains("undock_layout_after_teardown"),
            "close_preview_window must share the teardown helper with the \
             Destroyed arm"
        );
    }

    /// Regression lock: the dock PREFERENCE and the live-layout FACT are
    /// separate bits.
    ///
    /// `enter_docked_mode` used to gate its pre-dock snapshot on `!docked`, but
    /// `docked` deliberately survives a teardown so the next open re-tiles -
    /// while the teardown unconditionally CONSUMES `main_restore`. A docked
    /// close therefore left `docked: true, main_restore: None`, the reopen
    /// skipped the snapshot, and the undock/close after it had nothing to put
    /// back: Hive Manager stranded in the left-hand column beside empty desktop.
    #[test]
    fn reopening_a_docked_preview_takes_a_fresh_restore_snapshot() {
        let mut state = PersistedPreviewState::default();
        let before = WindowGeometry {
            x: 100,
            y: 80,
            width: 1280.0,
            height: 800.0,
        };

        // open -> dock
        assert!(state.needs_main_snapshot());
        state.record_main_snapshot(Some(before), false);
        state.docked = true;

        // dock again: idempotent, must NOT re-snapshot the tiled geometry as the
        // restore target - that would make undock a no-op.
        assert!(
            !state.needs_main_snapshot(),
            "a redundant dock must not overwrite the pre-dock snapshot with the \
             already-tiled geometry"
        );

        // close (the command or the titlebar X) restores and consumes the
        // snapshot; the PREFERENCE survives so the next open re-tiles.
        assert_eq!(state.take_main_restore().0.map(|g| g.x), Some(100));
        assert!(state.docked, "the dock preference must survive a teardown");
        // the Destroyed arm's second, idempotent pass
        assert!(state.take_main_restore().0.is_none());

        // reopen re-tiles off the surviving preference - and MUST snapshot again
        assert!(
            state.needs_main_snapshot(),
            "a reopened dock that skips the snapshot leaves the undock/close \
             after it with nothing to restore, stranding the main window in the \
             docked column"
        );
        let reopened = WindowGeometry {
            x: 220,
            y: 140,
            width: 1440.0,
            height: 900.0,
        };
        state.record_main_snapshot(Some(reopened), false);

        assert_eq!(state.take_main_restore().0.map(|g| g.x), Some(220));
    }

    /// The state machine above is only meaningful if the windowing layer
    /// actually routes through it. That layer is `#[cfg(not(test))]` and needs a
    /// live `AppHandle`, so - same discipline as the `Destroyed` arm lock above
    /// - the wiring is pinned against the source text.
    #[test]
    fn the_dock_path_gates_its_snapshot_on_the_layout_bit_not_the_preference() {
        // This file is checked out CRLF on Windows, so the `\n}\n` terminator
        // below only finds a function end after normalization. Without it the
        // slice silently runs to EOF and swallows this test's own assertion
        // strings, which makes the lock pass against any source at all.
        let source = include_str!("mod.rs").replace("\r\n", "\n");

        let dock = body_of(&source, "enter_docked_mode");
        assert!(
            dock.contains("stage_dock"),
            "enter_docked_mode must route its pre-dock reads through the state \
             machine, or the transition tests below bind nothing"
        );
        // The gate itself moved into the state machine when the commit was
        // deferred past the tiling (issue #160); it is still the LAYOUT bit.
        assert!(
            method_of(&source, "stage_dock").contains("needs_main_snapshot"),
            "the pre-dock snapshot must stay gated on the live-layout bit; \
             gating on `docked` makes a reopened dock skip the snapshot"
        );
        assert!(
            method_of(&source, "commit_dock").contains("record_main_snapshot"),
            "the snapshot must be recorded through the helper, so the geometry \
             and the layout bit are always set together"
        );
        // `stage_dock`'s mention above is an unconditional field initializer -
        // structurally mandatory for `StagedDock` to compile, so it cannot
        // encode whether the gate survives. This pins the gate itself.
        assert!(
            method_of(&source, "commit_dock").contains("if staged.snapshot_owed"),
            "the pre-dock snapshot must stay GATED on the live-layout bit, not \
             merely computed from it: an ungated commit overwrites the restore \
             geometry on every re-dock"
        );

        assert!(
            body_of(&source, "restore_main_window_geometry").contains("take_main_restore"),
            "the teardown must consume the snapshot through the helper, so the \
             layout bit cannot stay set after the geometry is gone"
        );
    }

    /// The text of `fn <name>` up to the first column-0 `}`.
    ///
    /// Callers must hand in CRLF-normalized source: a missed `\n}\n` here would
    /// widen the slice to the rest of the file and let an assertion pass by
    /// matching this test module's own text.
    fn body_of<'a>(source: &'a str, name: &str) -> &'a str {
        let needle = format!("fn {name}");
        let body = source
            .find(&needle)
            .map(|start| &source[start..])
            .unwrap_or_else(|| panic!("{name} must still exist"));
        let end = body
            .find("\n}\n")
            .unwrap_or_else(|| panic!("{name} must have a column-0 closing brace"));
        &body[..end]
    }

    /// The text of an indented `fn <name>` method up to its 4-space closing
    /// brace. Same delimiter discipline as [`body_of`].
    fn method_of<'a>(source: &'a str, name: &str) -> &'a str {
        let needle = format!("    fn {name}");
        let body = source
            .find(&needle)
            .map(|start| &source[start..])
            .unwrap_or_else(|| panic!("{name} must still exist as a method"));
        let end = body
            .find("\n    }\n")
            .unwrap_or_else(|| panic!("{name} must have a 4-space closing brace"));
        &body[..end]
    }

    // -- issue #160: a dock that fails partway must not half-apply -----------

    /// The geometry a dock reads before it moves anything.
    fn staging_reads() -> (WindowGeometry, WindowGeometry) {
        (
            // main, pre-dock
            WindowGeometry {
                x: 64,
                y: 32,
                width: 1280.0,
                height: 800.0,
            },
            // preview, floating where the operator left it
            WindowGeometry {
                x: 900,
                y: 120,
                width: 800.0,
                height: 600.0,
            },
        )
    }

    /// Regression lock for issue #160.
    ///
    /// `enter_docked_mode` used to set `docked = true` and record the pre-dock
    /// snapshot BEFORE issuing its `set_position`/`set_size` calls. A geometry
    /// call that failed partway returned `Err` with the session already
    /// reporting docked: the frontend showed a docked state the operator had
    /// just been told had failed, the docked minimum size stayed applied, and
    /// the main-window follow handler - which gates purely on `docked` - snapped
    /// the preview into a column on the next drag, half-applying a dock that
    /// had errored.
    #[test]
    fn a_dock_whose_tiling_fails_leaves_no_trace_in_the_state_machine() {
        let (main_before, preview_before) = staging_reads();
        let mut state = PersistedPreviewState::default();
        state.floating = Some(preview_before);

        let staged = state.stage_dock(Some(main_before), false, Some(preview_before));

        // Staging alone must change nothing at all - it only borrows the state.
        assert!(!state.docked);
        assert!(!state.layout_applied);
        assert!(state.main_restore.is_none());
        assert!(staged.snapshot_owed);

        // Stand in for the preview's own `Moved`/`Resized` handler, which runs
        // while the dock is in flight - `enter_docked_mode` deliberately drops
        // the lock across the geometry calls - and records `floating` on every
        // move so long as `!docked`. Without this the rollback below has
        // nothing to undo, and "the restore put the field back" is
        // indistinguishable from "nothing ever touched the field".
        state.floating = Some(WindowGeometry {
            x: 768,
            y: 0,
            width: 512.0,
            height: 1040.0,
        });

        // Inject the failure the real geometry calls would produce.
        let tiled: Result<(), &str> = Err("Failed to resize Hive Manager: os error 5");
        state.settle_dock(staged, &tiled);

        assert!(
            !state.docked,
            "a dock whose tiling failed must report NOT docked - `docked` is \
             both what status_for surfaces and what the follow handler retiles on"
        );
        assert!(
            !state.layout_applied,
            "no docked layout is live after a failed dock"
        );
        assert!(
            state.main_restore.is_none(),
            "a failed dock must not leave a restore snapshot stranded behind a \
             `docked: false` that no teardown path will ever consume"
        );
        assert!(
            state.needs_main_snapshot(),
            "the operator must be able to retry cleanly: the retry owes a fresh \
             pre-dock snapshot"
        );
        assert_eq!(
            state.floating.map(|geometry| (geometry.x, geometry.y)),
            Some((900, 120)),
            "the remembered floating position must survive a failed dock"
        );
    }

    /// The success half of the same transition: the ACTIVE state is committed,
    /// and both bits land together.
    #[test]
    fn a_dock_commits_the_active_state_once_the_tiling_succeeds() {
        let (main_before, preview_before) = staging_reads();
        let mut state = PersistedPreviewState::default();

        let staged = state.stage_dock(Some(main_before), true, Some(preview_before));
        let tiled: Result<(), &str> = Ok(());
        state.settle_dock(staged, &tiled);

        assert!(state.docked, "a dock that tiled must report docked");
        assert!(!state.needs_main_snapshot(), "the layout is now live");
        assert_eq!(
            state.main_restore.map(|geometry| (geometry.x, geometry.y)),
            Some((64, 32)),
            "the snapshot must hold the geometry read BEFORE the windows moved"
        );
        assert!(state.main_was_maximized);
        assert_eq!(
            state.floating.map(|geometry| (geometry.x, geometry.y)),
            Some((900, 120)),
            "the floating snapshot must hold the pre-tiling read, not whatever \
             the preview's own Moved handler recorded while the dock was in flight"
        );
    }

    /// A redundant dock over an ALREADY LIVE layout is the one case where the
    /// rollback must keep its hands off: that layout is still applied and its
    /// restore snapshot is the only way back out of it.
    #[test]
    fn a_failed_re_dock_leaves_the_live_layout_and_its_snapshot_alone() {
        let (main_before, preview_before) = staging_reads();
        let mut state = PersistedPreviewState::default();
        state.floating = Some(preview_before);
        state.record_main_snapshot(Some(main_before), true);
        state.docked = true;

        // Both windows are already tiled, so these reads are of the COLUMNS.
        let tiled_main = WindowGeometry {
            x: 0,
            y: 0,
            width: 768.0,
            height: 1040.0,
        };
        let tiled_preview = WindowGeometry {
            x: 768,
            y: 0,
            width: 512.0,
            height: 1040.0,
        };
        let staged = state.stage_dock(Some(tiled_main), false, Some(tiled_preview));
        assert!(
            !staged.snapshot_owed,
            "a live layout already owns its snapshot"
        );

        let tiled: Result<(), &str> = Err("Failed to position the preview: os error 5");
        state.settle_dock(staged, &tiled);

        assert!(state.docked, "the layout that was already live stays live");
        assert!(!state.needs_main_snapshot());
        assert_eq!(
            state.main_restore.map(|geometry| (geometry.x, geometry.y)),
            Some((64, 32)),
            "a failed re-tile must not overwrite the live layout's restore \
             snapshot with the already-tiled column geometry"
        );
        assert!(state.main_was_maximized);
        assert_eq!(
            state.floating.map(|geometry| (geometry.x, geometry.y)),
            Some((900, 120)),
            "the rollback restores `floating` verbatim: the preview's own \
             Moved handler may have recorded the half-tiled column into it \
             while the dock was in flight"
        );
    }

    /// The fourth quadrant of the transition matrix, and the ONLY one that
    /// reaches the false branch of EITHER gate in `commit_dock`: a redundant
    /// dock over a live layout that SUCCEEDS.
    ///
    /// Not hypothetical - this is the reopen path. `open_preview_window`
    /// re-tiles off the surviving `docked` preference, so `commit_dock` runs
    /// with `previous.docked == true` on every reopen of a docked preview, and
    /// `dock_preview_window` is an unguarded command besides. Both staged reads
    /// are therefore of the COLUMNS, and each gate is the only thing keeping
    /// that column geometry out of a snapshot the next teardown will restore
    /// from.
    #[test]
    fn a_successful_re_dock_keeps_the_live_layouts_snapshots() {
        let (main_before, preview_before) = staging_reads();
        let mut state = PersistedPreviewState::default();
        state.floating = Some(preview_before);
        state.record_main_snapshot(Some(main_before), true);
        state.docked = true;

        // Both windows are already tiled, so these reads are of the COLUMNS.
        let tiled_main = WindowGeometry {
            x: 0,
            y: 0,
            width: 768.0,
            height: 1040.0,
        };
        let tiled_preview = WindowGeometry {
            x: 768,
            y: 0,
            width: 512.0,
            height: 1040.0,
        };
        let staged = state.stage_dock(Some(tiled_main), false, Some(tiled_preview));
        assert!(
            !staged.snapshot_owed,
            "a live layout already owns its snapshot"
        );

        let tiled: Result<(), &str> = Ok(());
        state.settle_dock(staged, &tiled);

        assert!(state.docked, "the layout stays live");
        assert!(!state.needs_main_snapshot());
        assert_eq!(
            state.floating.map(|geometry| (geometry.x, geometry.y)),
            Some((900, 120)),
            "commit_dock's `!docked` gate is the only thing standing between a \
             re-dock and the operator's remembered floating position: without \
             it the docked column is committed as `floating`, and the next \
             undock drops the preview into a column-shaped window instead of \
             where the operator left it"
        );
        assert_eq!(
            state.main_restore.map(|geometry| (geometry.x, geometry.y)),
            Some((64, 32)),
            "a re-dock over a live layout owes no fresh snapshot, so \
             `snapshot_owed` must gate it: without that gate the pre-dock \
             geometry is overwritten with the already-tiled column, and the \
             undock after it restores the main window into the column it is \
             already in"
        );
        assert!(
            state.main_was_maximized,
            "the maximized bit belongs to the pre-dock read, not the re-tile"
        );
    }

    /// The transition tests above only bind the app if `enter_docked_mode`
    /// actually routes through them in the right order. That function needs a
    /// live `AppHandle`, so - same discipline as the #159 locks above - the
    /// ordering is pinned against the source text.
    #[test]
    fn the_dock_path_commits_its_active_state_only_after_the_tiling() {
        let source = include_str!("mod.rs").replace("\r\n", "\n");
        let dock = body_of(&source, "enter_docked_mode");

        let split = dock
            .find("dock_split(")
            .expect("enter_docked_mode must still compute the split");
        let stage = dock
            .find("stage_dock(")
            .expect("enter_docked_mode must stage the dock through the state machine");
        let tile = dock
            .find("tile_docked_windows(")
            .expect("enter_docked_mode must issue its geometry calls through the tiler");
        // The ARGUMENTS are pinned, not just the position: a settle handed a
        // fabricated `Ok` sits in exactly the right place in the ordering below
        // while committing `docked = true` for a tiling that failed - issue
        // #160 restored, green. `body_of` slices only this function, so the
        // transition tests' own `settle_dock(staged, &tiled)` calls cannot
        // satisfy it.
        let settle = dock
            .find("settle_dock(staged, &tiled)")
            .expect("enter_docked_mode must settle the state machine on the ACTUAL tiling result");

        assert!(
            split < stage,
            "the too-narrow-display bail must stay ahead of every mutation"
        );
        assert!(
            stage < tile && tile < settle,
            "the docked state must be committed AFTER the geometry calls, not \
             before them: a set_position/set_size that fails partway otherwise \
             returns Err with the session already reporting docked"
        );
        assert!(
            dock.contains("undo_failed_dock"),
            "a failed dock must undo the window side effects it applied, not \
             just the persisted state"
        );
        assert!(
            !dock.contains("docked = true"),
            "enter_docked_mode must not set the ACTIVE dock bit itself - that \
             belongs to commit_dock, which only runs on a successful tiling"
        );

        let propagate = dock
            .find("return Err(error)")
            .expect("a failed tiling must be reported to the caller");
        assert!(
            settle < propagate,
            "the original tiling error must reach the operator AFTER the state \
             has settled: without the propagation the dock command reports \
             success for a dock that never tiled, and open_preview_window's \
             downgrade-to-floating never fires"
        );

        assert!(
            method_of(&source, "commit_dock").contains("!staged.previous.docked"),
            "the floating snapshot must stay gated on the PREFERENCE as it \
             stood before the attempt, or a re-dock commits the column"
        );

        // Deadlock lock. Every one of these reads is a blocking round-trip to
        // the event-loop thread, and that thread takes this same mutex in the
        // window-event handlers. This is an async command, so it is never ON
        // the event-loop thread and never short-circuits: a read taken under
        // the guard freezes the event loop against the command with no timeout
        // on either side. Pinned textually because the whole path is
        // `#[cfg(not(test))]` and cannot be driven from a test.
        let guard = dock
            .find("runtime(app_state).lock()")
            .expect("enter_docked_mode must take the runtime lock to stage");
        let mut first_read = usize::MAX;
        for read in [
            "read_geometry(&main)",
            "main.is_maximized()",
            "read_geometry(&preview)",
        ] {
            let at = dock
                .find(read)
                .unwrap_or_else(|| panic!("enter_docked_mode must still call {read}"));
            assert!(
                at < guard,
                "`{read}` must be hoisted ABOVE the runtime lock: it blocks on \
                 a reply from the event-loop thread, which takes that same \
                 mutex in the preview's Moved/Resized/Destroyed handlers - \
                 holding it across the read deadlocks both windows permanently"
            );
            first_read = first_read.min(at);
        }

        // Mutual exclusion, and the direct consequence of the hoist above: with
        // every read AND the tiling itself outside the runtime mutex, the state
        // is consistent only between one attempt's staging and its own
        // settlement. Two overlapping attempts therefore both stage a snapshot
        // debt while no layout is live yet.
        let transition = dock
            .find("DOCK_TRANSITION.lock()")
            .expect("enter_docked_mode must serialize the whole dock transition");
        assert!(
            transition < first_read,
            "the transition lock must be taken BEFORE the pre-dock reads: taken \
             any later, a second attempt reads its own `pre-dock` geometry off \
             the columns the first one just tiled and commits them as the \
             restore snapshot, so the next teardown puts Hive Manager back into \
             the column it is already in - and that poisoned snapshot persists \
             across restarts"
        );
        assert!(
            transition < guard,
            "the transition lock must be strictly OUTERMOST - taken before the \
             runtime mutex and never while holding it - or the two lock orders \
             invert"
        );

        // The rollback must not swallow the cause the operator needs.
        assert!(
            method_of(&source, "settle_dock").contains("rollback_dock"),
            "a failed tiling must roll the dock fields back"
        );
        assert!(
            body_of(&source, "undo_failed_dock").contains("tracing::warn!"),
            "the physical undo is best effort: a secondary failure must be \
             logged, never propagated over the original tiling error"
        );
    }

    /// Regression lock for the concurrent-dock finding.
    ///
    /// `enter_docked_mode` has two callers - the `dock_preview_window` command
    /// and `open_preview_window`'s re-dock off the surviving preference - and
    /// both are `#[tauri::command] pub async fn`, so Tauri runs them on the
    /// tokio pool. Nothing serialized them: the only statics in the module were
    /// the runtime mutex and the retile flag, and the runtime mutex is
    /// deliberately DROPPED across both the pre-dock reads and the tiling so the
    /// blocking window getters cannot deadlock the event loop. That left the
    /// entire transition running unlocked.
    ///
    /// Two overlapping attempts then both stage `snapshot_owed`, because both
    /// stage while `layout_applied` is still false. If either tiling fails the
    /// loser's `rollback_dock` persists `docked: false, layout_applied: false,
    /// main_restore: None` over the winner's commit and `undo_failed_dock`
    /// physically un-tiles the winner's layout; if both succeed, the second
    /// attempt's reads land on the columns the first one just tiled and get
    /// committed as `main_restore`. `snapshot_owed` cannot catch that - it is
    /// computed for BOTH attempts before either settles - so the serial re-dock
    /// lock above binds nothing here.
    ///
    /// The teardown paths take the same lock: the titlebar X reaches
    /// `undock_layout_after_teardown` through a task spawned off `Destroyed`,
    /// which the frontend's dock-button busy flag never covers.
    ///
    /// The whole path is `#[cfg(not(test))]` and needs a live `AppHandle`, so -
    /// same discipline as the locks above - this is pinned against the source
    /// text.
    #[test]
    fn every_dock_transition_path_takes_the_same_serializing_lock() {
        let source = include_str!("mod.rs").replace("\r\n", "\n");
        // Every assertion below runs against the RUNTIME half of the file only.
        // This test names the lock it is checking for, so matching the whole
        // file would let it pass by finding its own assertion strings.
        let runtime_source = &source[..source
            .find("mod tests {")
            .expect("the test module must still terminate the runtime source")];

        assert!(
            runtime_source.contains("static DOCK_TRANSITION: Mutex<()> = Mutex::new(());"),
            "the dock transition lock must be its OWN static, separate from the \
             runtime mutex: the runtime mutex is taken by the event-loop thread \
             in the window-event handlers, and a transition blocks on replies \
             from that thread, so reusing it for this would deadlock both \
             windows permanently"
        );

        for name in [
            "enter_docked_mode",
            "leave_docked_mode",
            "undock_layout_after_teardown",
        ] {
            let body = body_of(runtime_source, name);
            let transition = body.find("DOCK_TRANSITION.lock()").unwrap_or_else(|| {
                panic!(
                    "{name} must serialize itself against the other dock \
                     transitions: a dock, an undock and a teardown all rewrite \
                     the same restore snapshot, and only one of them can own it"
                )
            });
            assert!(
                body.contains("let _transition = DOCK_TRANSITION.lock();"),
                "{name} must bind the transition guard to a NAMED local: an \
                 unnamed temporary guard drops at the statement semicolon, so \
                 the lock would be released before the transition it is meant \
                 to cover has run at all"
            );
            if let Some(runtime_lock) = body.find("runtime(app_state).lock()") {
                assert!(
                    transition < runtime_lock,
                    "{name} must take the transition lock BEFORE the runtime \
                     mutex - it is the outermost of the two everywhere, and one \
                     path taking them the other way round is a lock-order \
                     inversion"
                );
            }
        }
    }

    // -- issue #157 §2: dock geometry ---------------------------------------

    #[test]
    fn dock_split_columns_are_flush_and_never_overlap() {
        for (width, scale) in [
            (1920u32, 1.0f64),
            (1920, 1.5),
            (2560, 1.0),
            (3840, 2.0),
            (1440, 1.0),
            (1366, 1.0),
        ] {
            let (main, preview) = dock_split(width, scale)
                .unwrap_or_else(|error| panic!("{width}@{scale} should split: {error}"));
            assert_eq!(
                main + preview,
                width,
                "columns must tile {width} exactly at scale {scale}"
            );
            assert!(
                f64::from(main) >= MAIN_MIN_LOGICAL_WIDTH * scale,
                "main column below its minimum at {width}@{scale}"
            );
            assert!(
                f64::from(preview) >= DOCKED_PREVIEW_MIN_LOGICAL_WIDTH * scale,
                "preview column below its minimum at {width}@{scale}"
            );
        }
    }

    #[test]
    fn dock_split_refuses_displays_that_cannot_host_both_windows() {
        // 1220 logical px is exactly the floor; anything under it must refuse
        // rather than produce an overlapping "best effort" layout.
        assert!(dock_split(1219, 1.0).is_err());
        assert!(dock_split(1220, 1.0).is_ok());
        // 1080p at 200% scale is only 960 logical px of work area.
        assert!(dock_split(1920, 2.0).is_err());
        // A nonsensical scale factor falls back to 1.0 instead of panicking.
        assert!(dock_split(1920, 0.0).is_ok());
        assert!(dock_split(1920, f64::NAN).is_ok());
    }

    #[test]
    fn remembered_positions_are_only_restored_onto_a_live_monitor() {
        // Primary at the origin, a second monitor to its left (negative x, the
        // usual Windows layout for a display placed left of the primary).
        let monitors = [(0i32, 0i32, 1920u32, 1040u32), (-2560, -200, 2560, 1400)];

        assert!(position_is_on_a_monitor(10, 10, &monitors));
        assert!(position_is_on_a_monitor(-2000, 0, &monitors));
        assert!(position_is_on_a_monitor(1919, 1039, &monitors));

        // The external display was unplugged: the remembered corner is nowhere.
        assert!(!position_is_on_a_monitor(-2000, 0, &monitors[..1]));
        // Just past the bottom-right corner of the primary.
        assert!(!position_is_on_a_monitor(1920, 1040, &monitors[..1]));
        // No monitors reported at all is treated as "do not restore".
        assert!(!position_is_on_a_monitor(0, 0, &[]));
    }
}
