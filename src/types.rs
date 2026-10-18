//! Request and response shapes of the SendClipper REST API (`/v1`).
//!
//! The API only ever ADDS fields inside `/v1`; it never removes or retypes
//! one. Unknown response fields are ignored, and a missing one takes its
//! default. Timestamps are ISO 8601 strings, exactly as the API sends them.

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

/// A JSON object, for open-ended fields.
pub type Object = Map<String, Value>;

/// The paging block of a cursor-addressed list.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct CursorInfo {
    pub cursor: Option<String>,
    pub has_more: bool,
}

/// An `{ id, name }` pair.
#[derive(Debug, Clone, Default, Deserialize, Serialize)]
#[serde(default)]
pub struct NamedRef {
    pub id: String,
    pub name: String,
}

// ---------------------------------------------------------------- emails

/// One message. Give it a body (`subject` + `html`) or a published template
/// (`template_id` or `template`, plus `variables`).
#[derive(Debug, Clone, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SendEmailParams {
    /// A bare address on a verified domain: `hello@send.acme.io`. A display-name form is refused.
    pub from: String,
    /// ONE recipient. For several, use `batch().send()` with one item each.
    pub to: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub subject: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub html: Option<String>,
    /// The plain-text alternative.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub text: Option<String>,
    /// A published template's public id (`tpl_...`) or uuid.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub template_id: Option<String>,
    /// A published template's send name (its `key`, e.g. `"welcome"`). Use this or `template_id`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub template: Option<String>,
    /// Fills `{{tags}}` in a template, or in a hand-written `html` body.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub variables: Option<Object>,
    /// `"transactional"` (default) or `"marketing"`: marketing adds one-click unsubscribe and the postal-address footer.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub send_class: Option<String>,
    /// `Some(false)` leaves the open-tracking pixel out. Default on.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub track_opens: Option<bool>,
    /// `Some(false)` leaves links unwrapped. Default on.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub track_clicks: Option<bool>,
    /// Where replies go, sent as `Reply-To`: 1 to 10 bare addresses. Left out, replies go to `from`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reply_to: Option<Vec<String>>,
    /// Files sent with the message: at most 20, 10 MiB each and 25 MiB together,
    /// plus the sending provider's own limits. Not accepted on a batch item: the
    /// server fails that item with `attachments_not_batchable`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub attachments: Option<Vec<Attachment>>,
}

/// One file on `emails().send()` / `sandboxes().send()`.
///
/// ```no_run
/// # fn run() -> std::io::Result<()> {
/// let invoice = sendclipper::Attachment::new("invoice-42.pdf", std::fs::read("invoice-42.pdf")?).content_type("application/pdf");
/// # let _ = invoice;
/// # Ok(())
/// # }
/// ```
#[derive(Clone, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Attachment {
    /// The name the recipient sees, e.g. `invoice-42.pdf`.
    pub filename: String,
    /// The file's bytes. Sent as standard base64.
    #[serde(serialize_with = "serialize_base64")]
    pub content: Vec<u8>,
    /// e.g. `application/pdf`. Left out, the server guesses it from the filename.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub content_type: Option<String>,
}

impl Attachment {
    /// A file with its name and bytes, e.g. `Attachment::new("a.pdf", std::fs::read("a.pdf")?)`.
    pub fn new(filename: impl Into<String>, content: impl Into<Vec<u8>>) -> Attachment {
        Attachment {
            filename: filename.into(),
            content: content.into(),
            content_type: None,
        }
    }

    /// Set the MIME type instead of letting the server guess it from the filename.
    pub fn content_type(mut self, content_type: impl Into<String>) -> Attachment {
        self.content_type = Some(content_type.into());
        self
    }
}

/// Prints the size, not the bytes: a file can be megabytes long.
impl std::fmt::Debug for Attachment {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Attachment")
            .field("filename", &self.filename)
            .field("content", &format_args!("<{} bytes>", self.content.len()))
            .field("content_type", &self.content_type)
            .finish()
    }
}

fn serialize_base64<S: serde::Serializer>(bytes: &[u8], s: S) -> Result<S::Ok, S::Error> {
    use base64::Engine as _;
    s.serialize_str(&base64::engine::general_purpose::STANDARD.encode(bytes))
}

/// What `emails().send()` answers: the message was accepted (`queued`).
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct SentEmail {
    /// SendClipper's id for the message. Use it with `emails().get()`.
    pub id: String,
    pub message_id: String,
    pub ses_message_id: Option<String>,
    pub from: String,
    pub to: String,
    pub domain: String,
    pub subject: String,
    pub status: String,
    pub created_at: String,
    pub template_id: Option<String>,
}

/// Why one batch item was refused.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct BatchItemError {
    pub code: String,
    pub message: String,
    pub detail: Option<Value>,
}

/// One submitted item's outcome: `status` is `"accepted"` (`id` set) or `"failed"` (`error` set).
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct BatchResult {
    pub index: u32,
    pub status: String,
    pub id: Option<String>,
    pub message_id: Option<String>,
    pub error: Option<BatchItemError>,
}

/// Counts for a batch.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct BatchSummary {
    pub submitted: u32,
    pub accepted: u32,
    pub failed: u32,
}

/// One result per submitted item, in order.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct SendBatchResponse {
    pub data: Vec<BatchResult>,
    pub summary: BatchSummary,
}

/// Filters for the sent-mail log.
#[derive(Debug, Clone, Default)]
pub struct ListEmailsParams {
    pub search: Option<String>,
    pub status: Option<String>,
    /// Exact sending domain.
    pub domain: Option<String>,
    pub broadcast_id: Option<String>,
    /// ISO 8601 lower bound.
    pub from: Option<String>,
    /// ISO 8601 upper bound.
    pub to: Option<String>,
    pub page: Option<u32>,
    /// 1-100, default 50.
    pub page_size: Option<u32>,
}

/// One message in the log.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct EmailRow {
    pub id: String,
    pub message_id: String,
    pub from: String,
    pub to: String,
    pub domain: String,
    pub subject: String,
    pub status: String,
    pub ses_message_id: Option<String>,
    pub created_at: String,
}

/// One numbered page of the log.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct EmailList {
    pub items: Vec<EmailRow>,
    pub total: u64,
    pub page: u32,
    pub page_size: u32,
    /// A search counts only its newest 1,000 matches.
    pub total_capped: bool,
}

/// One step of a message's timeline.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct EmailEvent {
    pub id: String,
    #[serde(rename = "type")]
    pub kind: String,
    pub meta: Value,
    pub created_at: String,
}

/// One message with its body and timeline.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct EmailDetail {
    #[serde(flatten)]
    pub row: EmailRow,
    pub html: Option<String>,
    pub text: Option<String>,
    /// True once the retention window deleted the body.
    pub body_expired: bool,
    pub events: Vec<EmailEvent>,
    pub thread_id: Option<String>,
    pub send_class: Option<String>,
    pub track_opens: Option<bool>,
    pub track_clicks: Option<bool>,
}

/// Counts per status.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct EmailStats {
    pub total: u64,
    pub queued: u64,
    pub sent: u64,
    pub delivered: u64,
    pub opened: u64,
    pub clicked: u64,
    pub bounced: u64,
    pub complained: u64,
    pub failed: u64,
    pub body_retention_days: u32,
}

// --------------------------------------------------------------- domains

/// A sending (and optionally receiving) domain.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct Domain {
    pub id: String,
    pub name: String,
    /// `us-east-1`, `eu-west-1` or `us-west-2`.
    pub region: String,
    /// `verified`, `pending` or `failed`.
    pub status: String,
    pub spf_ok: bool,
    pub dkim_ok: bool,
    pub dmarc_ok: bool,
    pub records: Vec<DomainCheck>,
    pub sent: u64,
    pub check_count: u32,
    pub last_checked_at: Option<String>,
    pub created_at: String,
    pub last_error: Option<String>,
    pub receiving_enabled: bool,
    pub mx_ok: bool,
    pub receiving_verified: bool,
    pub receiving_error: Option<String>,
}

/// One SPF/DKIM/DMARC check.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct DomainCheck {
    pub name: String,
    pub ok: bool,
}

/// One DNS record to publish.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct DnsRecord {
    #[serde(rename = "type")]
    pub kind: String,
    pub purpose: String,
    pub host: String,
    pub value: String,
    pub copy_value: Option<String>,
    pub priority: Option<u32>,
    pub status: Option<String>,
}

/// A domain with the DNS records to publish.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct DomainRecords {
    pub domain: Domain,
    pub records: Vec<DnsRecord>,
}

/// Registers a domain.
#[derive(Debug, Clone, Default, Serialize)]
pub struct CreateDomainParams {
    /// A hostname such as `send.acme.com`. No scheme, no trailing slash.
    pub name: String,
    /// `us-east-1`, `eu-west-1` or `us-west-2`. Cannot change once the domain holds messages.
    pub region: String,
}

// ------------------------------------------------------------- templates

/// A template tag.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct TemplateTag {
    pub id: String,
    pub name: String,
    pub color: String,
    pub template_count: Option<u32>,
}

/// One card in the template list.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct TemplateSummary {
    pub id: String,
    pub public_id: String,
    pub name: String,
    /// `draft` or `published`.
    pub status: String,
    pub block_count: u32,
    pub updated_at: String,
    pub published_at: Option<String>,
    pub created_at: String,
    pub domains: Vec<NamedRef>,
    pub tags: Vec<TemplateTag>,
    pub key: Option<String>,
    pub project: Option<NamedRef>,
}

/// A template with its body.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct Template {
    #[serde(flatten)]
    pub summary: TemplateSummary,
    pub subject: String,
    pub preheader: String,
    /// Each block is a JSON object: `{"type": "text", "text": "Hi {{first_name|there}}"}`.
    pub blocks: Vec<Object>,
    pub html: String,
}

/// Creates or changes a template. Every field is optional; on an update only
/// the fields you set change.
#[derive(Debug, Clone, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TemplateParams {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub subject: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub preheader: Option<String>,
    /// See <https://sendclipper.com/docs> for the block types. Unknown fields are dropped.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub blocks: Option<Vec<Object>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub domain_ids: Option<Vec<String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub project_domain_id: Option<String>,
    /// The send name, unique in its project.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub key: Option<String>,
}

/// Filters for the template list.
#[derive(Debug, Clone, Default)]
pub struct ListTemplatesParams {
    /// A tag id.
    pub tag: Option<String>,
    /// A project (domain) id.
    pub project: Option<String>,
}

/// A render that sent nothing.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct TemplatePreview {
    pub subject: String,
    pub preheader: String,
    pub html: String,
    pub text: String,
    pub variables: Vec<PreviewVariable>,
    /// Links whose target a tag would make unsafe.
    pub unsafe_links: Vec<String>,
}

/// One tag a preview filled.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct PreviewVariable {
    pub name: String,
    pub fallback: Option<String>,
    pub value: String,
    pub provided: bool,
}

/// An uploaded image.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct TemplateAsset {
    pub id: String,
    pub public_id: String,
    /// Public: a mail client can load it.
    pub url: String,
    pub mime_type: String,
    pub bytes: u64,
    pub name: String,
    pub created_at: String,
}

// ------------------------------------------------------------- sandboxes

/// A sandbox: it traps mail, and nothing sent to it is delivered.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct Sandbox {
    pub id: String,
    pub name: String,
    pub slug: String,
    /// Mail sent to this address lands in the sandbox.
    pub inbox_address: String,
    pub inbound_live: bool,
    pub project_label: String,
    pub color: String,
    pub total: u64,
    pub unread: u64,
    pub message_cap: u32,
    pub retention_days: u32,
    pub last_trapped_at: Option<String>,
    pub created_at: String,
}

/// Creates or renames a sandbox.
#[derive(Debug, Clone, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SandboxParams {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub project_label: Option<String>,
    /// `#rrggbb`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub color: Option<String>,
}

/// Points a project at a sandbox.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct SandboxCredentials {
    pub host: String,
    pub port: u16,
    /// `required`, `optional` or `none`.
    pub tls: String,
    pub max_recipients: u32,
    pub username: String,
    pub password: String,
    pub max_message_bytes: u64,
    pub retention_days: u32,
    pub api_endpoint: String,
    pub inbox_id: String,
    pub inbox_address: String,
    pub inbound_domain: String,
    pub inbound_live: bool,
    /// The JSON token is hashed; only its hint can be read back.
    pub api_token_hint: String,
}

/// The new credentials. The token is shown once.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct ResetSandboxCredentials {
    pub username: String,
    pub password: String,
    pub api_token: String,
}

/// One trapped message in a list.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct SandboxMessageSummary {
    pub id: String,
    pub subject: String,
    pub from_address: String,
    pub from_name: String,
    pub to_addresses: Vec<String>,
    pub snippet: String,
    pub size_bytes: u64,
    pub read: bool,
    pub trapped_at: String,
}

/// One trapped message in full.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct SandboxMessage {
    #[serde(flatten)]
    pub summary: SandboxMessageSummary,
    pub cc_addresses: Vec<String>,
    /// The SMTP envelope, recorded apart from the headers (a Bcc recipient is only here).
    pub envelope_from: String,
    pub envelope_to: Vec<String>,
    pub body_html: Option<String>,
    /// `None` when the message carried no text/plain part.
    pub body_text: Option<String>,
    pub headers: Vec<MessageHeader>,
    pub transport: String,
    pub client_ip: Option<String>,
    pub has_raw: bool,
}

/// One header of a trapped message.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct MessageHeader {
    pub name: String,
    pub value: String,
}

/// Filters for a sandbox's messages.
#[derive(Debug, Clone, Default)]
pub struct ListSandboxMessagesParams {
    pub search: Option<String>,
    pub limit: Option<u32>,
}

/// What the sandbox JSON door answers.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct SandboxSendResult {
    pub id: String,
    pub duplicate: bool,
}

// -------------------------------------------------------------- webhooks

/// A URL SendClipper posts events to.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct WebhookEndpoint {
    pub id: String,
    pub url: String,
    /// `email.delivered`, `email.opened`, `email.clicked`, `email.bounced`, `email.complained`, `email.failed`.
    pub events: Vec<String>,
    pub active: bool,
    pub created_at: String,
    pub secret_masked: String,
    pub secret_rotated_at: Option<String>,
    pub previous_secret_expires_at: Option<String>,
    pub success_rate: Option<f64>,
    pub success_rate_window_days: u32,
    pub delivery_count: u64,
    pub delivered_count: u64,
    /// The full `whsec_...` signing secret: only on create and rotate. Store it.
    pub secret: Option<String>,
}

/// Registers or changes an endpoint.
#[derive(Debug, Clone, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct WebhookParams {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub url: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub events: Option<Vec<String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub active: Option<bool>,
    /// Accept a plain `http://` URL. Off by default.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub allow_insecure_http: Option<bool>,
}

/// Filters for delivery attempts.
#[derive(Debug, Clone, Default)]
pub struct ListWebhookDeliveriesParams {
    pub endpoint_id: Option<String>,
    pub final_only: Option<bool>,
    pub limit: Option<u32>,
}

/// Filters for undelivered events.
#[derive(Debug, Clone, Default)]
pub struct ListWebhookDispatchesParams {
    /// Default `dead_lettered`; `retrying` = pending or delivering; `all`.
    pub state: Option<String>,
    pub endpoint_id: Option<String>,
    pub cursor: Option<String>,
    /// 1-100.
    pub limit: Option<u32>,
}

/// One event's delivery to one endpoint.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct WebhookDispatch {
    pub id: String,
    pub endpoint_id: String,
    pub endpoint_url: String,
    /// The `id` the receiver saw in the body.
    pub event_id: String,
    pub event: String,
    /// `pending`, `delivering`, `delivered` or `dead_lettered`.
    pub state: String,
    pub attempts: u32,
    pub last_status_code: Option<u16>,
    pub last_error: Option<String>,
    pub last_attempt_error: Option<String>,
    pub first_attempt_at: Option<String>,
    pub last_attempt_at: Option<String>,
    pub next_attempt_at: Option<String>,
    pub created_at: String,
    pub replayed_at: Option<String>,
    pub replay_count: u32,
}

/// One cursor page of dispatches.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct WebhookDispatchPage {
    pub data: Vec<WebhookDispatch>,
    pub page: CursorInfo,
}

/// What a bulk replay queued.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct ReplayAllResult {
    pub queued: u32,
    pub remaining: u32,
    pub capped: bool,
    pub max: u32,
}

/// A webhook delivery's body.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct WebhookEvent {
    /// Stable for one (email, event): deduplicate on it.
    pub id: String,
    pub event: String,
    pub created_at: String,
    /// `emailId`, `messageId`, `to`, `domain`, `subject` and the event's own fields.
    pub data: Object,
}

// ------------------------------------------------- contacts & audiences

/// One person in an audience.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct Contact {
    pub id: String,
    pub name: String,
    pub email: String,
    /// `subscribed`, `unsubscribed` or `bounced`.
    pub status: String,
    pub audience_id: Option<String>,
    pub audience_name: Option<String>,
    pub audience_color: Option<String>,
    pub created_at: String,
}

/// One numbered page of contacts.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct ContactList {
    pub items: Vec<Contact>,
    pub total: u64,
    pub page: u32,
    pub page_size: u32,
    pub total_pages: u32,
    pub total_all: u64,
}

/// Adds a contact.
#[derive(Debug, Clone, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CreateContactParams {
    pub name: String,
    pub email: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub audience_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub status: Option<String>,
}

/// Filters for contacts.
#[derive(Debug, Clone, Default)]
pub struct ListContactsParams {
    pub audience_id: Option<String>,
    pub status: Option<String>,
    pub page: Option<u32>,
    /// 1-200, default 25.
    pub page_size: Option<u32>,
}

/// A CSV or vCard import.
#[derive(Debug, Clone, Default)]
pub struct ImportContactsParams {
    /// The CSV or vCard contents.
    pub file: Vec<u8>,
    /// `contacts.csv` / `contacts.vcf`: the extension decides the parser. Default `contacts.csv`.
    pub filename: Option<String>,
    pub audience_id: Option<String>,
    pub new_audience: Option<String>,
    pub create_audiences: Option<bool>,
    /// `skip` or `update`.
    pub on_duplicate: Option<String>,
    /// The column mapping as `import_preview()` returns it.
    pub mapping: Option<Object>,
}

/// A list of contacts.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct Audience {
    pub id: String,
    pub name: String,
    pub color: String,
    pub count: u64,
}

/// Every audience.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct AudienceList {
    pub audiences: Vec<Audience>,
    pub total_contacts: u64,
}

// ---------------------------------------------------------- suppressions

/// Stops mail to one address.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct Suppression {
    pub id: String,
    pub email: String,
    /// `hard_bounce`, `soft_bounce`, `complaint`, `unsubscribe`, `manual` or `provider`.
    pub reason: String,
    pub reason_label: String,
    pub reason_detail: Option<String>,
    pub diagnostic_code: Option<String>,
    /// `marketing` for an unsubscribe, `all` for everything else.
    pub scope: String,
    pub source_email_id: Option<String>,
    pub source_message_id: Option<String>,
    pub created_at: String,
}

/// One cursor page of the list.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct SuppressionList {
    pub data: Vec<Suppression>,
    pub page: CursorInfo,
    pub counts: SuppressionCounts,
}

/// Counts on the list.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct SuppressionCounts {
    pub total: u64,
    pub by_reason: std::collections::BTreeMap<String, u64>,
}

/// Filters for the list.
#[derive(Debug, Clone, Default)]
pub struct ListSuppressionsParams {
    /// Address search.
    pub q: Option<String>,
    pub reason: Option<String>,
    pub cursor: Option<String>,
    /// 1-100.
    pub limit: Option<u32>,
}

/// `created` is false when the address was already suppressed; `row` is the entry either way.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct SuppressionCreated {
    pub created: bool,
    pub row: Option<Suppression>,
}

// ------------------------------------------------------------- mailboxes

/// One address on a receiving domain.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct Mailbox {
    pub id: String,
    pub address: String,
    pub display_name: String,
    pub domain: String,
    pub domain_id: String,
    pub catch_all: bool,
    pub archived: bool,
    pub can_receive: bool,
    pub can_send: bool,
    pub unread: u64,
    pub total: u64,
}

/// Creates or changes an address.
#[derive(Debug, Clone, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MailboxParams {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub domain_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub local_part: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub display_name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub color: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub catch_all: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub archived: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub notify_owner: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub embed_origins: Option<Vec<String>>,
}

/// Filters for an address's mail.
#[derive(Debug, Clone, Default)]
pub struct ListMailboxMessagesParams {
    /// `all`, `inbox`, `starred`, `sent`, `drafts`, `archive`, `spam` or `trash`.
    pub folder: Option<String>,
    pub search: Option<String>,
    pub label: Option<String>,
    /// 1-200.
    pub limit: Option<u32>,
    pub cursor: Option<String>,
}

/// One message in a mailbox list.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct MailboxMessageRow {
    pub id: String,
    pub folder: String,
    pub from: String,
    pub from_name: String,
    pub to: String,
    pub cc: Option<String>,
    pub bcc: Option<String>,
    pub subject: String,
    pub snippet: String,
    pub label: Option<String>,
    pub unread: bool,
    pub starred: bool,
    pub received_at: String,
    pub mailbox_id: Option<String>,
    pub thread_id: Option<String>,
}

/// One cursor page of a mailbox.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct MailboxMessageList {
    pub items: Vec<MailboxMessageRow>,
    pub total: u64,
    pub unread: u64,
    pub total_capped: bool,
    pub page: CursorInfo,
}

/// One message in full, with its conversation.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct MailboxMessage {
    #[serde(flatten)]
    pub row: MailboxMessageRow,
    pub body_text: Option<String>,
    pub body_html: Option<String>,
    pub body_expired_at: Option<String>,
    /// The conversation, as the API sends it.
    pub thread: Option<Object>,
    /// Download one with `mailboxes().messages().attachment()`.
    pub attachments: Vec<MailboxAttachment>,
    /// SPF/DKIM/DMARC/spam verdicts for received mail; `None` for your own sends.
    pub authentication: Option<Object>,
}

/// One attachment of a received message.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct MailboxAttachment {
    pub id: String,
    pub filename: String,
    pub content_type: String,
    pub size_bytes: u64,
}

/// What a download returns.
#[derive(Debug, Clone, Default)]
pub struct DownloadedFile {
    pub content: Vec<u8>,
    pub content_type: Option<String>,
    pub filename: Option<String>,
}

// ------------------------------------------------------------- workspace

/// The workspace an API key belongs to.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct Workspace {
    pub id: String,
    pub name: String,
    pub public_id: String,
    pub created_at: String,
    /// `{ at, reason, message }` while sending is paused.
    pub sending_paused: Option<Object>,
    pub postal_address: Option<String>,
}

/// One of the workspace's keys, masked.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct ApiKey {
    pub id: String,
    pub name: String,
    pub masked: String,
    pub prefix: String,
    pub last4: String,
    /// `full`, `sending` or `read`.
    pub permission: String,
    pub domain_id: Option<String>,
    pub domain: Option<String>,
    pub created_at: String,
    pub last_used_at: Option<String>,
}
