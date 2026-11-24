//! Per-launch bearer token for the local HTTP API.
//!
//! The API binds `127.0.0.1`, but loopback is not an authentication boundary:
//! any local process (and any browser page through no-CORS `GET`s, which send
//! no `Origin`) could read conversations, agent events, journals and sessions.
//! Every `/api/*` request must now carry `Authorization: Bearer <token>`.
//!
//! Contract, shared with the Claude-Code-Setup callers:
//! - each launch generates a fresh random token;
//! - once the listener is bound, the token is written to
//!   `%APPDATA%\hive-manager\api-token` (the storage base dir; an isolated
//!   instance uses its own data dir): UTF-8, one line, no trailing newline,
//!   DACL restricted to the file owner on Windows, mode 0600 elsewhere;
//! - spawned agents receive it as `HIVE_API_TOKEN` and their prompts send it;
//! - the in-app frontend fetches it over the main-window-only Tauri command
//!   `get_api_token`. `EventSource` cannot set headers, so `GET .../stream`
//!   also accepts the token as the `hive_api_token` query parameter.
//!
//! `/health` and CORS preflights stay open; the browser-`Origin` gate in
//! `routes.rs` still runs first and is unchanged.

use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

use axum::{
    body::Body,
    http::{header::AUTHORIZATION, HeaderMap, HeaderValue, Method, Request, StatusCode},
    middleware::Next,
    response::{IntoResponse, Response},
    Json,
};
use sha2::{Digest, Sha256};

pub const API_TOKEN_FILE: &str = "api-token";
pub const API_TOKEN_ENV: &str = "HIVE_API_TOKEN";
pub const STREAM_TOKEN_QUERY: &str = "hive_api_token";

static API_TOKEN: OnceLock<String> = OnceLock::new();

/// This launch's API token. Generated on first use and stable for the process.
pub fn api_token() -> &'static str {
    API_TOKEN.get_or_init(|| {
        format!(
            "{}{}",
            uuid::Uuid::new_v4().simple(),
            uuid::Uuid::new_v4().simple()
        )
    })
}

/// Path of the token file under a storage base dir.
pub fn api_token_path(base_dir: &Path) -> PathBuf {
    base_dir.join(API_TOKEN_FILE)
}

/// Atomically (re)write the token file: create a temp file in the same
/// directory, restrict it to its owner *before* any token bytes are written,
/// then rename it over the previous launch's file. Fails closed: if the file
/// cannot be restricted, the token is never written.
pub fn persist_api_token(base_dir: &Path, token: &str) -> std::io::Result<PathBuf> {
    std::fs::create_dir_all(base_dir)?;
    let path = api_token_path(base_dir);
    let mut temp = tempfile::NamedTempFile::new_in(base_dir)?;
    restrict_to_owner(temp.path())?;
    temp.write_all(token.as_bytes())?;
    temp.flush()?;
    temp.persist(&path).map_err(|error| error.error)?;
    Ok(path)
}

#[cfg(windows)]
fn restrict_to_owner(path: &Path) -> std::io::Result<()> {
    use std::os::windows::ffi::OsStrExt;
    use windows_sys::Win32::Foundation::{LocalFree, HLOCAL};
    use windows_sys::Win32::Security::Authorization::{
        ConvertStringSecurityDescriptorToSecurityDescriptorW, SDDL_REVISION_1,
    };
    use windows_sys::Win32::Security::{
        SetFileSecurityW, DACL_SECURITY_INFORMATION, PROTECTED_DACL_SECURITY_INFORMATION,
        PSECURITY_DESCRIPTOR,
    };

    // Protected DACL, one ACE: full access for the file's owner (OW). No
    // inherited ACEs, so Administrators/SYSTEM/other users get nothing.
    let sddl: Vec<u16> = "D:P(A;;FA;;;OW)\0".encode_utf16().collect();
    let wide_path: Vec<u16> = path
        .as_os_str()
        .encode_wide()
        .chain(std::iter::once(0))
        .collect();
    let mut descriptor: PSECURITY_DESCRIPTOR = std::ptr::null_mut();
    // SAFETY: both strings are NUL-terminated UTF-16 buffers that outlive the
    // calls; the descriptor is allocated by the OS and released with LocalFree.
    unsafe {
        if ConvertStringSecurityDescriptorToSecurityDescriptorW(
            sddl.as_ptr(),
            SDDL_REVISION_1,
            &mut descriptor,
            std::ptr::null_mut(),
        ) == 0
        {
            return Err(std::io::Error::last_os_error());
        }
        let applied = SetFileSecurityW(
            wide_path.as_ptr(),
            DACL_SECURITY_INFORMATION | PROTECTED_DACL_SECURITY_INFORMATION,
            descriptor,
        );
        let error = (applied == 0).then(std::io::Error::last_os_error);
        LocalFree(descriptor as HLOCAL);
        match error {
            Some(error) => Err(error),
            None => Ok(()),
        }
    }
}

#[cfg(unix)]
fn restrict_to_owner(path: &Path) -> std::io::Result<()> {
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600))
}

#[cfg(not(any(windows, unix)))]
fn restrict_to_owner(_path: &Path) -> std::io::Result<()> {
    Ok(())
}

fn digest(value: &str) -> [u8; 32] {
    Sha256::digest(value.as_bytes()).into()
}

/// Constant-time comparison over fixed-length digests.
fn token_matches(candidate: &str, expected: &str) -> bool {
    let left = digest(candidate);
    let right = digest(expected);
    left.iter()
        .zip(right.iter())
        .fold(0u8, |difference, (a, b)| difference | (a ^ b))
        == 0
}

fn bearer_from_headers(headers: &HeaderMap) -> Option<&str> {
    let mut values = headers.get_all(AUTHORIZATION).iter();
    let value = values.next()?;
    if values.next().is_some() {
        return None;
    }
    let value = value.to_str().ok()?.trim();
    let (scheme, token) = value.split_once(' ')?;
    scheme
        .eq_ignore_ascii_case("bearer")
        .then(|| token.trim())
        .filter(|token| !token.is_empty())
}

fn stream_query_token(request: &Request<Body>) -> Option<String> {
    if request.method() != Method::GET || !request.uri().path().ends_with("/stream") {
        return None;
    }
    // Tokens are hex, so no percent-decoding is needed; an encoded value fails closed.
    request
        .uri()
        .query()?
        .split('&')
        .filter_map(|pair| pair.split_once('='))
        .find(|(key, _)| *key == STREAM_TOKEN_QUERY)
        .map(|(_, value)| value.to_string())
}

/// Whether a request carries this launch's token.
pub fn request_is_authorized(request: &Request<Body>, expected: &str) -> bool {
    if let Some(token) = bearer_from_headers(request.headers()) {
        return token_matches(token, expected);
    }
    stream_query_token(request).is_some_and(|token| token_matches(&token, expected))
}

fn is_exempt(request: &Request<Body>) -> bool {
    request.method() == Method::OPTIONS || request.uri().path() == "/health"
}

pub async fn require_api_token(request: Request<Body>, next: Next) -> Response {
    if is_exempt(&request) || request_is_authorized(&request, api_token()) {
        return next.run(request).await;
    }
    let mut response = (
        StatusCode::UNAUTHORIZED,
        Json(serde_json::json!({
            "error": "Missing or invalid API token. Send `Authorization: Bearer <token>`; \
                      agents use $HIVE_API_TOKEN, other local callers read \
                      %APPDATA%\\hive-manager\\api-token (rewritten on every app launch).",
            "status": 401,
        })),
    )
        .into_response();
    response
        .headers_mut()
        .insert("WWW-Authenticate", HeaderValue::from_static("Bearer"));
    response
}

const PROMPT_AUTH_HEADER: &str = "-H \"Authorization: Bearer $HIVE_API_TOKEN\"";

const PROMPT_AUTH_GUIDANCE: &str = "\nEvery Hive API request must send `Authorization: Bearer $HIVE_API_TOKEN` from your process environment (PowerShell: `$env:HIVE_API_TOKEN`); a request without it gets 401. Add the header to any API call you compose yourself. Never print the token, use verbose curl, or write its value to a file.\n";

/// Every `http://` / `https://` URL in a curl invocation's segment, each cut at
/// the first character that cannot be part of an unquoted shell URL.
fn segment_urls(segment: &str) -> Vec<&str> {
    let mut urls = Vec::new();
    let mut search = 0;
    while let Some(found) = segment[search..].find("http") {
        let start = search + found;
        let rest = &segment[start..];
        let after_scheme = if rest.starts_with("http://") {
            7
        } else if rest.starts_with("https://") {
            8
        } else {
            search = start + 4;
            continue;
        };
        let length = rest[after_scheme..]
            .find(|ch: char| ch.is_whitespace() || matches!(ch, '"' | '\'' | '`' | ')' | '|' | ';' | '&' | '<' | '>'))
            .map_or(rest.len(), |end| after_scheme + end);
        urls.push(&rest[..length]);
        search = start + length.max(after_scheme);
    }
    urls
}

/// A URL is the local Hive API only when its parsed authority is a loopback host
/// with an explicit numeric port and its path starts with `/api/`. Userinfo
/// (`localhost:1@evil.com`), look-alike hosts and loopback text inside an
/// external URL's path or query never qualify.
fn is_local_api_url(url: &str) -> bool {
    let Some(rest) = url.strip_prefix("http://").or_else(|| url.strip_prefix("https://")) else {
        return false;
    };
    let authority_end = rest.find(['/', '?', '#']).unwrap_or(rest.len());
    let (authority, path) = rest.split_at(authority_end);
    if authority.contains('@') {
        return false;
    }
    let Some((host, port)) = authority.rsplit_once(':') else {
        return false;
    };
    matches!(host, "localhost" | "127.0.0.1")
        && !port.is_empty()
        && port.bytes().all(|byte| byte.is_ascii_digit())
        && path.starts_with("/api/")
}

/// The part of a segment that belongs to the curl command itself: everything
/// before the first shell separator (`&&`, `||`, `;`, `|`). Line continuations
/// (`\` + newline) stay inside the command. A separator inside a quoted URL
/// would cut early, which only ever drops the header (fails closed).
fn command_portion(segment: &str) -> &str {
    let cut = ["&&", "||", ";", "|"]
        .iter()
        .filter_map(|separator| segment.find(separator))
        .min()
        .unwrap_or(segment.len());
    &segment[..cut]
}

/// An invocation gets the token only if it requests at least one URL and every
/// URL it requests is the local API; any external URL in the same invocation
/// fails closed (no header, so the local call gets an explicit 401).
fn targets_local_api(segment: &str) -> bool {
    let urls = segment_urls(segment);
    !urls.is_empty() && urls.iter().all(|url| is_local_api_url(url))
}

/// Byte offsets just past each `curl`/`curl.exe` executable token in a line.
fn curl_executable_ends(line: &str) -> Vec<usize> {
    let bytes = line.as_bytes();
    let mut ends = Vec::new();
    let mut search = 0;
    while let Some(found) = line[search..].find("curl") {
        let start = search + found;
        let mut end = start + 4;
        if line[end..].starts_with(".exe") {
            end += 4;
        }
        let boundary_before = start == 0
            || matches!(bytes[start - 1], b' ' | b'\t' | b'|' | b'(' | b'`' | b';' | b'&');
        let boundary_after = bytes.get(end).is_some_and(|byte| *byte == b' ');
        if boundary_before && boundary_after {
            ends.push(end);
        }
        search = end;
    }
    ends
}

/// Add the bearer header to every curl invocation in agent-facing text that
/// targets the local Hive API. A command block is a line plus its `\`
/// continuations; within it each invocation is judged by its own segment (from
/// its executable up to the next curl), so `curl https://example.com && curl
/// http://localhost:18800/api/...` authorizes only the second. Idempotent: an
/// invocation whose segment already names `Authorization: Bearer` is left
/// alone. Appends a one-time guidance note when anything was rewritten.
pub fn authorize_api_commands(content: &str) -> String {
    let mut lines: Vec<String> = content.split('\n').map(str::to_string).collect();
    let mut injected = false;
    let mut index = 0;
    while index < lines.len() {
        let mut end = index;
        while end + 1 < lines.len() && lines[end].trim_end().ends_with('\\') {
            end += 1;
        }
        let invocations: Vec<(usize, usize)> = (index..=end)
            .flat_map(|line| {
                curl_executable_ends(&lines[line])
                    .into_iter()
                    .map(move |position| (line, position))
            })
            .collect();
        let mut inserts = Vec::new();
        for (k, &(line, position)) in invocations.iter().enumerate() {
            let (stop_line, stop_position) = invocations
                .get(k + 1)
                .copied()
                .unwrap_or((end, lines[end].len()));
            let mut segment = String::new();
            for current in line..=stop_line {
                let text = &lines[current];
                let from = if current == line { position } else { 0 };
                let to = if current == stop_line { stop_position } else { text.len() };
                if current != line {
                    segment.push('\n');
                }
                segment.push_str(&text[from..to.max(from)]);
            }
            let segment = command_portion(&segment);
            if targets_local_api(segment) && !segment.contains("Authorization: Bearer") {
                inserts.push((line, position));
            }
        }
        for (line, position) in inserts.into_iter().rev() {
            lines[line].insert_str(position, &format!(" {PROMPT_AUTH_HEADER}"));
            injected = true;
        }
        index = end + 1;
    }
    let mut output = lines.join("\n");
    if injected && !output.contains(PROMPT_AUTH_GUIDANCE.trim()) {
        if !output.ends_with('\n') {
            output.push('\n');
        }
        output.push_str(PROMPT_AUTH_GUIDANCE);
    }
    output
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn token_is_stable_and_long() {
        assert_eq!(api_token(), api_token());
        assert_eq!(api_token().len(), 64);
    }

    #[test]
    fn persisted_token_is_a_single_line_file() {
        let dir = tempfile::TempDir::new().unwrap();
        let path = persist_api_token(dir.path(), "first-token").unwrap();
        assert_eq!(path, dir.path().join(API_TOKEN_FILE));
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "first-token");
        // A relaunch replaces the previous token in place.
        persist_api_token(dir.path(), "second-token").unwrap();
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "second-token");
    }

    #[cfg(unix)]
    #[test]
    fn persisted_token_is_owner_only() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::TempDir::new().unwrap();
        let path = persist_api_token(dir.path(), "token").unwrap();
        let mode = std::fs::metadata(path).unwrap().permissions().mode();
        assert_eq!(mode & 0o777, 0o600);
    }

    fn request(uri: &str, authorization: Option<&str>) -> Request<Body> {
        let mut builder = Request::builder().method("GET").uri(uri);
        if let Some(value) = authorization {
            builder = builder.header(AUTHORIZATION, value);
        }
        builder.body(Body::empty()).unwrap()
    }

    #[test]
    fn bearer_parsing_and_matching() {
        let expected = "a".repeat(64);
        assert!(request_is_authorized(
            &request("/api/sessions", Some(&format!("Bearer {expected}"))),
            &expected
        ));
        assert!(request_is_authorized(
            &request("/api/sessions", Some(&format!("bearer  {expected} "))),
            &expected
        ));
        assert!(!request_is_authorized(&request("/api/sessions", None), &expected));
        assert!(!request_is_authorized(
            &request("/api/sessions", Some("Bearer wrong")),
            &expected
        ));
        assert!(!request_is_authorized(
            &request("/api/sessions", Some(&format!("Basic {expected}"))),
            &expected
        ));
        // The query parameter is honoured only on stream routes.
        assert!(request_is_authorized(
            &request(&format!("/api/sessions/x/stream?{STREAM_TOKEN_QUERY}={expected}"), None),
            &expected
        ));
        assert!(!request_is_authorized(
            &request(&format!("/api/sessions/x?{STREAM_TOKEN_QUERY}={expected}"), None),
            &expected
        ));
    }

    #[test]
    fn duplicate_authorization_headers_are_rejected() {
        let expected = "b".repeat(64);
        let mut request = request("/api/sessions", Some(&format!("Bearer {expected}")));
        request
            .headers_mut()
            .append(AUTHORIZATION, HeaderValue::from_str(&format!("Bearer {expected}")).unwrap());
        assert!(!request_is_authorized(&request, &expected));
    }

    #[test]
    fn prompt_curls_to_the_local_api_gain_the_header() {
        let source = "Read it:\n\
curl -s \"http://localhost:18800/api/sessions/s/conversations/shared\"\n\
cat <<'JSON' | curl.exe -fsS -X POST \"http://localhost:18800/api/sessions/s/heartbeat\" \\\n  -H \"Content-Type: application/json\" \\\n  --data-binary @-\n\
curl -s https://example.com/api/thing\n\
Use `curl` carefully.";
        let rendered = authorize_api_commands(source);
        assert!(rendered.contains(
            "curl -H \"Authorization: Bearer $HIVE_API_TOKEN\" -s \"http://localhost:18800/api/sessions/s/conversations/shared\""
        ));
        assert!(rendered.contains(
            "| curl.exe -H \"Authorization: Bearer $HIVE_API_TOKEN\" -fsS -X POST"
        ));
        assert!(rendered.contains("curl -s https://example.com/api/thing"));
        assert!(rendered.contains("Use `curl` carefully."));
        assert!(rendered.contains("$env:HIVE_API_TOKEN"));
        // Idempotent.
        assert_eq!(authorize_api_commands(&rendered), rendered);
    }

    #[test]
    fn external_curl_beside_a_local_one_never_gets_the_token() {
        let source = "curl -s https://example.com/config && curl -s http://localhost:18800/api/sessions\n\
curl -s http://localhost:18800/api/sessions && curl -s https://example.com/x\n\
curl -s https://example.com/a \\\n  -o out && curl -s \\\n  http://127.0.0.1:18800/api/x";
        let rendered = authorize_api_commands(source);
        let header = "-H \"Authorization: Bearer $HIVE_API_TOKEN\"";
        assert!(rendered.contains(&format!(
            "curl -s https://example.com/config && curl {header} -s http://localhost:18800/api/sessions"
        )));
        assert!(rendered.contains(&format!(
            "curl {header} -s http://localhost:18800/api/sessions && curl -s https://example.com/x"
        )));
        assert!(rendered.contains("curl -s https://example.com/a \\"));
        assert!(rendered.contains(&format!("-o out && curl {header} -s \\")));
        assert_eq!(rendered.matches(header).count(), 3);
        assert_eq!(authorize_api_commands(&rendered), rendered);
    }

    #[cfg(windows)]
    #[test]
    fn persisted_token_has_a_protected_owner_only_dacl() {
        use std::os::windows::process::CommandExt;
        let dir = tempfile::TempDir::new().unwrap();
        let path = persist_api_token(dir.path(), "token").unwrap();
        // Only the owner-rights ACE remains and nothing is inherited ("(I)").
        let output = std::process::Command::new("icacls")
            .arg(&path)
            .creation_flags(0x0800_0000) // CREATE_NO_WINDOW
            .stdin(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .output()
            .unwrap();
        let listing = String::from_utf8_lossy(&output.stdout);
        assert!(listing.contains("OWNER RIGHTS"), "{listing}");
        assert!(!listing.contains("(I)"), "{listing}");
    }

    #[test]
    fn loopback_text_inside_an_external_url_never_gets_the_token() {
        for source in [
            "curl -s \"https://evil.com/?u=127.0.0.1:18800/api/\"",
            "curl -s https://evil.com/127.0.0.1:18800/api/sessions",
            "curl -s \"https://example.com/collect?next=http://localhost:18800/api/sessions\"",
            "curl -s http://localhost:18800@evil.com/api/sessions",
            "curl -s http://localhost.evil.com:18800/api/sessions",
            "curl -s http://localhost:18800/api/a https://evil.com/b",
        ] {
            assert_eq!(authorize_api_commands(source), source, "{source}");
        }
        assert!(is_local_api_url("http://127.0.0.1:18800/api/sessions"));
        assert!(is_local_api_url("http://localhost:41234/api/x?y=1"));
        assert!(!is_local_api_url("http://localhost/api/x"));
        assert!(!is_local_api_url("http://localhost:18800/health"));
    }

    #[test]
    fn shell_separators_end_an_invocation() {
        let header = "-H \"Authorization: Bearer $HIVE_API_TOKEN\"";
        // A local URL in a later command never authorizes an external curl.
        for source in [
            "curl https://example.com/config && echo http://localhost:18800/api/sessions",
            "curl https://example.com/config; echo http://localhost:18800/api/sessions",
            "curl https://example.com/config | tee http://localhost:18800/api/x",
            "curl https://example.com/config || echo http://127.0.0.1:18800/api/x",
        ] {
            assert_eq!(authorize_api_commands(source), source, "{source}");
        }
        // A local curl keeps its header when a later command names another URL.
        let rendered = authorize_api_commands(
            "curl -s http://localhost:18800/api/sessions | jq . && echo https://example.com",
        );
        assert!(rendered.starts_with(&format!("curl {header} -s http://localhost:18800/api/sessions | jq .")));
        assert_eq!(rendered.matches(header).count(), 1);
    }

    #[test]
    fn prompt_without_api_curls_is_unchanged() {
        let source = "No API here.\ncurl -s https://example.com\n";
        assert_eq!(authorize_api_commands(source), source);
    }
}
