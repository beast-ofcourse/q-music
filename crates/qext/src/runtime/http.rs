//! The `http` object extensions call: a blocking client with a per-extension
//! cookie jar and host-side permission checks.
//!
//! Why manual redirects: the manifest allowlist must hold for **every hop**, not
//! just the first request. The client is therefore configured with redirects
//! disabled and the loop lives here, so each `Location` is re-validated before a
//! connection is opened. A redirect to an undeclared host fails with that host
//! named.
//!
//! Why headers are rewritten: two request headers can weaken the sandbox, so
//! extension-supplied values are dropped rather than trusted.
//! - `Host` â€” would let a package address a target the allowlist never approved.
//! - `Content-Length` / `Transfer-Encoding` / `Connection` â€” framing is ours.
//!
//! Response shape is fixed by what shipped packages read: `statusCode`, `status`,
//! `ok`, `body` (text) and `headers` (name to array of values). Header names are
//! lowercased because HTTP names are case-insensitive and every fixture package
//! already normalises them.

use crate::runtime::permissions::{NetworkAllowlist, PermissionError};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::time::Duration;
use thiserror::Error;
use ureq::http::{HeaderValue, Method, Request};
use url::Url;

/// One completed redirect chain, raw: (status, status text, headers, body,
/// final url). Shared by `send` and the assembly site `finish`.
type RawHop = (u16, String, BTreeMap<String, Vec<String>>, Vec<u8>, String);

/// Manifest default when `capabilities.networkTimeoutSeconds` is absent.
pub const DEFAULT_TIMEOUT_SECONDS: u64 = 15;
/// Redirect hops a single logical request may take.
pub const MAX_REDIRECTS: u32 = 10;
/// Largest response body accepted, in bytes. Well above any JSON metadata
/// payload and far below anything that would threaten memory.
pub const MAX_BODY_BYTES: u64 = 16 * 1024 * 1024;

/// Request headers a package may not set, because the host owns them.
const RESERVED_REQUEST_HEADERS: [&str; 5] = [
    "host",
    "content-length",
    "transfer-encoding",
    "connection",
    "proxy-connection",
];

/// Headers dropped when a redirect crosses to another host.
const CREDENTIAL_HEADERS: [&str; 3] = ["authorization", "cookie", "proxy-authorization"];

#[derive(Debug, Error, PartialEq, Eq)]
pub enum HttpError {
    #[error("network access denied: {host} is not declared in permissions.network")]
    Denied { host: String },

    #[error("network access denied: {url:?} is not a usable http or https URL")]
    InvalidUrl { url: String },

    #[error("{method:?} is not a valid HTTP method")]
    InvalidMethod { method: String },

    #[error("request timed out after {seconds} seconds")]
    Timeout { seconds: u64 },

    #[error("request failed: {0}")]
    Transport(String),

    #[error("refused after {limit} redirects")]
    TooManyRedirects { limit: u32 },

    #[error("response body exceeded the {limit} byte limit")]
    BodyTooLarge { limit: u64 },
}

impl From<PermissionError> for HttpError {
    fn from(error: PermissionError) -> Self {
        match error {
            PermissionError::NetworkDenied { host } => HttpError::Denied { host },
            PermissionError::InvalidUrl { url } => HttpError::InvalidUrl { url },
            other => HttpError::InvalidUrl {
                url: other.to_string(),
            },
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct HttpLimits {
    pub timeout: Duration,
    pub max_redirects: u32,
    pub max_body_bytes: u64,
}

impl Default for HttpLimits {
    fn default() -> Self {
        Self {
            timeout: Duration::from_secs(DEFAULT_TIMEOUT_SECONDS),
            max_redirects: MAX_REDIRECTS,
            max_body_bytes: MAX_BODY_BYTES,
        }
    }
}

impl HttpLimits {
    pub fn with_timeout_seconds(seconds: u64) -> Self {
        Self {
            timeout: Duration::from_secs(seconds.max(1)),
            ..Self::default()
        }
    }
}

/// One response, serialised straight into JavaScript.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct HttpResponse {
    pub status_code: u16,
    pub status: String,
    pub ok: bool,
    /// The URL that produced this response, after any redirects.
    pub url: String,
    pub body: String,
    pub headers: BTreeMap<String, Vec<String>>,
}

impl HttpResponse {
    pub fn header(&self, name: &str) -> Option<&str> {
        self.headers
            .get(&name.to_ascii_lowercase())
            .and_then(|values| values.first())
            .map(String::as_str)
    }

    pub fn header_values(&self, name: &str) -> &[String] {
        self.headers
            .get(&name.to_ascii_lowercase())
            .map(Vec::as_slice)
            .unwrap_or(&[])
    }
}

/// The host `http` object for one extension.
///
/// Holds its own [`ureq::Agent`], which owns its own cookie jar, so two
/// extensions never see each other's sessions.
pub struct HostHttp {
    agent: ureq::Agent,
    allowlist: NetworkAllowlist,
    limits: HttpLimits,
}

impl std::fmt::Debug for HostHttp {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("HostHttp")
            .field("allowlist", &self.allowlist)
            .field("limits", &self.limits)
            .finish()
    }
}

impl HostHttp {
    pub fn new(allowlist: NetworkAllowlist, limits: HttpLimits) -> Self {
        // `max_redirects(0)` disables the client's own redirect following so
        // every hop passes through `request`'s loop and the allowlist.
        let agent: ureq::Agent = ureq::Agent::config_builder()
            .http_status_as_error(false)
            .max_redirects(0)
            .timeout_global(Some(limits.timeout))
            .build()
            .into();

        Self {
            agent,
            allowlist,
            limits,
        }
    }

    pub fn allowlist(&self) -> &NetworkAllowlist {
        &self.allowlist
    }

    pub fn limits(&self) -> HttpLimits {
        self.limits
    }

    /// Forget every cookie this extension collected.
    pub fn clear_cookies(&self) {
        self.agent.cookie_jar_lock().clear();
    }

    /// Number of cookies currently stored; used by tests and diagnostics.
    pub fn cookie_count(&self) -> usize {
        self.agent.cookie_jar_lock().iter().count()
    }

    pub fn get(&self, url: &str, headers: &[(String, String)]) -> Result<HttpResponse, HttpError> {
        self.request("GET", url, None, headers)
    }

    pub fn post(
        &self,
        url: &str,
        body: Vec<u8>,
        headers: &[(String, String)],
    ) -> Result<HttpResponse, HttpError> {
        self.request("POST", url, Some(body), headers)
    }

    pub fn put(
        &self,
        url: &str,
        body: Vec<u8>,
        headers: &[(String, String)],
    ) -> Result<HttpResponse, HttpError> {
        self.request("PUT", url, Some(body), headers)
    }

    pub fn patch(
        &self,
        url: &str,
        body: Vec<u8>,
        headers: &[(String, String)],
    ) -> Result<HttpResponse, HttpError> {
        self.request("PATCH", url, Some(body), headers)
    }

    pub fn delete(&self, url: &str, headers: &[(String, String)]) -> Result<HttpResponse, HttpError> {
        self.request("DELETE", url, None, headers)
    }

    /// Execute one logical request, following redirects hop by hop.
    pub fn request(
        &self,
        method: &str,
        url: &str,
        body: Option<Vec<u8>>,
        headers: &[(String, String)],
    ) -> Result<HttpResponse, HttpError> {
        let (status_code, status_text, headers, bytes, final_url) =
            self.send(method, url, body, headers)?;

        Ok(HttpResponse {
            status_code,
            status: status_text,
            ok: (200..300).contains(&status_code),
            body: String::from_utf8_lossy(&bytes).into_owned(),
            url: final_url,
            headers,
        })
    }

    /// One response as raw bytes, following redirects.
    ///
    /// Host-only, and crucially unreachable from package code: a package file
    /// is a zip whose bytes cannot survive a text decode, so the host's own
    /// fetches (registry installs) need the body before any string coercion.
    pub fn get_bytes(&self, url: &str, headers: &[(String, String)]) -> Result<Vec<u8>, HttpError> {
        let (_, _, _, bytes, _) = self.send("GET", url, None, headers)?;
        Ok(bytes)
    }

    /// Run one method through the redirect loop and return the raw outcome.
    ///
    /// Every hop is allowlist-rechecked; credential headers are dropped when a
    /// redirect leaves the original host, so an allowlisted CDN never receives
    /// the user's bearer token. The cookie jar is unaffected: it attaches
    /// cookies per host on its own.
    fn send(
        &self,
        method: &str,
        url: &str,
        body: Option<Vec<u8>>,
        headers: &[(String, String)],
    ) -> Result<RawHop, HttpError> {
        let method = parse_method(method)?;

        // The first hop is checked before any socket work.
        let mut current: Url = self.allowlist.check_url(url)?;
        let mut current_method = method;
        let mut current_body = body;
        let mut current_headers: Vec<(String, String)> = headers
            .iter()
            .filter(|(name, _)| !is_reserved_request_header(name))
            .cloned()
            .collect();

        let mut hops: u32 = 0;

        loop {
            let mut builder = Request::builder()
                .method(current_method.clone())
                .uri(current.as_str());

            for (name, value) in &current_headers {
                let parsed = HeaderValue::from_str(value).map_err(|_| HttpError::Transport(
                    format!("header {name:?} has a value that is not a valid HTTP header"),
                ))?;
                builder = builder.header(name.as_str(), parsed);
            }

            let request = builder
                .body(current_body.clone().unwrap_or_default())
                .map_err(|error| HttpError::Transport(error.to_string()))?;

            let response = self
                .agent
                .run(request)
                .map_err(|error| map_transport_error(error, self.limits.timeout))?;

            let status = response.status();

            if is_redirect(status.as_u16()) {
                if hops >= self.limits.max_redirects {
                    return Err(HttpError::TooManyRedirects {
                        limit: self.limits.max_redirects,
                    });
                }

                let location = response
                    .headers()
                    .get(ureq::http::header::LOCATION)
                    .and_then(|value| value.to_str().ok())
                    .map(str::to_string);

                let Some(location) = location else {
                    // A redirect without a target is returned as-is: the
                    // package can inspect the status itself.
                    return finish(response, self.limits.max_body_bytes, current.as_str().to_string());
                };

                let next = current.join(&location).map_err(|_| HttpError::InvalidUrl {
                    url: location.clone(),
                })?;

                // Re-check every hop. This is the control that stops an
                // allowlisted host from bouncing the request elsewhere.
                let next = self.allowlist.check_url(next.as_str())?;

                if next.host_str() != current.host_str() {
                    strip_credential_headers(&mut current_headers);
                }

                match status.as_u16() {
                    // 303 always becomes a bodyless GET.
                    303 => {
                        current_method = Method::GET;
                        current_body = None;
                    }
                    // Browsers (and every package written against them) turn a
                    // POST into a GET on 301/302. Other methods are preserved.
                    301 | 302 if current_method == Method::POST => {
                        current_method = Method::GET;
                        current_body = None;
                    }
                    // 301/302 on other methods, and 307/308 by definition,
                    // preserve method and body.
                    _ => {}
                }

                current = next;
                hops += 1;
                continue;
            }

            return finish(
                response,
                self.limits.max_body_bytes,
                current.as_str().to_string(),
            );
        }
    }
}

fn parse_method(method: &str) -> Result<Method, HttpError> {
    let trimmed = method.trim();
    if trimmed.is_empty() {
        return Err(HttpError::InvalidMethod {
            method: method.to_string(),
        });
    }

    Method::from_bytes(trimmed.to_ascii_uppercase().as_bytes()).map_err(|_| {
        HttpError::InvalidMethod {
            method: method.to_string(),
        }
    })
}

fn is_redirect(status: u16) -> bool {
    matches!(status, 301 | 302 | 303 | 307 | 308)
}

fn normalise(name: &str) -> String {
    name.trim().to_ascii_lowercase()
}

fn is_reserved_request_header(name: &str) -> bool {
    let name = normalise(name);
    RESERVED_REQUEST_HEADERS.contains(&name.as_str())
}

/// Drop credentials when a redirect leaves the original host.
///
/// Without this, a provider that redirects to an allowlisted CDN would hand
/// that CDN the user's bearer token. The cookie jar is unaffected: it attaches
/// cookies per host on its own.
fn strip_credential_headers(headers: &mut Vec<(String, String)>) {
    headers.retain(|(name, _)| !CREDENTIAL_HEADERS.contains(&normalise(name).as_str()));
}

fn map_transport_error(error: ureq::Error, timeout: Duration) -> HttpError {
    match error {
        ureq::Error::Timeout(_) => HttpError::Timeout {
            seconds: timeout.as_secs(),
        },
        ureq::Error::Io(io_error) => match io_error.kind() {
            std::io::ErrorKind::TimedOut | std::io::ErrorKind::WouldBlock => HttpError::Timeout {
                seconds: timeout.as_secs(),
            },
            _ => HttpError::Transport(io_error.to_string()),
        },
        ureq::Error::BodyExceedsLimit(limit) => HttpError::BodyTooLarge { limit },
        other => HttpError::Transport(other.to_string()),
    }
}

/// Read a terminal response: status, headers, raw body, final URL.
fn finish(
    mut response: ureq::http::Response<ureq::Body>,
    max_body_bytes: u64,
    url: String,
) -> Result<RawHop, HttpError> {
    let status = response.status();
    let status_code = status.as_u16();

    let mut headers: BTreeMap<String, Vec<String>> = BTreeMap::new();
    for (name, value) in response.headers().iter() {
        let entry = headers
            .entry(normalise(name.as_str()))
            .or_default();
        entry.push(String::from_utf8_lossy(value.as_bytes()).into_owned());
    }

    let body = response
        .body_mut()
        .with_config()
        .limit(max_body_bytes)
        .read_to_vec()
        .map_err(|error| map_transport_error(error, Duration::from_secs(0)))?;

    Ok((
        status_code,
        status.canonical_reason().unwrap_or("").to_string(),
        headers,
        body,
        url,
    ))
}

/// Request shape handed over the JSON bridge.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct HttpCallSpec {
    #[serde(default = "default_method")]
    pub method: String,
    pub url: String,
    #[serde(default)]
    pub body: Option<String>,
    #[serde(default)]
    pub headers: BTreeMap<String, String>,
}

fn default_method() -> String {
    "GET".to_string()
}

impl HttpCallSpec {
    pub fn header_pairs(&self) -> Vec<(String, String)> {
        self.headers
            .iter()
            .map(|(name, value)| (name.clone(), value.clone()))
            .collect()
    }

    pub fn execute(&self, http: &HostHttp) -> Result<HttpResponse, HttpError> {
        http.request(
            &self.method,
            &self.url,
            self.body.clone().map(String::into_bytes),
            &self.header_pairs(),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::runtime::testing::spawn_server;
    use std::thread;

    fn allowlist(hosts: &[&str]) -> NetworkAllowlist {
        let owned: Vec<String> = hosts.iter().map(|host| host.to_string()).collect();
        NetworkAllowlist::from_patterns(&owned)
    }

    fn json_server() -> String {
        spawn_server(|request| match request.path.as_str() {
            "/json" => (
                200,
                vec![("Content-Type".into(), "application/json".into())],
                br#"{"hello":"world"}"#.to_vec(),
            ),
            "/missing" => (404, vec![], b"nope".to_vec()),
            _ => (404, vec![], b"unknown".to_vec()),
        })
    }

    fn redirect_server() -> String {
        spawn_server(|request| match request.path.as_str() {
            "/redirect/a" => (
                302,
                vec![("Location".into(), "/redirect/b".into())],
                Vec::new(),
            ),
            "/redirect/b" => (302, vec![("Location".into(), "/json".into())], Vec::new()),
            "/redirect/evil" => (
                302,
                vec![("Location".into(), "http://not-allowed.invalid/x".into())],
                Vec::new(),
            ),
            "/redirect/loop" => (
                302,
                vec![("Location".into(), "/redirect/loop".into())],
                Vec::new(),
            ),
            "/redirect/see-other" => (
                303,
                vec![("Location".into(), "/echo".into())],
                Vec::new(),
            ),
            "/redirect/temporary" => (
                307,
                vec![("Location".into(), "/echo".into())],
                Vec::new(),
            ),
            "/json" => (
                200,
                vec![("Content-Type".into(), "application/json".into())],
                br#"{"hello":"world"}"#.to_vec(),
            ),
            "/echo" => (
                200,
                vec![],
                format!("{} {}", request.method, request.body.len()).into_bytes(),
            ),
            _ => (404, vec![], b"unknown".to_vec()),
        })
    }

    fn http_for(base: &str) -> HostHttp {
        let host = base
            .trim_start_matches("http://")
            .split(':')
            .next()
            .unwrap_or("127.0.0.1");
        HostHttp::new(allowlist(&[host]), HttpLimits::default())
    }

    #[test]
    fn returns_json_body_status_and_headers() {
        let base = json_server();
        let http = http_for(&base);

        let response = http.get(&format!("{base}/json"), &[]).expect("request");

        assert_eq!(response.status_code, 200);
        assert_eq!(response.status, "OK");
        assert!(response.ok);
        assert_eq!(response.body, r#"{"hello":"world"}"#);
        assert_eq!(
            response.header("content-type"),
            Some("application/json")
        );
    }

    #[test]
    fn returns_404_as_a_response_not_an_error() {
        let base = json_server();
        let http = http_for(&base);

        let response = http.get(&format!("{base}/missing"), &[]).expect("request");

        assert_eq!(response.status_code, 404);
        assert_eq!(response.status, "Not Found");
        assert!(!response.ok);
        assert_eq!(response.body, "nope");
    }

    #[test]
    fn follows_a_redirect_chain_to_the_final_response() {
        let base = redirect_server();
        let http = http_for(&base);

        let response = http
            .get(&format!("{base}/redirect/a"), &[])
            .expect("request");

        assert_eq!(response.status_code, 200);
        assert_eq!(response.body, r#"{"hello":"world"}"#);
        assert_eq!(
            response.url,
            format!("{base}/json"),
            "url must report the final hop, matching the fetch() response contract"
        );
    }

    #[test]
    fn refuses_a_redirect_to_a_non_allowlisted_host() {
        let base = redirect_server();
        let http = http_for(&base);

        let error = http
            .get(&format!("{base}/redirect/evil"), &[])
            .expect_err("must be refused");

        assert_eq!(
            error,
            HttpError::Denied {
                host: "not-allowed.invalid".to_string()
            }
        );
        assert!(error.to_string().contains("not-allowed.invalid"));
    }

    #[test]
    fn stops_a_redirect_loop_at_the_hop_limit() {
        let base = redirect_server();
        let http = HostHttp::new(
            allowlist(&["127.0.0.1"]),
            HttpLimits {
                max_redirects: 3,
                ..HttpLimits::default()
            },
        );

        let error = http
            .get(&format!("{base}/redirect/loop"), &[])
            .expect_err("must stop");

        assert_eq!(error, HttpError::TooManyRedirects { limit: 3 });
    }

    #[test]
    fn set_cookies_are_echoed_on_the_next_request_and_cleared_on_demand() {
        let base = spawn_server(|request| match request.path.as_str() {
            "/set-cookie" => (
                200,
                vec![
                    ("Set-Cookie".into(), "sid=abc123; Path=/".into()),
                    ("Set-Cookie".into(), "theme=dark; Path=/".into()),
                ],
                b"ok".to_vec(),
            ),
            "/echo-cookies" => (
                200,
                vec![],
                request.header("cookie").unwrap_or("").as_bytes().to_vec(),
            ),
            _ => (404, vec![], b"unknown".to_vec()),
        });

        let http = http_for(&base);

        let first = http.get(&format!("{base}/set-cookie"), &[]).expect("request");
        assert_eq!(first.status_code, 200);
        assert_eq!(http.cookie_count(), 2);

        let echoed = http
            .get(&format!("{base}/echo-cookies"), &[])
            .expect("request");
        assert!(echoed.body.contains("sid=abc123"), "cookie jar sent {:?}", echoed.body);
        assert!(echoed.body.contains("theme=dark"), "cookie jar sent {:?}", echoed.body);

        http.clear_cookies();
        assert_eq!(http.cookie_count(), 0);

        let after = http
            .get(&format!("{base}/echo-cookies"), &[])
            .expect("request");
        assert!(
            after.body.trim().is_empty(),
            "cookies survived clearCookies(): {:?}",
            after.body
        );
    }

    #[test]
    fn repeated_response_headers_are_exposed_as_an_array() {
        let base = spawn_server(|_| {
            (
                200,
                vec![
                    ("Set-Cookie".into(), "a=1; Path=/".into()),
                    ("Set-Cookie".into(), "b=2; Path=/".into()),
                ],
                b"ok".to_vec(),
            )
        });

        let http = http_for(&base);
        let response = http.get(&format!("{base}/"), &[]).expect("request");

        assert_eq!(
            response.header_values("set-cookie"),
            ["a=1; Path=/", "b=2; Path=/"]
        );
    }

    #[test]
    fn slow_endpoint_raises_a_timeout_instead_of_hanging() {
        let base = spawn_server(|_| {
            thread::sleep(Duration::from_millis(1500));
            (200, vec![], b"late".to_vec())
        });

        let http = HostHttp::new(
            allowlist(&["127.0.0.1"]),
            HttpLimits {
                timeout: Duration::from_millis(250),
                ..HttpLimits::default()
            },
        );

        let error = http
            .get(&format!("{base}/"), &[])
            .expect_err("must time out");
        assert!(matches!(error, HttpError::Timeout { .. }), "got {error:?}");
    }

    #[test]
    fn request_to_a_non_allowlisted_host_never_reaches_the_network() {
        let http = HostHttp::new(allowlist(&["api.example.com"]), HttpLimits::default());

        let error = http
            .get("http://denied.invalid/x", &[])
            .expect_err("must be refused");
        assert_eq!(
            error,
            HttpError::Denied {
                host: "denied.invalid".to_string()
            }
        );
    }

    #[test]
    fn extension_supplied_host_header_is_ignored() {
        let base = spawn_server(|request| {
            (
                200,
                vec![],
                request.header("host").unwrap_or("").as_bytes().to_vec(),
            )
        });

        let http = http_for(&base);
        let response = http
            .get(
                &format!("{base}/"),
                &[("Host".to_string(), "evil.invalid".to_string())],
            )
            .expect("request");

        assert!(
            response.body.starts_with("127.0.0.1"),
            "server saw Host {:?}",
            response.body
        );
    }

    #[test]
    fn post_sends_the_body() {
        let base = redirect_server();
        let http = http_for(&base);

        let response = http
            .post(&format!("{base}/echo"), b"abc".to_vec(), &[])
            .expect("request");

        assert_eq!(response.body, "POST 3");
    }

    /// The host's own fetches (registry installs) read zip bytes; this asserts
    /// they arrive before any text coercion would mangle them.
    #[test]
    fn get_bytes_returns_the_raw_body_and_follows_redirects() {
        let base = spawn_server(|request| match request.path.as_str() {
            "/package" => (200, vec![], (0u8..=255u8).collect::<Vec<u8>>().clone()),
            "/bounce" => (
                302,
                vec![("Location".into(), "/package".into())],
                Vec::new(),
            ),
            _ => (404, vec![], b"unknown".to_vec()),
        });
        let http = http_for(&base);

        let direct = http.get_bytes(&format!("{base}/package"), &[]).expect("direct");
        let expected: Vec<u8> = (0..=255).collect();
        assert_eq!(direct, expected);

        let bounced = http.get_bytes(&format!("{base}/bounce"), &[]).expect("redirected");
        assert_eq!(bounced, expected);
    }

    #[test]
    fn see_other_redirect_rewrites_the_method_to_get() {
        let base = redirect_server();
        let http = http_for(&base);

        let response = http
            .post(&format!("{base}/redirect/see-other"), b"abc".to_vec(), &[])
            .expect("request");

        assert_eq!(response.body, "GET 0");
    }

    #[test]
    fn temporary_redirect_preserves_method_and_body() {
        let base = redirect_server();
        let http = http_for(&base);

        let response = http
            .post(&format!("{base}/redirect/temporary"), b"abc".to_vec(), &[])
            .expect("request");

        assert_eq!(response.body, "POST 3");
    }

    #[test]
    fn invalid_method_is_a_typed_error() {
        let base = json_server();
        let http = http_for(&base);

        let error = http
            .request("GE T", &format!("{base}/json"), None, &[])
            .expect_err("invalid method");
        assert!(matches!(error, HttpError::InvalidMethod { .. }), "got {error:?}");
    }

    #[test]
    fn response_serialises_to_the_javascript_shape() {
        let response = HttpResponse {
            status_code: 200,
            status: "OK".to_string(),
            ok: true,
            url: "https://api.example.com/v1".to_string(),
            body: "{}".to_string(),
            headers: BTreeMap::from([("content-type".to_string(), vec!["application/json".to_string()])]),
        };

        let value = serde_json::to_value(&response).expect("serialises");

        assert_eq!(value["statusCode"], 200);
        assert_eq!(value["status"], "OK");
        assert_eq!(value["ok"], true);
        assert_eq!(value["url"], "https://api.example.com/v1");
        assert_eq!(value["body"], "{}");
        assert_eq!(value["headers"]["content-type"][0], "application/json");
        assert!(value.get("status_code").is_none());
    }

    #[test]
    fn call_spec_deserialises_from_javascript() {
        let spec: HttpCallSpec = serde_json::from_str(
            r#"{
                "url": "https://api.example.com/v1",
                "method": "POST",
                "body": "{\"a\":1}",
                "headers": { "Content-Type": "application/json" }
            }"#,
        )
        .expect("parses");

        assert_eq!(spec.method, "POST");
        assert_eq!(spec.body.as_deref(), Some("{\"a\":1}"));
        assert_eq!(spec.header_pairs().len(), 1);
    }

    #[test]
    fn credentials_are_stripped_when_a_redirect_leaves_the_host() {
        let mut headers = vec![
            ("Authorization".to_string(), "Bearer secret".to_string()),
            ("Cookie".to_string(), "sid=abc".to_string()),
            ("Proxy-Authorization".to_string(), "Basic x".to_string()),
            ("X-Api-Key".to_string(), "public".to_string()),
        ];

        strip_credential_headers(&mut headers);

        assert_eq!(
            headers,
            vec![("X-Api-Key".to_string(), "public".to_string())]
        );
    }

    #[test]
    fn framing_headers_from_extensions_are_reserved() {
        for name in [
            "Host",
            "host",
            "Content-Length",
            "Transfer-Encoding",
            "Connection",
            "Proxy-Connection",
        ] {
            assert!(is_reserved_request_header(name), "{name} must be reserved");
        }

        assert!(!is_reserved_request_header("X-Custom-Header"));
        assert!(!is_reserved_request_header("Authorization"));
    }

    #[test]
    fn two_extensions_never_share_a_cookie_jar() {
        let base = spawn_server(|request| match request.path.as_str() {
            "/set-cookie" => (
                200,
                vec![("Set-Cookie".into(), "sid=abc123; Path=/".into())],
                b"ok".to_vec(),
            ),
            "/echo-cookies" => (
                200,
                vec![],
                request.header("cookie").unwrap_or("").as_bytes().to_vec(),
            ),
            _ => (404, vec![], b"unknown".to_vec()),
        });

        let first = http_for(&base);
        let second = http_for(&base);

        first
            .get(&format!("{base}/set-cookie"), &[])
            .expect("request");

        assert_eq!(first.cookie_count(), 1);
        assert_eq!(second.cookie_count(), 0);

        let response = second
            .get(&format!("{base}/echo-cookies"), &[])
            .expect("request");
        assert!(
            response.body.trim().is_empty(),
            "second extension received the first extension's cookies: {:?}",
            response.body
        );
    }
}
