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
    webview::NewWindowResponse, AppHandle, Emitter, LogicalSize, Manager, Monitor,
    PhysicalPosition, PhysicalSize, State, WebviewUrl, WebviewWindow, WebviewWindowBuilder,
    WindowEvent,
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
    /// Whether the preview was MAXIMIZED when the dock took it over.
    ///
    /// Pairs with `floating` exactly as `main_was_maximized` pairs with
    /// `main_restore`: the tiler has to drop the window out of maximize before
    /// any explicit geometry sticks, so without this bit the state is simply
    /// destroyed by docking. `floating` cannot stand in for it - a maximized
    /// window's rect is not the rect it restores DOWN to, which is why the
    /// `Moved`/`Resized` handler declines to record one.
    ///
    /// `#[serde(default)]`, like every field here, so a
    /// `preview-window-state.json` written before this existed still loads.
    #[serde(default)]
    preview_was_maximized: bool,
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

    /// Consumes the remembered preview-maximize bit, handing back the floating
    /// rect it belongs with.
    ///
    /// Mirrors [`Self::take_main_restore`], and for the same reason: the bit
    /// describes a restore that is owed exactly ONCE. Left behind after the
    /// window it was read from is gone, it gets replayed onto the next,
    /// unrelated preview window - which the operator never maximized.
    ///
    /// `floating` is deliberately NOT taken. It is the remembered position,
    /// replayed on every open for the life of the install; the maximize bit is
    /// a one-shot.
    fn take_preview_restore(&mut self) -> (Option<WindowGeometry>, bool) {
        (
            self.floating,
            std::mem::take(&mut self.preview_was_maximized),
        )
    }

    fn dock_fields(&self) -> DockFields {
        DockFields {
            docked: self.docked,
            floating: self.floating,
            preview_was_maximized: self.preview_was_maximized,
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
        preview_was_maximized: bool,
    ) -> StagedDock {
        StagedDock {
            previous: self.dock_fields(),
            snapshot_owed: self.needs_main_snapshot(),
            main_geometry,
            main_was_maximized,
            preview_geometry,
            preview_was_maximized,
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
        //
        // Exclusive branches, exactly like `undo_failed_dock`. `preview_geometry`
        // is that same pre-tiling read, so for a MAXIMIZED preview it holds the
        // maximized rect - and `floating` means the rect the preview restores
        // DOWN to, which is why the `Moved`/`Resized` handler declines to record
        // one. Committing the maximized rect here would be the one remaining
        // writer that destroys it, and both consumers replay it and THEN
        // re-maximize, so it would become the re-maximized window's own
        // restore-down rect: issue #163 reinstated through the dock.
        //
        // The maximized branch REASSIGNS rather than skipping, and that is
        // load-bearing: the tiler unmaximizes the preview before positioning it,
        // so the in-flight handler sees `!docked` and a now-unmaximized window
        // and lands the docked COLUMN in `floating`. Writing the pre-dock value
        // back is what still closes the race described above.
        if !staged.previous.docked {
            if staged.preview_was_maximized {
                self.floating = staged.previous.floating;
            } else if let Some(geometry) = staged.preview_geometry {
                self.floating = Some(geometry);
            }
            // Under the SAME gate, because it is the same snapshot: by the time
            // a re-tile of an already-live dock stages, the tiler has long since
            // unmaximized the preview, so the read is a guaranteed `false` and
            // committing it would erase the bit the first dock captured.
            self.preview_was_maximized = staged.preview_was_maximized;
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
            preview_was_maximized,
            main_restore,
            main_was_maximized,
            layout_applied,
        } = staged.previous;
        self.docked = docked;
        self.floating = floating;
        self.preview_was_maximized = preview_was_maximized;
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
    preview_was_maximized: bool,
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
    /// Read before the tiler unmaximizes the preview - afterwards it is always
    /// `false`, which is how the state came to be lost on every path.
    preview_was_maximized: bool,
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
/// Coalesces the PREVIEW's own move/resize storm, exactly as [`RETILE_PENDING`]
/// coalesces the main window's.
///
/// Deliberately a second flag rather than sharing that one: the two storms are
/// independent, and one flag would let a main-window drag suppress the preview's
/// recording (and vice versa) for as long as it lasted.
#[cfg(not(test))]
static FLOATING_RECORD_PENDING: AtomicBool = AtomicBool::new(false);

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

/// The monitor whose work area the dock geometry is computed against.
///
/// ONE helper because the fallback policy has to be the same at both call
/// sites. `enter_docked_mode` sizes the two columns from this monitor's work
/// area and `follow_docked_preview` re-derives the preview's column from it on
/// every move/resize, so a fallback that only the dock honoured would let the
/// pane stop tracking in exactly the state the fallback was added to survive -
/// docked via the fallback, then never followed again (issue #161).
///
/// The fallback is right for BOTH, because `Ok(None)` never means "on a
/// monitor we cannot name" on any backend tao supports - it means "on no
/// monitor at all", so there is no correct non-primary monitor for the
/// fallback to override:
/// - Windows: unreachable. `current_monitor` is `MonitorFromWindow(hwnd,
///   MONITOR_DEFAULTTONEAREST)` wrapped unconditionally in `Some`.
/// - Linux/GTK: the backend already applies this exact primary fallback
///   internally, so `None` means even the primary is unavailable.
/// - macOS: `NSWindow::screen()` is nil only when the window is offscreen.
///
/// `match` rather than `.or(..)`: `.or` is EAGER, so it would call
/// `primary_monitor()` on every lookup even when the current monitor is
/// already in hand - and its `?` would then propagate a teardown-race error
/// out of a caller that had everything it needed. The fallback is a cold path.
///
/// `Ok(None)` and `Err(e)` stay DISTINCT arms. An exhausted lookup and a failed
/// one are different diagnostics; collapsing them discards the backend's error.
///
/// Both getters are blocking round-trips to the event-loop thread
/// (tauri-runtime-wry's `window_getter!` posts a user message and blocks on
/// `rx.recv()` with no timeout), and that thread takes [`PREVIEW_RUNTIME`] in
/// the window-event handlers. MUST NOT be called while that mutex is held.
/// [`DOCK_TRANSITION`] is fine - the event loop never takes it.
#[cfg(not(test))]
fn resolve_dock_monitor(window: &WebviewWindow) -> Result<Monitor, String> {
    match window
        .current_monitor()
        .map_err(|error| format!("Failed to read the current monitor: {error}"))?
    {
        Some(monitor) => Some(monitor),
        None => window
            .primary_monitor()
            .map_err(|error| format!("Failed to read the primary monitor: {error}"))?,
    }
    .ok_or_else(|| "Could not determine which monitor Hive Manager is on".to_string())
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

    // Shared with `follow_docked_preview` so the two cannot drift apart on the
    // fallback. Taken before the runtime lock, never under it.
    let monitor = resolve_dock_monitor(&main)?;

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
    // Ordering here is the whole of issue #163: `tile_docked_windows` drops the
    // preview out of maximize before it can position it, so read after the
    // tiling this is unconditionally `false` and every restore below becomes a
    // no-op.
    let preview_was_maximized = preview.is_maximized().unwrap_or(false);

    // `stage_dock` borrows the state immutably, so nothing here can commit the
    // docked state early.
    let staged = {
        let guard = runtime(app_state).lock();
        guard.state.stage_dock(
            main_geometry,
            main_was_maximized,
            preview_geometry,
            preview_was_maximized,
        )
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

    // Exclusive branches, exactly like the main window below - but the maximized
    // arm puts a rect back FIRST, and it is deliberately the OTHER rect.
    // `preview_geometry` is a LIVE read taken before the tiling, so for a
    // maximized preview it holds the MAXIMIZED rect: replaying that would leave
    // the window unmaximized at maximized pixels, overhanging the work area by
    // the invisible resize border and showing the maximize glyph where the
    // operator left a restore glyph. `previous.floating` is the remembered
    // RESTORED-DOWN geometry - the very rect `leave_docked_mode` replays - and it
    // has to go back BEFORE the re-maximize: `tile_docked_windows` has already
    // unmaximized this window and moved it into the docked column, so the column
    // is now the window's OWN restore-down target. Re-maximizing without
    // reinstating the rect leaves the operator's next restore click landing in
    // the docked column instead of where they left the window.
    if staged.preview_was_maximized {
        if let Some(geometry) = staged.previous.floating {
            // Through the remembered-geometry helper, not bare `apply_geometry`:
            // this is the same persisted field every other replay site routes
            // through (`leave_docked_mode`, the reopen path,
            // `restore_main_window_geometry`), and the helper declines a rect no
            // connected monitor covers. A laptop undocked from an external
            // display would otherwise be reinstated offscreen, and re-maximizing
            // an offscreen window maximizes it onto a display the operator
            // cannot see.
            apply_remembered_geometry(preview, geometry);
        }
        if let Err(error) = preview.maximize() {
            tracing::warn!("Failed to re-maximize the preview after a failed dock: {error}");
        }
    } else if let Some(geometry) = staged.preview_geometry {
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

    let (floating, was_maximized) = {
        let mut guard = runtime(app_state).lock();
        guard.state.docked = false;
        let taken = guard.state.take_preview_restore();
        persist(&guard);
        taken
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

    // AFTER the rect, not instead of it. `floating` is the RESTORED-DOWN
    // geometry - the `Moved`/`Resized` handler declines to record a maximized
    // window - so applying it first is what gives the re-maximized preview a
    // sane rect to restore down INTO, rather than the docked column the tiler
    // left as its last non-maximized size.
    if was_maximized {
        if let Err(error) = preview.maximize() {
            tracing::warn!("Failed to re-maximize the preview after undocking: {error}");
        }
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
    // The SAME resolution the dock used, fallback included. This runs on a
    // spawned task off the window-event callback, holding neither mutex.
    let monitor = resolve_dock_monitor(&main)?;

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

/// Record where the operator left the FLOATING preview - run off the
/// window-event callback, after the drag storm has settled.
///
/// Every check in here is re-derived at CALL time rather than captured when the
/// event that scheduled this fired. That is the whole point of deferring: a dock
/// or a maximize landing inside the coalescing window has to be seen by these
/// checks, not by a snapshot taken before it happened.
///
/// Three of the four steps below are blocking round-trips to the event-loop
/// thread, which is why the caller must never run this per event: a drag
/// delivers `Moved` at frame rate, and paying four round-trips a frame is felt
/// as a stuttering drag. For the same reason the runtime mutex is never held
/// across them - that thread takes this very mutex in the handlers that post
/// these events, so holding it across a getter deadlocks both windows.
#[cfg(not(test))]
fn record_floating_geometry(app: &AppHandle, app_state: &Arc<AppState>) {
    let Some(window) = app.get_webview_window(PREVIEW_WINDOW_LABEL) else {
        return;
    };
    // Cheapest bail first, and before any window getter: a docked preview
    // records nothing. The write below re-checks under the lock, so a dock
    // landing between the two reads still cannot slip a column into `floating`.
    if runtime(app_state).lock().state.docked {
        return;
    }
    // `floating` is the rect the preview is restored TO, so a maximized window
    // must not write it. Recording the maximized rect there both destroys the
    // pre-maximize rect the restore actually wants and turns every later replay
    // into the unmaximized-at-maximized-pixels artefact of issue #163. Leaving
    // the field alone keeps exactly the rect a restore-down would land on.
    if window.is_maximized().unwrap_or(false) {
        return;
    }
    let Some(geometry) = read_geometry(&window) else {
        return;
    };
    // Held in memory and flushed on close/dock/undock rather than written on
    // every frame of a drag.
    let mut guard = runtime(app_state).lock();
    if !guard.state.docked {
        guard.state.floating = Some(geometry);
    }
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
            // Cheapest bail first, and before anything is even scheduled: a
            // docked preview records nothing, so the retile storm this arm sees
            // while docked should not cost a spawned task per event either.
            // `record_floating_geometry` re-checks, so a dock landing inside the
            // debounce window is still caught.
            if runtime(&preview_state).lock().state.docked {
                return;
            }
            // Coalesce the event storm a window drag produces, exactly as the
            // main window's retile above does, and get off the window-event
            // callback before touching window geometry. Recording per event cost
            // four blocking round-trips to THIS thread per frame of a drag,
            // which is felt as stutter; deferring also means the rect finally
            // recorded is the one read after the operator let go, rather than
            // the last of several hundred intermediate ones.
            //
            // Nothing is captured for the deferred task to act on. It re-reads
            // the dock bit, the maximize bit and the geometry when it runs, so a
            // dock or a maximize landing inside the debounce window changes the
            // outcome instead of being overwritten by a stale enqueue-time read.
            //
            // Neither teardown NOR the dock depends on this task to flush the
            // final rect, so a pending pass being dropped by one of them costs
            // nothing. `close_preview_window` takes its own live read before
            // destroying the window, and `enter_docked_mode` takes one before
            // tiling, which `commit_dock` then writes - both of them the rect
            // the operator actually let go of. The one path with no read of its
            // own is the titlebar X, which reaches only `WindowEvent::Destroyed`
            // and by then has no window left to read: a drag released and X-ed
            // inside RETILE_DEBOUNCE_MS keeps the previous pass's rect instead
            // of the last few pixels. Left as-is deliberately - the flag is
            // claimed on the FIRST event, so a sustained drag records about
            // every RETILE_DEBOUNCE_MS and only the tail is ever at stake, and
            // closing that tail would mean giving the X a geometry read that the
            // window's destruction has already made impossible.
            if FLOATING_RECORD_PENDING.swap(true, Ordering::SeqCst) {
                return;
            }
            let record_app = preview_app.clone();
            let record_state = Arc::clone(&preview_state);
            tauri::async_runtime::spawn(async move {
                tokio::time::sleep(std::time::Duration::from_millis(RETILE_DEBOUNCE_MS)).await;
                // Cleared BEFORE the record, so an event arriving while this
                // runs schedules the next pass rather than being swallowed.
                FLOATING_RECORD_PENDING.store(false, Ordering::SeqCst);
                record_floating_geometry(&record_app, &record_state);
            });
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

    // A close is not an undock, so the maximize bit is still owed here: without
    // this, closing a maximized-then-docked preview and reopening it loses the
    // state just as permanently as the undock did.
    //
    // Gated on the surviving dock PREFERENCE, because that decides who the bit
    // is owed TO. With the preference set, `open_preview_window` re-tiles this
    // window the moment it is shown - maximizing it here would be undone
    // immediately AND would consume a bit the re-tile cannot recapture (see
    // `commit_dock`'s gate), so it is left in place for the undock after it.
    let (floating, was_maximized) = {
        let mut guard = runtime(app_state).lock();
        if guard.state.docked {
            (guard.state.floating, false)
        } else {
            let taken = guard.state.take_preview_restore();
            persist(&guard);
            taken
        }
    };
    if let Some(geometry) = floating {
        apply_remembered_geometry(&window, geometry);
    }
    // Same ordering as the undock: the remembered rect first, so the window has
    // something sane to restore down into.
    if was_maximized {
        if let Err(error) = window.maximize() {
            tracing::warn!("Failed to reopen the preview maximized: {error}");
        }
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
                // Clearing the preference makes THIS window the floating one, so
                // it is also the rightful consumer of the maximize bit that
                // `create_preview_window` deliberately left for an undock that
                // will now never come - the undock control is gated on `docked`.
                // Left set behind `docked: false` the bit is replayed onto some
                // later, unrelated preview instead, which is exactly the
                // stranding `take_preview_restore` and
                // `a_dock_whose_tiling_fails_leaves_the_preview_maximize_bit_alone`
                // exist to rule out.
                let (_, was_maximized) = {
                    let mut guard = runtime(&state).lock();
                    guard.state.docked = false;
                    let taken = guard.state.take_preview_restore();
                    persist(&guard);
                    taken
                };
                // `floating` is deliberately dropped rather than replayed: the
                // rect is already on this window - applied at creation, and
                // re-applied by `undo_failed_dock` on the tiling-failure path -
                // so it is already the sane rect to restore down INTO. Same
                // ordering as `leave_docked_mode`, and the lock is released
                // first: `maximize` is a blocking round-trip to the event-loop
                // thread, which takes this same mutex.
                if was_maximized {
                    if let Err(error) = window.maximize() {
                        tracing::warn!(
                            "Failed to maximize the preview after a downgraded dock: {error}"
                        );
                    }
                }
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
        // Same contract as the `Moved`/`Resized` handler and `commit_dock`:
        // `floating` is the rect the preview is restored TO, so a maximized
        // window records nothing. Recording it here made the in-app Close button
        // destroy the pre-maximize rect that the titlebar X - which reaches only
        // `WindowEvent::Destroyed` and records no geometry at all - leaves
        // intact, so one operator gesture produced two different outcomes
        // depending on which chrome was clicked. Skipping is what keeps the two
        // teardown paths in step.
        //
        // Both getters run with the runtime mutex RELEASED: this is an async
        // command, so it never short-circuits onto the main thread, and the
        // event-loop thread it posts to takes this very mutex in the handlers
        // above. Re-checked under the lock below, so a dock landing between the
        // two reads still cannot slip a column into `floating`.
        let geometry = if window.is_maximized().unwrap_or(false) {
            None
        } else {
            read_geometry(&window)
        };
        {
            let mut guard = runtime(&state).lock();
            if !guard.state.docked {
                if let Some(geometry) = geometry {
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

    /// What `read_geometry` actually returns for a MAXIMIZED preview: the work
    /// area outset by the invisible resize border, so it overhangs on every
    /// side. Deliberately NOT the floating rect from [`staging_reads`] - pairing
    /// that rect with `preview_was_maximized: true` is a state the live reads in
    /// `enter_docked_mode` cannot produce, and fixtures that did it are what let
    /// the maximized commit path go unnoticed.
    fn maximized_read() -> WindowGeometry {
        WindowGeometry {
            x: -8,
            y: -8,
            width: 1936.0,
            height: 1056.0,
        }
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

        let staged = state.stage_dock(Some(main_before), false, Some(preview_before), false);

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

        // `preview_before` is the FLOATING rect, so the maximize bit has to be
        // `false`: a maximized window's live read can never be that rect, and
        // the pairing is what this test asserts the commit of. The maximized
        // case is `a_dock_over_a_maximized_preview_keeps_the_remembered_rect`.
        let staged = state.stage_dock(Some(main_before), true, Some(preview_before), false);
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
        // The operator had the preview maximized when the live layout was
        // established, so the undock after it still owes a re-maximize.
        state.preview_was_maximized = true;
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
        // The tiler unmaximized the preview to place it, so this read is
        // `false` even though the operator's remembered state is `true`.
        let staged = state.stage_dock(Some(tiled_main), false, Some(tiled_preview), false);
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
        assert!(
            state.preview_was_maximized,
            "the remembered preview-maximize bit belongs to the live layout's \
             snapshot, not to this re-tile: the tiler unmaximized the preview \
             when the layout was established, so a failed re-dock that let the \
             staged `false` through would leave the undock after it with \
             nothing to re-maximize"
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
        // The operator had the preview maximized when the live layout was
        // established, so the undock after it still owes a re-maximize.
        state.preview_was_maximized = true;
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
        // The tiler unmaximized the preview to place it, so this read is
        // `false` even though the operator's remembered state is `true`.
        let staged = state.stage_dock(Some(tiled_main), false, Some(tiled_preview), false);
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
        assert!(
            state.preview_was_maximized,
            "the preview-maximize bit must share the floating snapshot's \
             `!docked` gate (issue #163). By the time a re-tile stages, the \
             tiler has already unmaximized the preview, so the staged read is a \
             guaranteed `false` - committing it ungated erases the operator's \
             remembered state on the reopen path, which re-docks off the \
             surviving preference on every single open"
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
            "preview.is_maximized()",
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

    // -- issue #163: the preview's maximized state survives a dock -----------

    /// The same text with `//` line comments AND `/* … */` block comments
    /// stripped.
    ///
    /// Every "the code still calls X" assertion below runs through this. A bare
    /// `contains` is satisfied the moment someone TYPES the characters - a
    /// comment saying "we should call `preview.maximize()` here" turns the
    /// assertion permanently green while the call is gone, which is exactly the
    /// failure mode these locks exist to catch.
    ///
    /// Block comments have to go with them. Commenting out a multi-line
    /// selection is what an editor's comment shortcut emits, so stripping only
    /// `//` left the likeliest way one of these calls actually gets deleted
    /// completely unguarded - and it fails SILENTLY, in the green direction.
    ///
    /// Newlines spanned by a stripped block are kept, so the surviving text
    /// keeps its line structure. Only sound because no function it is pointed
    /// at contains `//` or `/*` inside a string literal; such a literal would
    /// be truncated here, which shows up as a failing assertion rather than a
    /// silent pass. Rust block comments nest and this scanner does not track
    /// depth, so a contrived `/* /* */ preview.maximize() */` would still leak.
    fn without_comments(source: &str) -> String {
        let mut out = String::with_capacity(source.len());
        let mut rest = source;
        loop {
            let line = rest.find("//");
            let block = rest.find("/*");
            let line_first = match (line, block) {
                (Some(line_at), Some(block_at)) => line_at < block_at,
                (Some(_), None) => true,
                _ => false,
            };
            if line_first {
                let at = line.expect("line_first is only set with a `//` in hand");
                out.push_str(&rest[..at]);
                match rest[at..].find('\n') {
                    Some(newline) => rest = &rest[at + newline..],
                    None => return out,
                }
            } else if let Some(at) = block {
                out.push_str(&rest[..at]);
                let body = &rest[at..];
                // An unterminated `/*` comments out everything after it, so
                // dropping the whole tail is the correct reading rather than a
                // fallback - and it leaves the assertions with no call text to
                // match, i.e. it fails toward RED.
                let end = body.find("*/").map_or(body.len(), |close| close + 2);
                out.extend(body[..end].chars().filter(|character| *character == '\n'));
                rest = &body[end..];
            } else {
                out.push_str(rest);
                return out;
            }
        }
    }

    /// A dock that tiles must remember that it took a MAXIMIZED preview over.
    ///
    /// `tile_docked_windows` unmaximizes the preview before it can position it,
    /// and nothing recorded that, so the state was simply destroyed: the undock
    /// dropped the preview back to `floating` un-maximized every time.
    #[test]
    fn a_dock_that_tiles_remembers_the_preview_was_maximized() {
        let (main_before, _) = staging_reads();
        let mut state = PersistedPreviewState::default();

        let staged = state.stage_dock(Some(main_before), false, Some(maximized_read()), true);
        // Staging only borrows: nothing is remembered until the tiling lands.
        assert!(
            !state.preview_was_maximized,
            "stage_dock must not commit the maximize bit either - it takes \
             `&self` precisely so no field can be written before the tiling \
             result is known"
        );

        let tiled: Result<(), &str> = Ok(());
        state.settle_dock(staged, &tiled);

        assert!(
            state.preview_was_maximized,
            "a successful dock must remember the pre-dock maximized state, or \
             the undock has nothing to restore"
        );
    }

    /// `floating` is the RESTORED-DOWN rect, and `commit_dock` is the second
    /// writer of it that the `Moved`/`Resized` guard does not cover.
    ///
    /// `preview_geometry` is a LIVE read taken before `tile_docked_windows`
    /// unmaximizes anything, so for a maximized preview it is the MAXIMIZED
    /// rect. Committing that destroys the operator's remembered position on the
    /// exact maximize-then-dock path this issue is about - and because
    /// `leave_docked_mode` and `create_preview_window` replay the rect and THEN
    /// re-maximize (non-exclusive, unlike the main window's branches), the
    /// maximized rect becomes the re-maximized window's own restore-down rect.
    /// One restore click later the preview sits unmaximized at maximized pixels,
    /// which is the artefact the issue opens with.
    #[test]
    fn a_dock_over_a_maximized_preview_keeps_the_remembered_rect() {
        let (main_before, preview_before) = staging_reads();
        let mut state = PersistedPreviewState::default();
        state.floating = Some(preview_before);

        let staged = state.stage_dock(Some(main_before), false, Some(maximized_read()), true);

        // Stand in for the preview's own handler, which runs while the dock is
        // in flight: the tiler unmaximizes the preview BEFORE positioning it, so
        // the guard at the top of that arm no longer bails and the docked column
        // lands here. This is why the fix reassigns rather than skipping.
        state.floating = Some(WindowGeometry {
            x: 768,
            y: 0,
            width: 512.0,
            height: 1040.0,
        });

        let tiled: Result<(), &str> = Ok(());
        state.settle_dock(staged, &tiled);

        assert!(
            state.preview_was_maximized,
            "the bit still has to land - this test constrains the rect, not the \
             bit, and must fail for the right reason"
        );
        let (floating, was_maximized) = state.take_preview_restore();
        assert!(was_maximized);
        assert_eq!(
            floating.map(|geometry| (geometry.x, geometry.y, geometry.width, geometry.height)),
            Some((900, 120, 800.0, 600.0)),
            "a dock that takes over a MAXIMIZED preview must keep the remembered \
             restored-down rect: the staged read is the maximized rect, and the \
             undock applies `floating` and THEN re-maximizes, so committing it \
             leaves the window restoring down to maximized pixels. It must also \
             not keep the docked COLUMN the in-flight handler recorded - hence a \
             write-back rather than a skip"
        );
    }

    /// The failure half: a dock that never tiled never took the preview out of
    /// maximize, so it owes no restore and must not claim one.
    #[test]
    fn a_dock_whose_tiling_fails_leaves_the_preview_maximize_bit_alone() {
        let (main_before, preview_before) = staging_reads();
        let mut state = PersistedPreviewState::default();
        state.floating = Some(preview_before);

        let staged = state.stage_dock(Some(main_before), false, Some(preview_before), true);
        let tiled: Result<(), &str> = Err("Failed to resize the preview: os error 5");
        state.settle_dock(staged, &tiled);

        assert!(
            !state.preview_was_maximized,
            "a failed dock must leave NO trace in the state machine (#160): a \
             remembered maximize stranded behind `docked: false` is consumed by \
             whichever unrelated path replays the floating rect next"
        );
        // The physical restore for this path rides `StagedDock`, not the
        // persisted field - the window still IS maximized, and `undo_failed_dock`
        // is what has to leave it that way.
        assert!(
            staged.preview_was_maximized,
            "the staged read must survive the rollback: it is the only record \
             that the window the tiler unmaximized had been maximized"
        );
    }

    /// The bit is a ONE-SHOT. `floating` is not.
    #[test]
    fn the_remembered_preview_maximize_bit_is_consumed_by_its_restore() {
        let (_, preview_before) = staging_reads();
        let mut state = PersistedPreviewState::default();
        state.floating = Some(preview_before);
        state.preview_was_maximized = true;

        let (floating, was_maximized) = state.take_preview_restore();
        assert!(was_maximized, "the restore must see the remembered bit");
        assert_eq!(
            floating.map(|geometry| (geometry.x, geometry.y)),
            Some((900, 120)),
            "the restore must get the floating rect that belongs with the bit"
        );

        let (floating, was_maximized) = state.take_preview_restore();
        assert!(
            !was_maximized,
            "the bit must be CONSUMED by its restore: left set, it outlives the \
             window it was read from and re-maximizes the next, unrelated \
             preview the operator opens"
        );
        assert_eq!(
            floating.map(|geometry| (geometry.x, geometry.y)),
            Some((900, 120)),
            "`floating` must NOT be consumed alongside it - it is the remembered \
             position, replayed on every open for the life of the install"
        );
    }

    /// The persisted file predates the field, and an operator upgrading into
    /// this must not have their dock preference silently reset to defaults.
    #[test]
    fn a_preview_state_written_before_the_maximize_bit_existed_still_loads() {
        let legacy = r#"{
            "docked": true,
            "floating": {"x": 900, "y": 120, "width": 800.0, "height": 600.0},
            "main_restore": {"x": 64, "y": 32, "width": 1280.0, "height": 800.0},
            "main_was_maximized": true,
            "layout_applied": true,
            "session_urls": {}
        }"#;

        let state: PersistedPreviewState = serde_json::from_str(legacy).expect(
            "an existing preview-window-state.json must keep loading: without \
             #[serde(default)] the new field makes the whole file unreadable, \
             and the fallback to `default()` silently drops the operator's dock \
             preference, remembered position and restore snapshot",
        );
        assert!(state.docked);
        assert!(state.layout_applied);
        assert!(state.main_was_maximized);
        assert!(
            !state.preview_was_maximized,
            "a file that never recorded the bit reads as `not maximized`"
        );

        let mut fresh = PersistedPreviewState::default();
        fresh.preview_was_maximized = true;
        let round_tripped: PersistedPreviewState =
            serde_json::from_str(&serde_json::to_string(&fresh).expect("state must serialize"))
                .expect("state must round-trip");
        assert!(
            round_tripped.preview_was_maximized,
            "the bit must survive a restart, or closing Hive Manager while \
             docked loses it just as permanently as the undock did"
        );
    }

    /// The asymmetry IS the bug, so this asserts SYMMETRY rather than presence.
    ///
    /// `undo_failed_dock` restored the main window's maximized state and not the
    /// preview's. Fixing only the failure path - as the original review comment
    /// suggested - would have produced the mirror-image asymmetry instead: a
    /// FAILED dock preserving the state while a successful dock-then-undock
    /// destroyed it, two outcomes from one button depending on whether the
    /// tiling happened to error.
    ///
    /// The whole path is `#[cfg(not(test))]` and needs live windows, so - same
    /// discipline as the #159/#160 locks above - it is pinned against the
    /// source text.
    #[test]
    fn a_failed_dock_restores_both_windows_maximized_state_or_neither() {
        let source = include_str!("mod.rs").replace("\r\n", "\n");
        let undo = without_comments(body_of(&source, "undo_failed_dock"));

        let main_sites = undo.matches("main.maximize()").count();
        let preview_sites = undo.matches("preview.maximize()").count();
        assert!(
            main_sites > 0,
            "the physical undo must still re-maximize the main window"
        );
        assert_eq!(
            main_sites, preview_sites,
            "the two windows must come out of a failed dock the same way: the \
             tiler unmaximizes BOTH of them, so a restore that covers only one \
             is the #163 asymmetry"
        );

        let branch = undo
            .find("if staged.preview_was_maximized")
            .expect("the preview restore must branch on the staged read");
        let replay = undo
            .find("staged.preview_geometry")
            .expect("the non-maximized case must still replay the staged rect");
        assert!(
            branch < replay,
            "the maximize check must GATE the geometry replay, not follow it: \
             `preview_geometry` is a live read taken before the tiling, so for a \
             maximized preview it holds the MAXIMIZED rect - replaying it leaves \
             the window unmaximized at maximized pixels, overhanging the work \
             area by the invisible resize border, which is the artefact the \
             issue opens with"
        );
        assert!(
            undo.contains("} else if let Some(geometry) = staged.preview_geometry"),
            "the two cases must stay EXCLUSIVE, exactly like the main window's: \
             applying the maximized rect and then re-maximizing over it \
             overwrites the window's own restore-down rect with the maximized one"
        );
    }

    /// A failed dock must put the remembered rect back BEFORE it re-maximizes.
    ///
    /// The test above counts `preview.maximize()` SITES, so it stays green with
    /// the rect application missing entirely - which is exactly the state this
    /// path was in. `tile_docked_windows` unmaximizes the preview and moves it
    /// into the docked column before the tiling errors, so by the time the undo
    /// runs, that column IS the window's own restore-down target. Re-maximizing
    /// on top of it hides the damage until the operator's next restore click,
    /// which lands them in the column rather than where they left the window.
    ///
    /// The rect to reinstate is `previous.floating`, NOT `preview_geometry`:
    /// the latter is the live pre-tiling read, so for a maximized preview it is
    /// the maximized rect, and replaying THAT is issue #163 itself. The former is
    /// the remembered restored-down geometry - the same rect `leave_docked_mode`
    /// replays, which is what makes the two restore paths agree.
    ///
    /// `#[cfg(not(test))]` and needs live windows, so - same discipline as the
    /// locks around it - pinned against the source text, comments stripped so
    /// prose about the call cannot satisfy it.
    #[test]
    fn a_failed_dock_reinstates_the_remembered_rect_before_re_maximizing() {
        let source = include_str!("mod.rs").replace("\r\n", "\n");
        let undo = without_comments(body_of(&source, "undo_failed_dock"));

        let branch = undo
            .find("if staged.preview_was_maximized")
            .expect("the preview restore must still branch on the staged read");
        // Only the maximized arm is under test; the `else if` below it already
        // has its own rect application and its own lock.
        let arm = &undo[branch..];

        let remembered = arm.find("staged.previous.floating").expect(
            "the maximized arm must reinstate the REMEMBERED restore-down rect: \
             with no rect applied at all, the re-maximize inherits the docked \
             column the tiler left as this window's last un-maximized size, and \
             the operator's next restore click lands them in it",
        );
        // Specifically the REMEMBERED-geometry helper, not bare `apply_geometry`.
        // `previous.floating` is persisted, so it can name a monitor that is no
        // longer connected; the helper declines a rect no display covers, and
        // every other replay site for this field routes through it. Bare
        // `apply_geometry` here would reinstate an offscreen rect and then
        // maximize onto a display the operator cannot see.
        let applied = arm
            .find("apply_remembered_geometry(preview, geometry)")
            .expect(
                "the maximized arm must APPLY the remembered rect through the \
                 monitor-checked helper, not merely name it and not via bare \
                 apply_geometry",
            );
        let maximize = arm
            .find("preview.maximize()")
            .expect("the maximized arm must still re-maximize the preview");

        assert!(
            remembered < maximize && applied < maximize,
            "the rect must go back BEFORE the re-maximize, exactly as \
             `leave_docked_mode` does it: applied afterwards it is overwritten \
             by the maximize, which leaves the restore-down target pointing at \
             the docked column - the asymmetry between the two restore paths \
             that this pins shut"
        );
        assert!(
            arm.contains("if let Some(geometry) = staged.previous.floating {"),
            "the reinstatement must be gated on the remembered rect EXISTING: \
             there is no floating rect to put back before the operator has ever \
             moved the preview, and the assertions above match an ungated \
             unwrap just as well"
        );
    }

    /// Every path that replays the remembered floating rect must replay the
    /// maximize bit with it - including the reopen, which the issue's acceptance
    /// criteria omit.
    ///
    /// Excluding `create_preview_window` would leave close-while-maximized then
    /// reopen losing the state, i.e. the same bug through a different door.
    #[test]
    fn every_path_that_replays_the_remembered_rect_replays_the_maximize_bit() {
        let source = include_str!("mod.rs").replace("\r\n", "\n");
        // Runtime half only: this test names the calls it looks for, so
        // searching the whole file would let it pass against its own text.
        let runtime_source = &source[..source
            .find("mod tests {")
            .expect("the test module must still terminate the runtime source")];

        for name in ["leave_docked_mode", "create_preview_window"] {
            let body = without_comments(body_of(runtime_source, name));
            assert!(
                body.contains("take_preview_restore()"),
                "{name} must read the remembered maximize bit through the \
                 CONSUMER: a bit that is only read stays set after the window it \
                 describes is gone, and gets replayed onto the next preview the \
                 operator opens"
            );
            let replay = body.find("apply_remembered_geometry").unwrap_or_else(|| {
                panic!("{name} must still replay the remembered floating rect")
            });
            let maximize = body.find(".maximize()").unwrap_or_else(|| {
                panic!(
                    "{name} restores the preview's floating RECT but not its \
                     maximized state - that is issue #163 itself"
                )
            });
            assert!(
                replay < maximize,
                "{name} must apply the remembered rect BEFORE re-maximizing: \
                 `floating` holds the RESTORED-DOWN geometry, so applying it \
                 first is what leaves the re-maximized window a sane rect to \
                 restore down into instead of the docked column the tiler left \
                 as its last un-maximized size"
            );
            assert!(
                body.contains("if was_maximized {"),
                "{name} must re-maximize when the remembered bit is SET. Every \
                 assertion above pins only that `.maximize()` is PRESENT and \
                 ordered after the rect replay - an inverted `if !was_maximized` \
                 satisfies all of them identically while re-maximizing exactly \
                 when the preview was NOT maximized, which is issue #163 plus a \
                 spurious maximize. This path is `#[cfg(not(test))]`, so no \
                 behavioural test compiles it: this literal is the only lock"
            );
        }

        // The fourth replay site. `open_preview_window`'s downgrade branch fires
        // when a reopen's re-tile fails, which makes THIS window the floating one
        // and so the rightful consumer of a bit `create_preview_window`
        // deliberately left for an undock the `docked: false` now rules out.
        let downgrade = without_comments(body_of(runtime_source, "open_preview_window"));
        let cleared = downgrade
            .find("guard.state.docked = false;")
            .expect("the downgrade must clear the dock preference");
        let taken = downgrade.find("take_preview_restore()").expect(
            "the downgrade must CONSUME the maximize bit: left set behind a \
             `docked: false` no path in this session can drain - the undock \
             control is gated on `docked` - it is replayed onto some later, \
             unrelated preview instead",
        );
        assert!(
            cleared < taken,
            "the preference must be cleared BEFORE the bit is taken, so the \
             persist that follows cannot write a stranded pairing"
        );
        assert!(
            downgrade.contains("if was_maximized {"),
            "the downgrade must honour the bit it just consumed, not silently \
             discard the restore the operator is owed"
        );

        // The read has to happen before the tiler destroys what it is reading.
        let dock = without_comments(body_of(runtime_source, "enter_docked_mode"));
        let read = dock
            .find("preview.is_maximized()")
            .expect("the dock must read the preview's maximized state at all");
        let tile = dock
            .find("tile_docked_windows(")
            .expect("the dock must still issue its geometry through the tiler");
        assert!(
            read < tile,
            "the preview's maximized state must be read BEFORE the tiling: \
             `tile_docked_windows` unmaximizes the preview to position it, so a \
             read taken afterwards is unconditionally `false` and every restore \
             above degrades into a silent no-op"
        );

        // `floating` means the RESTORED-DOWN rect. The recorder that maintains
        // it is what makes that true. It lives behind the Moved/Resized arm's
        // debounce now (see `a_preview_drag_storm_pays_for_no_window_getters`),
        // so the guard is pinned where the getters actually are.
        let recorder = without_comments(body_of(runtime_source, "record_floating_geometry"));

        let maximized = recorder.find("is_maximized()").expect(
            "the preview's floating recorder must SKIP a maximized window: \
             `floating` is the rect the preview is restored TO, so recording a \
             maximized rect there destroys the pre-maximize rect the restore \
             wants and makes every replay above a no-op-looking reinstatement of \
             the maximized geometry",
        );
        let record = recorder
            .find("state.floating = Some(geometry)")
            .expect("the recorder must still record the floating rect");
        assert!(
            maximized < record,
            "the maximized check must come BEFORE the record, or it guards \
             nothing"
        );
        let guard = recorder
            .find("guard = runtime(")
            .expect("the recorder must take the runtime lock to record");
        assert!(
            maximized < guard,
            "`is_maximized()` must be read with the runtime mutex RELEASED: it \
             posts a message to the event-loop thread and blocks on the reply, \
             and that thread takes this very mutex in the window handlers - \
             taking it across the read deadlocks both windows permanently"
        );
        assert!(
            recorder.contains("if window.is_maximized().unwrap_or(false) {"),
            "the recorder's guard must SKIP the maximized window, not skip the \
             restored-down one: `recorder.find(\"is_maximized()\")` above matches \
             an inverted guard just as well, and an inverted guard records ONLY \
             maximized rects into `floating` - issue #163 reinstated through the \
             recorder instead of the restore"
        );
    }

    /// The preview's `Moved`/`Resized` arm must not run a single window getter.
    ///
    /// It used to run four - `is_maximized`, then `outer_position`,
    /// `outer_size` and `scale_factor` inside `read_geometry` - inline on the
    /// window-event callback. Each is a blocking round-trip to the event-loop
    /// thread, and a drag delivers this event at frame rate, so the operator
    /// paid all four per frame and felt it as stutter. The `docked` bail helps
    /// only while docked; the floating case, which is exactly when a drag storm
    /// happens, paid in full.
    ///
    /// The fix is the coalescing idiom already in this file for the main
    /// window's retile, so this pins the four properties that make it correct
    /// rather than merely present: the arm is getter-free, the deferred task
    /// waits, it clears its own flag, and the checks it runs are re-derived at
    /// fire time instead of captured at enqueue time.
    ///
    /// `#[cfg(not(test))]` and driven by real window events, so - same
    /// discipline as the locks around it - pinned against the source text.
    #[test]
    fn a_preview_drag_storm_pays_for_no_window_getters() {
        let source = include_str!("mod.rs").replace("\r\n", "\n");
        let runtime_source = &source[..source
            .find("mod tests {")
            .expect("the test module must still terminate the runtime source")];
        let wiring = without_comments(body_of(runtime_source, "wire_window_events"));
        let preview_arm = wiring
            .find("preview.on_window_event")
            .expect("the preview's own window events must still be wired");
        let arm = &wiring[preview_arm..];
        let arm = &arm[..arm
            .find("WindowEvent::Destroyed")
            .expect("the preview handler must still have its Destroyed arm")];

        for getter in [
            "is_maximized()",
            "read_geometry(",
            "outer_position()",
            "outer_size()",
            "scale_factor()",
        ] {
            assert!(
                !arm.contains(getter),
                "the Moved/Resized arm must not call `{getter}` inline: it is a \
                 blocking round-trip to the event-loop thread this callback is \
                 running ON, and a drag delivers this arm an event per frame"
            );
        }

        // Pinned by SHAPE, not just presence. Inverting this guard to
        // `if !FLOATING_RECORD_PENDING.swap(..)` is a one-character edit that
        // flips the polarity: the first event would return without scheduling
        // anything and latch the flag true forever, so the preview would never
        // record its position again for the life of the process — and a
        // presence-and-position check stays green through all of it. The
        // maximized-check assertion above pins its literal for the same reason.
        assert!(
            arm.contains("if FLOATING_RECORD_PENDING.swap(true, Ordering::SeqCst) {"),
            "the coalescing guard must claim the flag and bail on the ALREADY-PENDING \
             branch: an inverted guard latches the flag on the first event and \
             silently stops recording forever"
        );
        let swap = arm
            .find("FLOATING_RECORD_PENDING.swap(true, Ordering::SeqCst)")
            .expect(
                "the arm must coalesce the storm behind a pending flag, the same \
                 way the main window's retile above does",
            );
        let sleep = arm
            .find("tokio::time::sleep")
            .expect("the deferred task must WAIT for the storm to settle");
        let clear = arm
            .find("FLOATING_RECORD_PENDING.store(false, Ordering::SeqCst)")
            .expect(
                "the deferred task must clear the pending flag, or the FIRST \
                 drag latches it forever and the preview never records its \
                 position again for the life of the process",
            );
        let record = arm.find("record_floating_geometry(").expect(
            "the deferred task must still record the floating rect - a debounce \
             that coalesces every event into nothing is not a debounce",
        );

        assert!(
            swap < sleep && sleep < clear && clear < record,
            "the order is load-bearing: the flag is claimed before the wait (so \
             the storm collapses to one task), the wait precedes the clear and \
             the record (so the getters run ONCE, after the operator let go), \
             and the clear precedes the record (so an event arriving mid-record \
             schedules the next pass instead of being swallowed)"
        );

        // The whole reason the deferred work is a call and not an inline block:
        // everything it decides on has to be re-read when it fires. A `docked`
        // or `is_maximized` captured at enqueue time is a snapshot from before
        // the debounce window, and a dock landing inside that window would then
        // write its column into `floating`.
        let recorder = without_comments(body_of(runtime_source, "record_floating_geometry"));
        assert!(
            recorder.contains("state.docked") && recorder.contains("is_maximized()"),
            "the deferred recorder must re-check BOTH the dock bit and the \
             maximize bit itself, at the time it runs"
        );
        let deferred_reads = arm
            .find("record_floating_geometry(&record_app, &record_state)")
            .expect(
                "the deferred task must hand the recorder the handles and let it \
                 do its own reads, not pass it a rect read on this callback",
            );
        assert!(
            sleep < deferred_reads,
            "the recorder's reads must happen AFTER the wait, or the debounce \
             has only moved the per-event cost onto another thread"
        );
    }

    /// The reopen's maximize replay must stay GATED on the surviving dock
    /// preference.
    ///
    /// `a_successful_re_dock_keeps_the_live_layouts_snapshots` pins the other
    /// half of this invariant, but it sets `preview_was_maximized` directly - it
    /// ASSUMES the bit is still there when the re-dock stages. This gate is what
    /// makes that assumption true. Ungated, `create_preview_window` consumes the
    /// bit on a docked reopen and spends it on a maximize the re-tile
    /// immediately undoes; `commit_dock`'s `!previous.docked` gate then
    /// correctly declines to recapture it, so the undock after it drops the
    /// preview un-maximized - issue #163 through the reopen door.
    ///
    /// The path is `#[cfg(not(test))]` and needs live windows, so - same
    /// discipline as the locks above - it is pinned against the source text.
    #[test]
    fn a_docked_reopen_leaves_the_maximize_bit_for_the_undock() {
        let source = include_str!("mod.rs").replace("\r\n", "\n");
        let runtime_source = &source[..source
            .find("mod tests {")
            .expect("the test module must still terminate the runtime source")];
        let body = without_comments(body_of(runtime_source, "create_preview_window"));

        let gate = body.find("if guard.state.docked").expect(
            "the reopen's restore must branch on the surviving dock PREFERENCE: \
             with the preference set, `open_preview_window` re-tiles this window \
             the moment it is shown, so consuming the maximize bit here spends \
             it on a maximize the tiler immediately undoes - and `commit_dock`'s \
             `!staged.previous.docked` gate cannot recapture it, leaving the \
             undock after it nothing to restore",
        );
        let take = body
            .find("take_preview_restore()")
            .expect("the reopen must still CONSUME the bit when not docked");
        assert!(
            gate < take,
            "the dock-preference check must GATE the consume, not follow it: a \
             `take` that has already run has already cleared the bit"
        );
    }

    /// The in-app Close button and the titlebar X must leave the SAME state.
    ///
    /// `close_preview_window` records the preview's rect on the way out; the X
    /// reaches only `WindowEvent::Destroyed`, which records no geometry at all.
    /// Unguarded, the button wrote the MAXIMIZED rect into `floating` while the
    /// X - whose recorder correctly declines to - preserved the pre-maximize
    /// one, so one gesture produced two outcomes depending on which chrome the
    /// operator clicked. Skipping the maximized read is what converges them,
    /// which is the invariant `undock_layout_after_teardown` documents.
    #[test]
    fn the_in_app_close_declines_to_record_a_maximized_rect() {
        let source = include_str!("mod.rs").replace("\r\n", "\n");
        let runtime_source = &source[..source
            .find("mod tests {")
            .expect("the test module must still terminate the runtime source")];
        let close = without_comments(body_of(runtime_source, "close_preview_window"));

        let maximized = close.find("window.is_maximized()").expect(
            "close_preview_window must SKIP a maximized window: `floating` is \
             the rect the preview is restored TO, so recording a maximized rect \
             here destroys the pre-maximize rect the restore wants - and the \
             next open either refuses the off-work-area origin outright or \
             reopens the window unmaximized at maximized pixels",
        );
        let record = close
            .find("state.floating = Some(geometry)")
            .expect("close_preview_window must still record the floating rect");
        assert!(
            maximized < record,
            "the maximized check must come BEFORE the record, or it guards \
             nothing"
        );
        let guard = close
            .find("runtime(&state).lock()")
            .expect("close_preview_window must take the runtime lock to record");
        assert!(
            maximized < guard,
            "the window getters must run with the runtime mutex RELEASED: this \
             is an async command, so it never short-circuits onto the main \
             thread, and the event-loop thread it posts to takes this very mutex \
             in the preview's own window handlers - a queued Moved event and \
             this read then wait on each other with no timeout"
        );
    }

    // -- issue #161: one monitor-resolution policy, not two ------------------

    /// Both monitor lookups must resolve through ONE helper.
    ///
    /// `enter_docked_mode` falls back to `primary_monitor()` when
    /// `current_monitor()` returns `Ok(None)`; `follow_docked_preview` used to
    /// do the same lookup with no fallback at all. So a dock that only
    /// succeeded BECAUSE of the fallback was never followed again - the pane
    /// stopped tracking in exactly the state the fallback was added to
    /// survive. Same class as the trailing-dot host divergence from #159.
    ///
    /// The whole path is `#[cfg(not(test))]` and needs a live `AppHandle`, so -
    /// same discipline as the locks above - this is pinned against the source
    /// text.
    #[test]
    fn both_monitor_lookups_share_one_fallback_policy() {
        let source = include_str!("mod.rs").replace("\r\n", "\n");
        // Runtime half only. This test names the getters it is checking for,
        // so matching the whole file would let it pass against its own text.
        let runtime_source = &source[..source
            .find("mod tests {")
            .expect("the test module must still terminate the runtime source")];

        for name in ["enter_docked_mode", "follow_docked_preview"] {
            let body = body_of(runtime_source, name);
            assert!(
                body.contains("resolve_dock_monitor(&main)?"),
                "{name} must resolve the monitor through the shared helper: two \
                 open-coded lookups are free to disagree about the fallback, \
                 which is how docking came to survive an absent current monitor \
                 while the follow that keeps the pane tiled beside it did not"
            );
            for getter in ["current_monitor()", "primary_monitor()"] {
                assert!(
                    !body.contains(getter),
                    "{name} must not call {getter} itself - the fallback policy \
                     lives in resolve_dock_monitor and nowhere else, or the two \
                     sites drift apart again"
                );
            }
        }

        let helper = body_of(runtime_source, "resolve_dock_monitor");
        let current = helper
            .find("current_monitor()")
            .expect("resolve_dock_monitor must still read the current monitor");
        let cold_arm = helper
            .find("None => window")
            .expect("the fallback must sit on the None arm of a match");
        let primary = helper
            .find("primary_monitor()")
            .expect("resolve_dock_monitor must still fall back to the primary monitor");
        assert!(
            current < cold_arm && cold_arm < primary,
            "primary_monitor() must stay on the COLD path, reached only through \
             the None arm: `.or(..)` is EAGER, so it would call the fallback on \
             every dock and every follow even with the current monitor already \
             in hand, and its `?` would then propagate a teardown-race error out \
             of a caller that had everything it needed"
        );
        assert!(
            !helper.contains(".or("),
            "resolve_dock_monitor must not reach for the eager `.or(..)` \
             combinator - that is the regression #159 fixed"
        );
        assert!(
            helper.contains("Failed to read the current monitor: {error}")
                && helper.contains("Could not determine which monitor Hive Manager is on"),
            "the two diagnostics must stay DISTINCT: a failed read and an \
             exhausted lookup are different faults, and folding Ok(None) in with \
             Err(e) discards the backend's error"
        );

        // The helper's getters are blocking round-trips to the event-loop
        // thread, so the follow must not reach it from that thread (#159).
        // Anchor on the retile hop specifically: `wire_window_events` holds a
        // SECOND spawn in the preview's Destroyed arm, so a bare `find` would
        // measure against whichever one happens to sort first. And bind the
        // check to CONTAINMENT, not order - "the spawn appears somewhere above
        // the follow" stays true when the follow is hoisted back out of it.
        let wiring = body_of(runtime_source, "wire_window_events");
        let retile = wiring
            .find("let retile_app")
            .expect("the retile follow must still clone the handle it hops with");
        let block = &wiring[retile..];
        let open = block
            .find("tauri::async_runtime::spawn(async move {")
            .expect("the retile follow must hop off the window-event callback");
        let block = &block[open..];
        let close = block
            .find("\n            });")
            .expect("the retile spawn must close at its 12-space `});`");
        assert!(
            block[..close].contains("follow_docked_preview(&retile_app)"),
            "follow_docked_preview must be reached from INSIDE the SPAWNED task, never \
             inline in the window-event callback: it resolves the monitor \
             through getters that post a message to the event-loop thread and \
             block on the reply, so running it ON that thread deadlocks the \
             event loop against itself with no timeout to break it"
        );
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
