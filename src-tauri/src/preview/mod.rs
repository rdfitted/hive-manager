use tauri::Url;

#[cfg(not(test))]
use crate::http::state::AppState;
#[cfg(not(test))]
use std::sync::Arc;
#[cfg(not(test))]
use tauri::{
    webview::NewWindowResponse, AppHandle, Manager, State, WebviewUrl, WebviewWindowBuilder,
};

#[cfg(not(test))]
pub const PREVIEW_WINDOW_LABEL: &str = "operator-preview";
const LOCAL_API_PORT: u16 = 18_800;
const MAIN_DEV_SERVER_PORT: u16 = 1_420;

/// Parse and validate an operator-supplied preview URL before it reaches a webview.
///
/// Preview content is deliberately limited to ordinary web origins. The local Hive
/// API is excluded because loading it as the top-level origin would bypass browser
/// CORS protections for subsequent same-origin requests.
pub(crate) fn validate_preview_url(input: &str, configured_api_port: u16) -> Result<Url, String> {
    let trimmed = input.trim();
    if trimmed.is_empty() {
        return Err("Enter an http:// or https:// preview URL".to_string());
    }

    let url = Url::parse(trimmed).map_err(|_| "Enter a valid absolute preview URL".to_string())?;

    if !matches!(url.scheme(), "http" | "https") {
        return Err("Preview URLs must use http:// or https://".to_string());
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

    host.eq_ignore_ascii_case("tauri.localhost")
        || (host.eq_ignore_ascii_case("localhost")
            && url.port_or_known_default() == Some(MAIN_DEV_SERVER_PORT))
}

fn preview_navigation_allowed(url: &Url, configured_api_port: u16) -> bool {
    validate_preview_url(url.as_str(), configured_api_port).is_ok()
}

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
) -> Result<(), String> {
    let configured_api_port = app_state.config.read().await.api.port;
    let url = validate_preview_url(&url, configured_api_port)?;

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
        return Ok(());
    }

    let mut config = app
        .config()
        .app
        .windows
        .iter()
        .find(|config| config.label == PREVIEW_WINDOW_LABEL)
        .cloned()
        .ok_or_else(|| "Preview window configuration is missing".to_string())?;
    config.url = WebviewUrl::External(url);

    let window = WebviewWindowBuilder::from_config(&app, &config)
        .map_err(|error| format!("Failed to configure preview window: {error}"))?
        .on_navigation(move |url| preview_navigation_allowed(url, configured_api_port))
        .on_new_window(|_, _| NewWindowResponse::Deny)
        .build()
        .map_err(|error| format!("Failed to open preview window: {error}"))?;

    window
        .set_focus()
        .map_err(|error| format!("Failed to focus preview window: {error}"))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn validate(input: &str) -> Result<Url, String> {
        validate_preview_url(input, LOCAL_API_PORT)
    }

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
}
