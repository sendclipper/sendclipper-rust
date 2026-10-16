use std::collections::BTreeMap;
use std::fmt;

/// What every failed call returns: an API refusal, or a request that never
/// got an answer. Branch on [`Error::name`], SendClipper's machine-readable
/// code; [`Error::message`] is a sentence meant for a person.
#[derive(Debug, Clone)]
pub struct Error {
    /// The API's code: `domain_not_verified`, `template_variables_missing`,
    /// `validation_failed`, `rate_limited`, or `network_error` when no answer arrived.
    pub name: String,
    /// A sentence meant for a person.
    pub message: String,
    /// The HTTP status, or `None` when no answer arrived.
    pub status_code: Option<u16>,
    /// Code-specific facts (e.g. the missing template tags).
    pub detail: Option<serde_json::Value>,
    /// The response headers (lower-case names), or empty when no answer arrived.
    pub headers: BTreeMap<String, String>,
}

/// `Result<T, sendclipper::Error>`.
pub type Result<T> = std::result::Result<T, Error>;

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self.status_code {
            Some(s) => write!(f, "sendclipper: {} ({}): {}", self.name, s, self.message),
            None => write!(f, "sendclipper: {}: {}", self.name, self.message),
        }
    }
}

impl std::error::Error for Error {}

impl Error {
    pub(crate) fn new(name: impl Into<String>, message: impl Into<String>, status_code: Option<u16>) -> Self {
        Error {
            name: name.into(),
            message: message.into(),
            status_code,
            detail: None,
            headers: BTreeMap::new(),
        }
    }

    pub(crate) fn network(message: impl Into<String>) -> Self {
        Error::new("network_error", message, None)
    }

    /// The API answers refusals in two shapes: the house envelope
    /// `{ error: { code, message, detail? } }` and Nest's default
    /// `{ statusCode, message: string | string[], error }` (request validation).
    pub(crate) fn from_response(status: u16, body: &[u8], headers: BTreeMap<String, String>) -> Self {
        use serde_json::Value;
        let parsed: Value = serde_json::from_slice(body).unwrap_or(Value::Null);
        let mut err = Error::new(code_for_status(status), format!("HTTP {status}"), Some(status));
        err.headers = headers;

        let envelope = |err: &mut Error, env: &serde_json::Map<String, Value>| {
            if let Some(code) = scalar(env.get("code")) {
                err.name = code;
            }
            if let Some(message) = scalar(env.get("message")) {
                err.message = message;
            }
            err.detail = env.get("detail").or_else(|| env.get("details")).cloned().filter(|v| !v.is_null());
        };

        if let Some(env) = parsed.get("error").and_then(Value::as_object) {
            envelope(&mut err, env);
            return err;
        }
        match parsed.get("message") {
            Some(Value::Object(m)) if m.get("error").is_some_and(Value::is_object) => {
                envelope(&mut err, m["error"].as_object().expect("checked"));
            }
            Some(Value::Array(list)) => {
                err.name = "validation_failed".into();
                err.message = list
                    .iter()
                    .map(|v| v.as_str().map(str::to_owned).unwrap_or_else(|| v.to_string()))
                    .collect::<Vec<_>>()
                    .join("; ");
                err.detail = Some(Value::Array(list.clone()));
            }
            Some(Value::String(s)) => err.message = s.clone(),
            _ => {
                let text = String::from_utf8_lossy(body);
                let snippet: String = text.trim().chars().take(300).collect();
                if !snippet.is_empty() {
                    err.message = snippet;
                }
            }
        }
        err
    }
}

fn scalar(v: Option<&serde_json::Value>) -> Option<String> {
    match v? {
        serde_json::Value::String(s) if !s.is_empty() => Some(s.clone()),
        serde_json::Value::Number(n) => Some(n.to_string()),
        serde_json::Value::Bool(b) => Some(b.to_string()),
        _ => None,
    }
}

fn code_for_status(status: u16) -> String {
    match status {
        400 => "bad_request".into(),
        401 => "unauthorized".into(),
        403 => "forbidden".into(),
        404 => "not_found".into(),
        409 => "conflict".into(),
        413 => "payload_too_large".into(),
        429 => "rate_limited".into(),
        s => format!("http_{s}"),
    }
}

/// Why [`crate::verify_webhook`] refused a delivery.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WebhookVerificationError {
    /// `missing_header`, `malformed_header`, `timestamp_out_of_tolerance`,
    /// `signature_mismatch` or `invalid_payload`.
    pub reason: &'static str,
    pub message: String,
}

impl fmt::Display for WebhookVerificationError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "sendclipper: webhook {}: {}", self.reason, self.message)
    }
}

impl std::error::Error for WebhookVerificationError {}
