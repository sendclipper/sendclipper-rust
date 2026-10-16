use std::time::{SystemTime, UNIX_EPOCH};

use hmac::{Hmac, Mac};
use sha2::Sha256;

use crate::error::WebhookVerificationError;
use crate::types::WebhookEvent;

/// SendClipper's documented tolerance: five minutes.
pub const WEBHOOK_TOLERANCE_SECONDS: u64 = 300;

/// The header SendClipper signs deliveries in.
pub const SIGNATURE_HEADER: &str = "x-sendclipper-signature-v2";

/// Where [`verify_webhook`] finds the signature: an `http::HeaderMap` (axum,
/// actix-web, hyper, reqwest), name/value pairs, or just the header's value.
pub enum Headers<'a> {
    Map(&'a http::HeaderMap),
    Pairs(Vec<(&'a str, &'a str)>),
    Value(&'a str),
}

impl<'a> From<&'a http::HeaderMap> for Headers<'a> {
    fn from(h: &'a http::HeaderMap) -> Self {
        Headers::Map(h)
    }
}

impl<'a> From<&'a str> for Headers<'a> {
    fn from(v: &'a str) -> Self {
        Headers::Value(v)
    }
}

impl<'a> From<&'a String> for Headers<'a> {
    fn from(v: &'a String) -> Self {
        Headers::Value(v.as_str())
    }
}

impl<'a> From<Vec<(&'a str, &'a str)>> for Headers<'a> {
    fn from(v: Vec<(&'a str, &'a str)>) -> Self {
        Headers::Pairs(v)
    }
}

/// Tunes [`verify_webhook`].
#[derive(Debug, Clone, Copy, Default)]
pub struct VerifyOptions {
    /// Default five minutes.
    pub tolerance_seconds: Option<u64>,
    /// The current time in seconds, for tests.
    pub now: Option<u64>,
}

fn refuse(reason: &'static str, message: impl Into<String>) -> WebhookVerificationError {
    WebhookVerificationError {
        reason,
        message: message.into(),
    }
}

/// Verify a webhook delivery and return its parsed event.
///
/// `payload` is the body EXACTLY as received (`axum::body::Bytes`,
/// `actix_web::web::Bytes`), never re-encoded JSON.
///
/// SendClipper signs `"{timestamp}.{raw_body}"` with HMAC-SHA256 and sends
/// `X-SendClipper-Signature-V2: t=<seconds>,v1=<hex>[,v1=<hex>]`. During a
/// secret rotation there are two `v1=` values; any match is accepted.
pub fn verify_webhook<'h>(
    payload: &[u8],
    headers: impl Into<Headers<'h>>,
    secret: &str,
    options: VerifyOptions,
) -> Result<WebhookEvent, WebhookVerificationError> {
    if secret.is_empty() {
        return Err(refuse("missing_header", "No webhook signing secret was given."));
    }
    let header = match headers.into() {
        Headers::Value(v) => v.to_owned(),
        Headers::Map(m) => m
            .get_all(SIGNATURE_HEADER)
            .iter()
            .filter_map(|v| v.to_str().ok())
            .collect::<Vec<_>>()
            .join(","),
        Headers::Pairs(p) => p
            .iter()
            .filter(|(k, _)| {
                let n = k.to_ascii_lowercase().replace('_', "-");
                n == SIGNATURE_HEADER || n == format!("http-{SIGNATURE_HEADER}")
            })
            .map(|(_, v)| *v)
            .collect::<Vec<_>>()
            .join(","),
    };
    if header.is_empty() {
        return Err(refuse(
            "missing_header",
            "The request carries no X-SendClipper-Signature-V2 header.",
        ));
    }
    let (timestamp, signatures) = parse_signature_header(&header);
    let Some(timestamp) = timestamp.filter(|_| !signatures.is_empty()) else {
        return Err(refuse("malformed_header", "X-SendClipper-Signature-V2 has no t= or no v1= value."));
    };

    let tolerance = options.tolerance_seconds.unwrap_or(WEBHOOK_TOLERANCE_SECONDS);
    let now = options
        .now
        .unwrap_or_else(|| SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0));
    let drift = now.abs_diff(timestamp);
    if drift > tolerance {
        return Err(refuse(
            "timestamp_out_of_tolerance",
            format!("The delivery was signed {drift} s from now, outside the {tolerance} s tolerance."),
        ));
    }

    // ANY match, and every candidate is compared: a rotation sends two.
    let mut matched = false;
    for candidate in &signatures {
        let Ok(bytes) = hex::decode(candidate) else { continue };
        let mut mac = Hmac::<Sha256>::new_from_slice(secret.as_bytes()).expect("HMAC takes any key length");
        mac.update(format!("{timestamp}.").as_bytes());
        mac.update(payload);
        if mac.verify_slice(&bytes).is_ok() {
            matched = true;
        }
    }
    if !matched {
        return Err(refuse("signature_mismatch", "The signature does not match this secret and body."));
    }
    serde_json::from_slice(payload).map_err(|e| refuse("invalid_payload", format!("The signed body is not a webhook event: {e}")))
}

/// `hex(HMAC-SHA256(secret, "{timestamp}.{raw_body}"))`. Exported for tests of your own handler.
pub fn sign_payload(secret: &str, timestamp: u64, raw_body: &[u8]) -> String {
    let mut mac = Hmac::<Sha256>::new_from_slice(secret.as_bytes()).expect("HMAC takes any key length");
    mac.update(format!("{timestamp}.").as_bytes());
    mac.update(raw_body);
    hex::encode(mac.finalize().into_bytes())
}

/// A valid header value for a body, e.g. to test your endpoint locally.
/// `None` signs at the current time.
pub fn webhook_signature_header(secret: &str, raw_body: &[u8], timestamp: Option<u64>) -> String {
    let t = timestamp.unwrap_or_else(|| SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0));
    format!("t={t},v1={}", sign_payload(secret, t, raw_body))
}

/// `(timestamp, signatures)` from `t=<seconds>,v1=<hex>[,v1=<hex>]`.
pub fn parse_signature_header(header: &str) -> (Option<u64>, Vec<String>) {
    let mut timestamp = None;
    let mut signatures = Vec::new();
    for part in header.split(',') {
        let Some((k, v)) = part.split_once('=') else { continue };
        let (k, v) = (k.trim(), v.trim());
        if k == "t" && !v.is_empty() && v.bytes().all(|b| b.is_ascii_digit()) {
            timestamp = v.parse().ok();
        } else if k == "v1" && !v.is_empty() {
            signatures.push(v.to_owned());
        }
    }
    (timestamp, signatures)
}
