use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::Duration;

use reqwest::header::{HeaderMap, HeaderName, HeaderValue};
use serde::de::DeserializeOwned;
use serde::Serialize;

use crate::error::{Error, Result};
use crate::resources::*;
use crate::VERSION;

/// Where requests go unless [`ClientBuilder::base_url`] or `SENDCLIPPER_BASE_URL` says otherwise.
pub const DEFAULT_BASE_URL: &str = "https://sendclipper.com";

const MAX_RETRY_AFTER: Duration = Duration::from_secs(60);

/// The SendClipper API client. Cheap to clone and safe to share across tasks.
///
/// ```no_run
/// # async fn run() -> sendclipper::Result<()> {
/// let client = sendclipper::Client::new("sc_live_xxxxxxxxxxxxxxxx");
/// let email = client
///     .emails()
///     .send(&sendclipper::SendEmailParams {
///         from: "hello@send.acme.io".into(),
///         to: "ada@lumen.io".into(),
///         subject: Some("Your receipt".into()),
///         html: Some("<p>Thanks for your order.</p>".into()),
///         ..Default::default()
///     })
///     .await?;
/// println!("Email {} accepted", email.id);
/// # Ok(())
/// # }
/// ```
#[derive(Clone)]
pub struct Client {
    inner: Arc<Inner>,
    opts: RequestOptions,
}

struct Inner {
    api_key: Option<String>,
    base_url: String,
    http: reqwest::Client,
    timeout: Duration,
    max_retries: u32,
    default_headers: Vec<(String, String)>,
}

/// Per-call settings, set with [`Client::with_idempotency_key`] and friends.
#[derive(Clone, Default, Debug)]
pub(crate) struct RequestOptions {
    idempotency_key: Option<String>,
    timeout: Option<Duration>,
    max_retries: Option<u32>,
    headers: Vec<(String, String)>,
    bearer: Option<String>,
}

/// Builds a [`Client`].
pub struct ClientBuilder {
    api_key: Option<String>,
    base_url: Option<String>,
    timeout: Duration,
    max_retries: u32,
    default_headers: Vec<(String, String)>,
    http: Option<reqwest::Client>,
}

impl ClientBuilder {
    /// A `sc_live_...` key. Defaults to `SENDCLIPPER_API_KEY`.
    pub fn api_key(mut self, key: impl Into<String>) -> Self {
        self.api_key = Some(key.into());
        self
    }

    /// Another API origin (no `/v1`). Defaults to `SENDCLIPPER_BASE_URL`, then `https://sendclipper.com`.
    pub fn base_url(mut self, url: impl Into<String>) -> Self {
        self.base_url = Some(url.into());
        self
    }

    /// The per-attempt timeout. Default 30 seconds.
    pub fn timeout(mut self, d: Duration) -> Self {
        self.timeout = d;
        self
    }

    /// How often a network error, a 429 or a 5xx is retried. Default 2.
    pub fn max_retries(mut self, n: u32) -> Self {
        self.max_retries = n;
        self
    }

    /// A header sent on every request.
    pub fn default_header(mut self, name: impl Into<String>, value: impl Into<String>) -> Self {
        self.default_headers.push((name.into(), value.into()));
        self
    }

    /// Your own `reqwest::Client` (a proxy, custom TLS). Redirects to another
    /// host should not carry the `Authorization` header; reqwest's default
    /// policy already drops it.
    pub fn http_client(mut self, http: reqwest::Client) -> Self {
        self.http = Some(http);
        self
    }

    pub fn build(self) -> Client {
        let api_key = self
            .api_key
            .filter(|k| !k.is_empty())
            .or_else(|| std::env::var("SENDCLIPPER_API_KEY").ok().filter(|k| !k.is_empty()));
        let base = self
            .base_url
            .filter(|u| !u.is_empty())
            .or_else(|| std::env::var("SENDCLIPPER_BASE_URL").ok().filter(|u| !u.is_empty()))
            .unwrap_or_else(|| DEFAULT_BASE_URL.to_owned());
        let base = base.trim_end_matches('/');
        let base = base.strip_suffix("/v1").unwrap_or(base).to_owned();
        Client {
            inner: Arc::new(Inner {
                api_key,
                base_url: base,
                http: self.http.unwrap_or_default(),
                timeout: self.timeout,
                max_retries: self.max_retries,
                default_headers: self.default_headers,
            }),
            opts: RequestOptions::default(),
        }
    }
}

pub(crate) enum Body {
    None,
    Json(Vec<u8>),
    Form(Vec<u8>, String),
}

pub(crate) enum As {
    Json,
    Text,
    Bytes,
}

impl Client {
    /// A client with this API key and every other setting at its default.
    /// An empty key reads `SENDCLIPPER_API_KEY`.
    pub fn new(api_key: impl Into<String>) -> Client {
        Client::builder().api_key(api_key).build()
    }

    /// A client that reads `SENDCLIPPER_API_KEY` and `SENDCLIPPER_BASE_URL`.
    pub fn from_env() -> Client {
        Client::builder().build()
    }

    pub fn builder() -> ClientBuilder {
        ClientBuilder {
            api_key: None,
            base_url: None,
            timeout: Duration::from_secs(30),
            max_retries: 2,
            default_headers: Vec::new(),
            http: None,
        }
    }

    /// The API origin requests go to, without `/v1`.
    pub fn base_url(&self) -> &str {
        &self.inner.base_url
    }

    /// A copy of this client whose next writes carry YOUR Idempotency-Key, so
    /// your own retries of the same logical operation are safe too:
    ///
    /// ```no_run
    /// # async fn run(client: sendclipper::Client, params: sendclipper::SendEmailParams) -> sendclipper::Result<()> {
    /// client.with_idempotency_key("order-42-receipt").emails().send(&params).await?;
    /// # Ok(())
    /// # }
    /// ```
    pub fn with_idempotency_key(&self, key: impl Into<String>) -> Client {
        let mut c = self.clone();
        c.opts.idempotency_key = Some(key.into());
        c
    }

    /// A copy of this client with another per-attempt timeout.
    pub fn with_timeout(&self, d: Duration) -> Client {
        let mut c = self.clone();
        c.opts.timeout = Some(d);
        c
    }

    /// A copy of this client with another retry count.
    pub fn with_max_retries(&self, n: u32) -> Client {
        let mut c = self.clone();
        c.opts.max_retries = Some(n);
        c
    }

    /// A copy of this client that adds a header to its requests.
    pub fn with_header(&self, name: impl Into<String>, value: impl Into<String>) -> Client {
        let mut c = self.clone();
        c.opts.headers.push((name.into(), value.into()));
        c
    }

    pub(crate) fn with_bearer(&self, token: &str) -> Client {
        let mut c = self.clone();
        c.opts.bearer = Some(token.to_owned());
        c
    }

    pub fn emails(&self) -> Emails<'_> {
        Emails { c: self }
    }
    pub fn batch(&self) -> Batch<'_> {
        Batch { c: self }
    }
    pub fn domains(&self) -> Domains<'_> {
        Domains { c: self }
    }
    pub fn templates(&self) -> Templates<'_> {
        Templates { c: self }
    }
    pub fn sandboxes(&self) -> Sandboxes<'_> {
        Sandboxes { c: self }
    }
    pub fn webhooks(&self) -> Webhooks<'_> {
        Webhooks { c: self }
    }
    pub fn contacts(&self) -> Contacts<'_> {
        Contacts { c: self }
    }
    pub fn audiences(&self) -> Audiences<'_> {
        Audiences { c: self }
    }
    pub fn suppressions(&self) -> Suppressions<'_> {
        Suppressions { c: self }
    }
    pub fn mailboxes(&self) -> Mailboxes<'_> {
        Mailboxes { c: self }
    }
    pub fn automations(&self) -> Automations<'_> {
        Automations { c: self }
    }
    pub fn workspace(&self) -> WorkspaceResource<'_> {
        WorkspaceResource { c: self }
    }
    pub fn api_keys(&self) -> ApiKeys<'_> {
        ApiKeys { c: self }
    }

    // ------------------------------------------------------------ plumbing

    pub(crate) async fn get<T: DeserializeOwned>(&self, path: &str, query: &[(&str, String)]) -> Result<T> {
        self.json("GET", path, query, Body::None).await
    }

    pub(crate) async fn send_json<T: DeserializeOwned, B: Serialize + ?Sized>(&self, method: &str, path: &str, body: &B) -> Result<T> {
        let bytes = serde_json::to_vec(body).map_err(|e| Error::new("invalid_request", format!("encoding the request: {e}"), None))?;
        // A request body is always a JSON object.
        let bytes = if bytes == b"null" { b"{}".to_vec() } else { bytes };
        self.json(method, path, &[], Body::Json(bytes)).await
    }

    pub(crate) async fn send_empty<T: DeserializeOwned>(&self, method: &str, path: &str) -> Result<T> {
        let body = if method == "POST" { Body::Json(b"{}".to_vec()) } else { Body::None };
        self.json(method, path, &[], body).await
    }

    pub(crate) async fn json<T: DeserializeOwned>(&self, method: &str, path: &str, query: &[(&str, String)], body: Body) -> Result<T> {
        let (bytes, headers) = self.request(method, path, query, body, As::Json).await?;
        let bytes = if bytes.iter().all(u8::is_ascii_whitespace) {
            b"null".to_vec()
        } else {
            bytes
        };
        serde_json::from_slice(&bytes).map_err(|e| {
            let mut err = Error::new(
                "invalid_response",
                format!("SendClipper's answer could not be decoded: {e}"),
                Some(200),
            );
            err.headers = headers;
            err
        })
    }

    pub(crate) async fn text(&self, path: &str, query: &[(&str, String)]) -> Result<String> {
        let (bytes, _) = self.request("GET", path, query, Body::None, As::Text).await?;
        Ok(String::from_utf8_lossy(&bytes).into_owned())
    }

    pub(crate) async fn bytes(&self, path: &str, query: &[(&str, String)]) -> Result<(Vec<u8>, BTreeMap<String, String>)> {
        self.request("GET", path, query, Body::None, As::Bytes).await
    }

    async fn request(
        &self,
        method: &str,
        path: &str,
        query: &[(&str, String)],
        body: Body,
        as_: As,
    ) -> Result<(Vec<u8>, BTreeMap<String, String>)> {
        let inner = &self.inner;
        let token = self
            .opts
            .bearer
            .as_deref()
            .or(inner.api_key.as_deref())
            .filter(|t| !t.is_empty())
            .ok_or_else(|| {
                Error::new(
                    "api_key_missing",
                    "No SendClipper API key. Pass one to sendclipper::Client::new(\"sc_live_...\") or set SENDCLIPPER_API_KEY.",
                    Some(401),
                )
            })?;

        let mut url = format!("{}/v1{}", inner.base_url, path);
        let pairs: Vec<String> = query
            .iter()
            .filter(|(_, v)| !v.is_empty())
            .map(|(k, v)| format!("{}={}", enc(k), enc(v)))
            .collect();
        if !pairs.is_empty() {
            url.push('?');
            url.push_str(&pairs.join("&"));
        }

        let mut headers: Vec<(String, String)> = vec![
            ("Authorization".into(), format!("Bearer {token}")),
            (
                "Accept".into(),
                if matches!(as_, As::Json) {
                    "application/json".into()
                } else {
                    "*/*".into()
                },
            ),
            ("User-Agent".into(), format!("sendclipper-rust/{VERSION}")),
        ];
        headers.extend(inner.default_headers.iter().cloned());
        headers.extend(self.opts.headers.iter().cloned());

        let (payload, is_form) = match body {
            Body::None => (None, false),
            Body::Json(b) => {
                headers.push(("Content-Type".into(), "application/json".into()));
                (Some(b), false)
            }
            Body::Form(b, ctype) => {
                headers.push(("Content-Type".into(), ctype));
                (Some(b), true)
            }
        };

        let read = method == "GET" || method == "HEAD";
        // The API de-duplicates every JSON mutation on this header for 24 hours.
        // A multipart body is not fingerprinted server-side, so it gets no key
        // and is never retried.
        let mut has_key = false;
        if !read && !is_form {
            let key = self
                .opts
                .idempotency_key
                .clone()
                .unwrap_or_else(|| format!("sc-rust-{}", uuid::Uuid::new_v4()));
            headers.push(("Idempotency-Key".into(), key));
            has_key = true;
        } else if let Some(key) = &self.opts.idempotency_key {
            headers.push(("Idempotency-Key".into(), key.clone()));
            has_key = true;
        }
        let retries = if read || has_key {
            self.opts.max_retries.unwrap_or(inner.max_retries)
        } else {
            0
        };
        let timeout = self.opts.timeout.unwrap_or(inner.timeout);

        let mut header_map = HeaderMap::new();
        for (k, v) in &headers {
            let name = HeaderName::from_bytes(k.as_bytes()).map_err(|e| Error::new("invalid_request", format!("header {k}: {e}"), None))?;
            let value = HeaderValue::from_str(v).map_err(|e| Error::new("invalid_request", format!("header {k}: {e}"), None))?;
            header_map.insert(name, value);
        }
        let method_v = reqwest::Method::from_bytes(method.as_bytes()).expect("a valid method");

        let mut attempt: u32 = 0;
        loop {
            let mut rb = inner
                .http
                .request(method_v.clone(), &url)
                .headers(header_map.clone())
                .timeout(timeout);
            if let Some(p) = &payload {
                rb = rb.body(p.clone());
            }
            let res = match rb.send().await {
                Ok(r) => r,
                Err(e) => {
                    if attempt < retries {
                        tokio::time::sleep(backoff(attempt)).await;
                        attempt += 1;
                        continue;
                    }
                    let path_only = url.split('?').next().unwrap_or(&url).trim_start_matches(&inner.base_url).to_owned();
                    return Err(if e.is_timeout() {
                        Error::network(format!(
                            "SendClipper did not answer within {:?} ({} {}).",
                            timeout, method, path_only
                        ))
                    } else {
                        Error::network(format!("Could not reach SendClipper at {}: {}", inner.base_url, source_chain(&e)))
                    });
                }
            };
            let status = res.status().as_u16();
            let res_headers: BTreeMap<String, String> = res
                .headers()
                .iter()
                .map(|(k, v)| (k.as_str().to_ascii_lowercase(), v.to_str().unwrap_or("").to_owned()))
                .collect();
            let ok = (200..300).contains(&status);

            if !ok && matches!(status, 408 | 429 | 500 | 502 | 503 | 504) && attempt < retries {
                tokio::time::sleep(retry_after(&res_headers).unwrap_or_else(|| backoff(attempt))).await;
                attempt += 1;
                continue;
            }
            let bytes = match res.bytes().await {
                Ok(b) => b.to_vec(),
                Err(e) => {
                    if attempt < retries {
                        tokio::time::sleep(backoff(attempt)).await;
                        attempt += 1;
                        continue;
                    }
                    return Err(Error::network(format!("Could not read SendClipper's answer: {}", source_chain(&e))));
                }
            };
            if !ok {
                let err = Error::from_response(status, &bytes, res_headers.clone());
                // Our own earlier attempt is still running server-side (a timeout
                // that was not a failure). Its result is replayed once it finishes.
                if err.name == "idempotency_key_in_flight" && attempt < retries {
                    tokio::time::sleep(retry_after(&res_headers).unwrap_or_else(|| backoff(attempt))).await;
                    attempt += 1;
                    continue;
                }
                return Err(err);
            }
            return Ok((bytes, res_headers));
        }
    }
}

impl std::fmt::Debug for Client {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Client")
            .field("base_url", &self.inner.base_url)
            .finish_non_exhaustive()
    }
}

fn source_chain(e: &dyn std::error::Error) -> String {
    let mut out = e.to_string();
    let mut cur = e.source();
    while let Some(s) = cur {
        out.push_str(": ");
        out.push_str(&s.to_string());
        cur = s.source();
    }
    out
}

/// Percent-encode one path segment or query value (RFC 3986 unreserved kept).
pub(crate) fn enc(s: &str) -> String {
    const SET: &percent_encoding::AsciiSet = &percent_encoding::NON_ALPHANUMERIC
        .remove(b'-')
        .remove(b'.')
        .remove(b'_')
        .remove(b'~');
    percent_encoding::utf8_percent_encode(s, SET).to_string()
}

pub(crate) fn backoff(attempt: u32) -> Duration {
    let base = (0.5 * 2f64.powi(attempt.min(10) as i32)).min(8.0);
    let jitter = (uuid::Uuid::new_v4().as_u128() % 1_000_000) as f64 / 1_000_000.0;
    Duration::from_secs_f64(base / 2.0 + jitter * (base / 2.0))
}

fn retry_after(headers: &BTreeMap<String, String>) -> Option<Duration> {
    let raw = headers.get("retry-after")?.trim();
    if let Ok(secs) = raw.parse::<f64>() {
        if secs >= 0.0 {
            return Some(Duration::from_secs_f64(secs).min(MAX_RETRY_AFTER));
        }
    }
    let at = httpdate::parse_http_date(raw).ok()?;
    Some(
        at.duration_since(std::time::SystemTime::now())
            .unwrap_or(Duration::ZERO)
            .min(MAX_RETRY_AFTER),
    )
}

/// Query pairs: empty values are dropped by the client.
pub(crate) struct Q(pub Vec<(&'static str, String)>);

impl Q {
    pub(crate) fn new() -> Q {
        Q(Vec::new())
    }

    pub(crate) fn s(mut self, k: &'static str, v: &Option<String>) -> Q {
        if let Some(v) = v {
            self.0.push((k, v.clone()));
        }
        self
    }

    pub(crate) fn n(mut self, k: &'static str, v: Option<u32>) -> Q {
        if let Some(v) = v {
            self.0.push((k, v.to_string()));
        }
        self
    }

    /// Booleans are written the way the API reads them: `true` / `false`.
    pub(crate) fn b(mut self, k: &'static str, v: Option<bool>) -> Q {
        if let Some(v) = v {
            self.0.push((k, v.to_string()));
        }
        self
    }
}
