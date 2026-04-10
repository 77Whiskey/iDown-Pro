use std::sync::Arc;
use std::time::Duration;

use reqwest::header::{self, HeaderMap, HeaderValue};
use reqwest::{cookie::Jar, Client};

/// Shared HTTP client handle.  Cheaply cloneable (wraps `Arc<Client>` under
/// the hood from reqwest, plus our shared cookie jar).
pub type HttpClient = Arc<Client>;

/// Shared cookie jar that can be inspected/modified outside the client.
pub type SharedCookieJar = Arc<Jar>;

/// Realistic Chrome 120 on Windows 11 user-agent string.
const DEFAULT_USER_AGENT: &str = "Mozilla/5.0 (Windows NT 10.0; Win64; x64) \
    AppleWebKit/537.36 (KHTML, like Gecko) Chrome/120.0.0.0 Safari/537.36";

/// Instagram web application ID used in the `X-IG-App-ID` header.
const IG_APP_ID: &str = "936619743392459";

/// Build the application-wide HTTP client with cookie support, timeouts, and
/// Instagram-specific default headers.
///
/// Returns both the client and the cookie jar so callers that need to
/// inject or read cookies (e.g. the auth module) can do so.
pub fn build_http_client() -> (HttpClient, SharedCookieJar) {
    let jar = Arc::new(Jar::default());

    let mut default_headers = HeaderMap::new();
    default_headers.insert(
        header::HeaderName::from_static("x-ig-app-id"),
        HeaderValue::from_static(IG_APP_ID),
    );

    let client = Client::builder()
        .cookie_provider(Arc::clone(&jar))
        .user_agent(DEFAULT_USER_AGENT)
        .default_headers(default_headers)
        .pool_idle_timeout(Duration::from_secs(90))
        .pool_max_idle_per_host(10)
        .connect_timeout(Duration::from_secs(10))
        .timeout(Duration::from_secs(30))
        .build()
        .expect("failed to build HTTP client");

    (Arc::new(client), jar)
}

/// Build request headers required for Instagram's private web API endpoints.
///
/// These headers are added *per-request* on top of the client defaults.
pub fn instagram_api_headers(csrf_token: &str) -> HeaderMap {
    let mut headers = HeaderMap::new();

    headers.insert(
        header::HeaderName::from_static("x-csrftoken"),
        HeaderValue::from_str(csrf_token).unwrap_or_else(|_| HeaderValue::from_static("")),
    );
    headers.insert(
        header::HeaderName::from_static("x-requested-with"),
        HeaderValue::from_static("XMLHttpRequest"),
    );
    headers.insert(header::REFERER, HeaderValue::from_static("https://www.instagram.com/"));

    headers
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn build_client_succeeds() {
        let (client, _jar) = build_http_client();
        // Ensure the client is cloneable through Arc.
        let _clone = Arc::clone(&client);
    }

    #[test]
    fn api_headers_contain_csrf() {
        let headers = instagram_api_headers("test_token_123");
        assert_eq!(
            headers.get("x-csrftoken").unwrap().to_str().unwrap(),
            "test_token_123"
        );
        assert_eq!(
            headers.get("x-requested-with").unwrap().to_str().unwrap(),
            "XMLHttpRequest"
        );
        assert_eq!(
            headers.get(header::REFERER).unwrap().to_str().unwrap(),
            "https://www.instagram.com/"
        );
    }
}
