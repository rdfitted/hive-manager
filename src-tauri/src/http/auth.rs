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
static API_PORT: OnceLock<u16> = OnceLock::new();
/// The configured default; used only before `serve` has bound (tests, or prompt
/// writes that race startup).
const DEFAULT_API_PORT: u16 = 18800;

/// Record the port this launch's API actually bound, so prompt rewriting sends
/// the token only to this instance (isolated instances bind other ports).
pub fn set_api_port(port: u16) {
    let _ = API_PORT.set(port);
}

fn api_port() -> u16 {
    API_PORT.get().copied().unwrap_or(DEFAULT_API_PORT)
}

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
        && port.parse::<u16>().is_ok_and(|port| port == api_port())
        && path.starts_with("/api/")
}

/// The part of a segment that belongs to the curl command itself: everything
/// before the first shell separator (`&&`, `||`, `;`, `|`) that sits outside
/// single or double quotes. Line continuations (`\` + newline) stay inside the
/// command. A separator inside a quoted argument (for example
/// `"http://localhost:18800/api/x;v=1"`) does not end the command, so a later
/// destination in the same curl stays visible to the destination check.
///
/// `in_backticks` marks an invocation that starts right after a backtick (a
/// `` `curl ...` `` command substitution or markdown inline code); the next
/// unquoted backtick closes it, exactly as the shell would.
fn command_portion(segment: &str, in_backticks: bool) -> &str {
    let bytes = segment.as_bytes();
    let mut quote: Option<u8> = None;
    let mut index = 0;
    while index < bytes.len() {
        let byte = bytes[index];
        match quote {
            Some(open) => {
                if byte == open {
                    quote = None;
                } else if open == b'"' && byte == b'\\' {
                    index += 1; // skip the escaped byte inside double quotes
                }
            }
            None => match byte {
                b'\'' | b'"' => quote = Some(byte),
                b'\\' => index += 1,
                b';' | b'|' => return &segment[..index],
                b'`' if in_backticks => return &segment[..index],
                b'&' if bytes.get(index + 1) == Some(&b'&') => return &segment[..index],
                _ => {}
            },
        }
        index += 1;
    }
    // An unterminated quote keeps the whole segment, which only adds URLs to judge.
    segment
}

/// One shell word plus whether it contains a parameter or command expansion
/// (`$` or a backtick), and whether any of that expansion is unquoted. An
/// unquoted expansion is word-split by the shell and can add arguments (and so
/// destinations) that the literal text does not show.
#[derive(Debug, Default, PartialEq)]
struct ShellWord {
    text: String,
    expansion: bool,
    unquoted_expansion: bool,
}

/// Split a curl command's argument text the way a POSIX shell would: whitespace
/// separates words, single quotes are literal, double quotes honour `\`
/// escapes, and `\` + newline is a line continuation. The flag is `true` when
/// the text ends inside an open quote (an incomplete command).
fn shell_words(text: &str) -> (Vec<ShellWord>, bool) {
    let mut words = Vec::new();
    let mut current = ShellWord::default();
    let mut in_word = false;
    let mut chars = text.chars().peekable();
    while let Some(ch) = chars.next() {
        match ch {
            '\'' => {
                in_word = true;
                loop {
                    match chars.next() {
                        Some('\'') => break,
                        Some(other) => current.text.push(other),
                        None => {
                            words.push(current);
                            return (words, true);
                        }
                    }
                }
            }
            '"' => {
                in_word = true;
                loop {
                    match chars.next() {
                        Some('"') => break,
                        Some('\\') => match chars.next() {
                            Some('\n') => {}
                            Some(escaped @ ('"' | '\\' | '$' | '`')) => current.text.push(escaped),
                            Some(other) => {
                                current.text.push('\\');
                                current.text.push(other);
                            }
                            None => {
                                words.push(current);
                                return (words, true);
                            }
                        },
                        Some(other @ ('$' | '`')) => {
                            current.expansion = true;
                            current.text.push(other);
                        }
                        Some(other) => current.text.push(other),
                        None => {
                            words.push(current);
                            return (words, true);
                        }
                    }
                }
            }
            '\\' => match chars.next() {
                Some('\n') | None => {}
                Some(other) => {
                    in_word = true;
                    current.text.push(other);
                }
            },
            ch if ch.is_whitespace() => {
                if in_word {
                    words.push(std::mem::take(&mut current));
                    in_word = false;
                }
            }
            other => {
                // `{a,b}` brace expansion also adds words in bash.
                if matches!(other, '$' | '`' | '{' | '}') {
                    current.expansion = true;
                    current.unquoted_expansion = true;
                }
                in_word = true;
                current.text.push(other);
            }
        }
    }
    if in_word {
        words.push(current);
    }
    (words, false)
}

/// Short options whose value is the next word (or the rest of the cluster).
const CURL_SHORT_WITH_VALUE: &str = "AbcdEeFHmorTuwXY";
/// Long options whose value is the next word (or follows `=`).
const CURL_LONG_WITH_VALUE: &[&str] = &[
    "header", "request", "data", "data-raw", "data-binary", "data-urlencode", "data-ascii",
    "json", "output", "write-out", "user", "user-agent", "referer", "form", "form-string",
    "cookie", "cookie-jar", "upload-file", "max-time", "connect-timeout", "retry",
    "retry-delay", "retry-max-time", "cert", "key", "cacert", "range", "limit-rate",
    "max-filesize", "speed-limit", "speed-time", "expect100-timeout",
];
/// Value-less long options seen in Hive prompts and common use.
const CURL_LONG_FLAGS: &[&str] = &[
    "silent", "show-error", "fail", "fail-with-body", "location", "include", "insecure",
    "compressed", "no-buffer", "globoff", "http1.1", "http2", "get", "head",
    "retry-all-errors", "retry-connrefused", "no-progress-meter",
];
/// Value-less short options. `-v/--verbose` (and `--trace*`) are deliberately
/// absent: they print request headers, which would expose the token.
const CURL_SHORT_FLAGS: &str = "fsSLikINGgZ0123456#";

/// Whether a curl invocation's arguments request only this instance's API.
/// Every positional argument (and every `--url` value) must be a literal local
/// API URL, and there must be at least one. Fails closed on: options that can
/// redirect or add requests (`-x/--proxy`, `-K/--config`, `--next`, ...) or are
/// not listed above; any unquoted `$`/backtick expansion (word splitting can add
/// arguments); any expansion inside a destination; and an incomplete command
/// (unterminated quote).
fn targets_local_api(segment: &str) -> bool {
    let (words, unterminated) = shell_words(segment);
    if unterminated || words.iter().any(|word| word.unquoted_expansion) {
        return false;
    }
    let is_destination = |word: &ShellWord| !word.expansion && is_local_api_url(&word.text);
    let mut destinations = 0;
    let mut words = words.into_iter();
    while let Some(word) = words.next() {
        if let Some(long) = word.text.strip_prefix("--") {
            let (name, attached) = match long.split_once('=') {
                Some((name, value)) => (name.to_string(), Some(value.to_string())),
                None => (long.to_string(), None),
            };
            if name == "url" {
                let ok = match attached {
                    Some(value) => !word.expansion && is_local_api_url(&value),
                    None => words.next().is_some_and(|value| is_destination(&value)),
                };
                if !ok {
                    return false;
                }
                destinations += 1;
            } else if CURL_LONG_WITH_VALUE.contains(&name.as_str()) {
                if attached.is_none() && words.next().is_none() {
                    return false;
                }
            } else if !(CURL_LONG_FLAGS.contains(&name.as_str()) && attached.is_none()) {
                return false;
            }
        } else if let Some(cluster) =
            word.text.strip_prefix('-').filter(|cluster| !cluster.is_empty())
        {
            for (offset, letter) in cluster.char_indices() {
                if CURL_SHORT_WITH_VALUE.contains(letter) {
                    let attached = &cluster[offset + letter.len_utf8()..];
                    if attached.is_empty() && words.next().is_none() {
                        return false;
                    }
                    break;
                }
                if !CURL_SHORT_FLAGS.contains(letter) {
                    return false;
                }
            }
        } else {
            if !is_destination(&word) {
                return false;
            }
            destinations += 1;
        }
    }
    destinations > 0
}

/// Byte offsets just past each `curl`/`curl.exe` executable in a line. A word
/// counts only in *command position*: at the start of the block or right after
/// a shell separator (`|`, `;`, `&`, `(`, `{`, a backtick), never as an
/// argument (`-o curl` is an option value), and never inside quotes. `quote`
/// and `at_command` carry the shell state across the continuation lines of
/// one command block.
fn curl_executable_ends(line: &str, quote: &mut Option<u8>, at_command: &mut bool) -> Vec<usize> {
    let bytes = line.as_bytes();
    let mut ends = Vec::new();
    let mut index = 0;
    while index < bytes.len() {
        let byte = bytes[index];
        if let Some(open) = *quote {
            if byte == open {
                *quote = None;
            } else if open == b'"' && byte == b'\\' {
                index += 1;
            }
            index += 1;
            continue;
        }
        match byte {
            b' ' | b'\t' | b'\r' => {
                index += 1;
                continue;
            }
            b'|' | b';' | b'&' | b'(' | b'{' | b'`' => {
                *at_command = true;
                index += 1;
                continue;
            }
            b'\'' | b'"' => {
                *at_command = false;
                *quote = Some(byte);
                index += 1;
                continue;
            }
            b'\\' => {
                // `\` at the end of a line is a continuation, not a word.
                if index + 1 < bytes.len() {
                    *at_command = false;
                }
                index += 2;
                continue;
            }
            _ => {}
        }
        // Byte comparisons: `index` may sit inside a multi-byte character.
        if *at_command && bytes[index..].starts_with(b"curl") {
            let mut end = index + 4;
            if bytes[end..].starts_with(b".exe") {
                end += 4;
            }
            if bytes.get(end).is_some_and(|byte| *byte == b' ') {
                ends.push(end);
                *at_command = false;
                index = end;
                continue;
            }
        }
        *at_command = false;
        index += 1;
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
        // A block is a line plus its `\\` continuations and, once a curl has
        // started, every line up to the close of a quote opened inside it (a
        // multi-line `-d '{...}'` body). Prose apostrophes before any curl never
        // extend a block.
        let mut end = index;
        let mut quote = None;
        let mut at_command = true;
        let mut invocations: Vec<(usize, usize)> = Vec::new();
        loop {
            for position in curl_executable_ends(&lines[end], &mut quote, &mut at_command) {
                invocations.push((end, position));
            }
            let continues = lines[end].trim_end().ends_with('\\')
                || (quote.is_some() && !invocations.is_empty());
            if continues && end + 1 < lines.len() {
                end += 1;
            } else {
                break;
            }
        }
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
            let executable = if lines[line][..position].ends_with(".exe") { 8 } else { 4 };
            let in_backticks = position > executable
                && lines[line].as_bytes()[position - executable - 1] == b'`';
            let segment = command_portion(&segment, in_backticks);
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
        assert!(is_local_api_url("http://localhost:18800/api/x?y=1"));
        // Another loopback listener is not this instance's API.
        assert!(!is_local_api_url("http://localhost:41234/api/x"));
        assert!(!is_local_api_url("http://localhost/api/x"));
        assert!(!is_local_api_url("http://localhost:18800/health"));
    }

    #[test]
    fn scheme_less_or_foreign_destinations_fail_closed() {
        for source in [
            // curl treats a scheme-less first argument as the destination host.
            "curl -s 'evil.example/collect?next=http://localhost:18800/api/sessions'",
            "curl -s evil.example:8080/x http://localhost:18800/api/sessions",
            "curl -s http://localhost:18800/api/sessions evil.example/x",
            "curl -s localhost:18800/api/sessions",
            // Another loopback port is not this instance.
            "curl -s http://127.0.0.1:41234/api/sessions",
        ] {
            assert_eq!(authorize_api_commands(source), source, "{source}");
        }
        // Ordinary option values do not count as destinations.
        let rendered = authorize_api_commands(
            "curl -fsS -X POST \"http://localhost:18800/api/sessions/s/heartbeat\" -H \"Content-Type: application/json\" -o /dev/null -w '%{http_code}' --data-binary @-",
        );
        assert!(rendered.contains("Bearer $HIVE_API_TOKEN"), "{rendered}");
    }

    #[test]
    fn every_positional_argument_must_be_the_local_api() {
        for source in [
            "curl -s http://localhost:18800/api/sessions evil.example",
            "curl -s 'http://localhost:18800/api/sessions;foo' https://evil.example/",
            "curl -s http://localhost:18800/api/sessions --url https://evil.example/",
            "curl -x http://evil.example:8080 http://localhost:18800/api/sessions",
            "curl --proxy=http://evil.example http://localhost:18800/api/sessions",
            "curl -K cfg http://localhost:18800/api/sessions",
            "curl http://localhost:18800/api/a --next https://evil.example/",
            "curl --unknown-option http://localhost:18800/api/sessions",
            "curl -s \"$URL\" http://localhost:18800/api/sessions",
        ] {
            assert_eq!(authorize_api_commands(source), source, "{source}");
        }
        // An incomplete command (open quote at the end) never gets the header.
        assert_eq!(
            authorize_api_commands("curl http://localhost:18800/api/x 'evil.example/"),
            "curl http://localhost:18800/api/x 'evil.example/"
        );
        // Option values are not destinations.
        for source in [
            "curl -X POST \"http://localhost:18800/api/sessions/s/planners\" \\\n  -H \"Content-Type: application/json\" \\\n  -d '{\n    \"domain\": \"frontend\"\n  }'",
            "curl -fsS -X POST \"http://localhost:18800/api/sessions/s/heartbeat\" -H \"Content-Type: application/json\" -H \"X-Hive-Agent-Token: $HIVE_AGENT_TOKEN\" --data-binary @-",
            "curl -sS -o NUL -w '%{http_code}' -d '{\"a\":\"evil.example/x; y\"}' http://127.0.0.1:18800/api/x",
            "curl -XPOST --url=http://localhost:18800/api/x",
            "1. Read the evaluator prompt: `curl -s \"http://localhost:18800/api/sessions/x/evaluators\"`",
   "   # 1. ui worker
   curl -X POST \"http://localhost:18800/api/sessions/s/qa-workers\" \
     -H \"Content-Type: application/json\" \
     -d '{}'
",
        ] {
            assert!(authorize_api_commands(source).contains("Bearer $HIVE_API_TOKEN"), "{source}");
        }
    }

    #[test]
    fn curl_inside_a_quoted_argument_is_not_an_invocation() {
        for source in [
            "curl http://localhost:18800/api/sessions example.com",
            "curl http://localhost:18800/api/sessions -H \"X: curl value\" https://example.com/collect",
            "curl http://localhost:18800/api/sessions -H 'X: curl value' https://example.com/collect",
            "curl http://localhost:18800/api/sessions \\\n  -H \"X: curl \\\n value\" https://example.com/collect",
        ] {
            assert_eq!(authorize_api_commands(source), source, "{source}");
        }
        // Multi-byte text must not panic the scanner; prose is not a command.
        let prose = "Spawned — curl -s http://localhost:18800/api/x";
        assert_eq!(authorize_api_commands(prose), prose);
        let rendered = authorize_api_commands("— marker
curl -s http://localhost:18800/api/x");
        assert!(rendered.contains("Bearer $HIVE_API_TOKEN"), "{rendered}");
        let rendered = authorize_api_commands(
            "curl -H \"X: curl value\" http://localhost:18800/api/sessions",
        );
        assert_eq!(rendered.matches("Bearer $HIVE_API_TOKEN\"").count(), 1, "{rendered}");
    }

    #[test]
    fn expansions_that_can_add_destinations_fail_closed() {
        for source in [
            "TAIL='x https://example.com/collect'\ncurl http://localhost:18800/api/$TAIL",
            "curl http://localhost:18800/api/x $EXTRA",
            "curl \"$URL\"",
            "curl http://localhost:18800/api/x -d $BODY",
            "curl http://localhost:18800/api/`echo x`",
            "curl --url \"http://localhost:18800/api/$X\"",
        ] {
            assert_eq!(authorize_api_commands(source), source, "{source}");
        }
        // A double-quoted expansion inside an option value is not split.
        let rendered = authorize_api_commands(
            "curl -H \"X-Hive-Agent-Token: $HIVE_AGENT_TOKEN\" http://localhost:18800/api/x",
        );
        assert!(rendered.contains("Bearer $HIVE_API_TOKEN"), "{rendered}");
    }

    #[test]
    fn multiline_quoted_bodies_are_validated_through_the_closing_quote() {
        let leak = "curl http://localhost:18800/api/x -d '{\n}' https://example.com/collect";
        assert_eq!(authorize_api_commands(leak), leak);
        let local = "curl http://localhost:18800/api/x -d '{\n  \"a\": 1\n}'\nnext line";
        let rendered = authorize_api_commands(local);
        assert!(rendered.starts_with("curl -H \"Authorization: Bearer $HIVE_API_TOKEN\" http://"), "{rendered}");
        // Prose apostrophes before any curl do not swallow later lines.
        let prose = "Don't forget:\ncurl -s http://localhost:18800/api/x";
        assert!(authorize_api_commands(prose).contains("curl -H \"Authorization"));
    }

    #[test]
    fn only_a_curl_in_command_position_starts_an_invocation() {
        for source in [
            "curl -s http://localhost:18800/api/sessions -o curl https://evil.example/collect",
            "curl -s http://localhost:18800/api/sessions -o curl \\\n  https://evil.example/collect",
            "EXTRA_URLS=' https://example.com/collect'\ncurl http://localhost:18800/api/sessions${EXTRA_URLS}",
        ] {
            assert_eq!(authorize_api_commands(source), source, "{source}");
        }
        // Command positions after separators still count.
        let header = "-H \"Authorization: Bearer $HIVE_API_TOKEN\"";
        for (source, expected) in [
            ("x=$(curl -s http://localhost:18800/api/a)", 1),
            ("cat <<'JSON' | curl -s http://localhost:18800/api/a --data-binary @-", 1),
            ("true && curl -s http://localhost:18800/api/a; curl -s http://localhost:18800/api/b", 2),
            ("   curl -s http://localhost:18800/api/a", 1),
        ] {
            assert_eq!(authorize_api_commands(source).matches(header).count(), expected, "{source}");
        }
    }

    #[test]
    fn brace_expansion_and_verbose_output_fail_closed() {
        for source in [
            "curl http://localhost:18800/api/x -d {body,https://example.com/collect}",
            "curl http://localhost:18800/api/{a,https://example.com/b}",
            "curl -v http://localhost:18800/api/x",
            "curl -sv http://localhost:18800/api/x",
            "curl --verbose http://localhost:18800/api/x",
            "curl --trace-ascii - http://localhost:18800/api/x",
        ] {
            assert_eq!(authorize_api_commands(source), source, "{source}");
        }
        // Quoted braces in a JSON body are data.
        let rendered = authorize_api_commands("curl -d '{\"a\":1}' http://localhost:18800/api/x");
        assert!(rendered.contains("Bearer $HIVE_API_TOKEN"), "{rendered}");
    }

    #[test]
    fn shell_words_follow_posix_quoting() {
        assert_eq!(
            shell_words("-H \"a b\" 'c d' e\\ f \\\n g")
                .0
                .into_iter()
                .map(|word| word.text)
                .collect::<Vec<_>>(),
            vec!["-H", "a b", "c d", "e f", "g"]
        );
        assert!(shell_words("'unterminated").1);
    }

    #[test]
    fn quoted_separators_do_not_hide_a_second_destination() {
        for source in [
            "curl \"http://localhost:18800/api/sessions;v=1\" https://example.com/collect",
            "curl 'http://localhost:18800/api/sessions|x' https://example.com/collect",
            "curl \"http://localhost:18800/api/a?b=1&&c=2\" evil.example/x",
            "curl -H \"X: a;b\" http://localhost:18800/api/sessions https://example.com/collect",
        ] {
            assert_eq!(authorize_api_commands(source), source, "{source}");
        }
        // A quoted separator inside a local-only invocation still authorizes it.
        let rendered = authorize_api_commands("curl -s \"http://localhost:18800/api/sessions;v=1\" && echo done");
        assert!(rendered.contains("Bearer $HIVE_API_TOKEN"), "{rendered}");
        assert_eq!(command_portion("curl \"a;b\" c; d", false), "curl \"a;b\" c");
        assert_eq!(command_portion("curl 'a|b' && d", false), "curl 'a|b' ");
        assert_eq!(command_portion(" -s x` rest", true), " -s x");
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
