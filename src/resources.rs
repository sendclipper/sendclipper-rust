//! One handle per API resource. Reach them through [`crate::Client`]:
//! `client.emails()`, `client.domains()`, ...

use std::time::{Duration, SystemTime};

use base64::Engine as _;
use futures_util::stream::{self, Stream};
use serde_json::{json, Value};

use crate::client::{enc, Body, Client, Q};
use crate::error::{Error, Result};
use crate::types::*;
use crate::webhook::{verify_webhook, Headers, VerifyOptions};

fn seg(s: &str) -> String {
    enc(s)
}

// ---------------------------------------------------------------- emails

/// Sends mail and reads the sent-mail log.
pub struct Emails<'a> {
    pub(crate) c: &'a Client,
}

impl<'a> Emails<'a> {
    /// Send one message. Resolves once SendClipper accepted it (status `queued`);
    /// delivery is reported later through [`Emails::get`] and webhooks.
    ///
    /// Every call carries an Idempotency-Key, so the SDK's own retries never
    /// send twice. Use [`Client::with_idempotency_key`] to make YOUR retries safe too.
    pub async fn send(&self, params: &SendEmailParams) -> Result<SentEmail> {
        self.c.send_json("POST", "/emails", params).await
    }

    /// Same as `client.batch().send()`.
    pub async fn send_batch(&self, emails: &[SendEmailParams]) -> Result<SendBatchResponse> {
        Batch { c: self.c }.send(emails).await
    }

    /// One message with its body and event timeline.
    pub async fn get(&self, id: &str) -> Result<EmailDetail> {
        self.c.get(&format!("/emails/{}", seg(id)), &[]).await
    }

    /// One page of the sent-mail log, newest first.
    pub async fn list(&self, params: &ListEmailsParams) -> Result<EmailList> {
        let q = Q::new()
            .s("search", &params.search)
            .s("status", &params.status)
            .s("domain", &params.domain)
            .s("broadcastId", &params.broadcast_id)
            .s("from", &params.from)
            .s("to", &params.to)
            .n("page", params.page)
            .n("pageSize", params.page_size);
        self.c.get("/emails", &q.0).await
    }

    /// Every message matching `params`, page after page. A failed page is
    /// yielded once as an `Err`, then the stream ends.
    pub fn list_all(&self, params: ListEmailsParams) -> impl Stream<Item = Result<EmailRow>> + 'a {
        let c = self.c;
        let size = params.page_size.unwrap_or(100);
        numbered(move |page| {
            let mut p = params.clone();
            p.page = Some(page);
            p.page_size = Some(size);
            async move {
                let res = Emails { c }.list(&p).await?;
                let done = (res.items.len() as u32) < size || u64::from(page) * u64::from(size) >= res.total;
                Ok((res.items, done))
            }
        })
    }

    /// Counts per status.
    pub async fn stats(&self) -> Result<EmailStats> {
        self.c.get("/emails/stats/summary", &[]).await
    }
}

/// Sends up to 500 messages in one request.
pub struct Batch<'a> {
    pub(crate) c: &'a Client,
}

impl Batch<'_> {
    /// Up to 500 messages (one recipient each). Every item is accepted or
    /// refused on its own; a refused item costs its siblings nothing.
    pub async fn send(&self, emails: &[SendEmailParams]) -> Result<SendBatchResponse> {
        self.c.send_json("POST", "/emails/batch", &json!({ "emails": emails })).await
    }
}

// --------------------------------------------------------------- domains

/// Manages sending and receiving domains.
pub struct Domains<'a> {
    pub(crate) c: &'a Client,
}

impl Domains<'_> {
    pub async fn list(&self) -> Result<Vec<Domain>> {
        self.c.get("/domains", &[]).await
    }

    /// Register a sending domain. Next: [`Domains::records`], publish them, then [`Domains::verify`].
    pub async fn create(&self, params: &CreateDomainParams) -> Result<Domain> {
        self.c.send_json("POST", "/domains", params).await
    }

    /// The DNS records to publish, each with its current status.
    pub async fn records(&self, id: &str) -> Result<DomainRecords> {
        self.c.get(&format!("/domains/{}/records", seg(id)), &[]).await
    }

    /// Check DNS now instead of waiting for the background check.
    pub async fn verify(&self, id: &str) -> Result<Domain> {
        self.c.send_empty("POST", &format!("/domains/{}/verify", seg(id))).await
    }

    /// Start receiving mail on this domain. Adds receiving records; never blocks sending.
    pub async fn enable_receiving(&self, id: &str) -> Result<Domain> {
        self.c.send_empty("POST", &format!("/domains/{}/receiving", seg(id))).await
    }

    pub async fn disable_receiving(&self, id: &str) -> Result<Domain> {
        self.c.send_empty("DELETE", &format!("/domains/{}/receiving", seg(id))).await
    }

    /// Move a domain to another region. Refused (`region_immutable`) once it holds messages.
    pub async fn set_region(&self, id: &str, region: &str) -> Result<Domain> {
        self.c
            .send_json("PATCH", &format!("/domains/{}/region", seg(id)), &json!({ "region": region }))
            .await
    }

    /// What deleting the domain would take with it.
    pub async fn deletion_preview(&self, id: &str) -> Result<Object> {
        self.c.get(&format!("/domains/{}/deletion", seg(id)), &[]).await
    }

    pub async fn remove(&self, id: &str) -> Result<()> {
        self.c
            .send_empty::<Value>("DELETE", &format!("/domains/{}", seg(id)))
            .await
            .map(drop)
    }
}

// ------------------------------------------------------------- templates

/// Manages email templates.
pub struct Templates<'a> {
    pub(crate) c: &'a Client,
}

impl<'a> Templates<'a> {
    pub fn tags(&self) -> TemplateTags<'a> {
        TemplateTags { c: self.c }
    }

    pub fn assets(&self) -> TemplateAssets<'a> {
        TemplateAssets { c: self.c }
    }

    pub async fn list(&self, params: &ListTemplatesParams) -> Result<Vec<TemplateSummary>> {
        self.c
            .get("/templates", &Q::new().s("tag", &params.tag).s("project", &params.project).0)
            .await
    }

    /// By uuid or public id (`tpl_...`).
    pub async fn get(&self, id: &str) -> Result<Template> {
        self.c.get(&format!("/templates/{}", seg(id)), &[]).await
    }

    /// The rendered HTML, tags left in place.
    pub async fn html(&self, id: &str) -> Result<String> {
        self.c.text(&format!("/templates/{}/html", seg(id)), &[]).await
    }

    /// A new draft. Publish it before sending with it.
    pub async fn create(&self, params: &TemplateParams) -> Result<Template> {
        self.c.send_json("POST", "/templates", params).await
    }

    /// Changes the fields set in `params`.
    pub async fn update(&self, id: &str, params: &TemplateParams) -> Result<Template> {
        self.c.send_json("PATCH", &format!("/templates/{}", seg(id)), params).await
    }

    pub async fn remove(&self, id: &str) -> Result<()> {
        self.c
            .send_empty::<Value>("DELETE", &format!("/templates/{}", seg(id)))
            .await
            .map(drop)
    }

    pub async fn duplicate(&self, id: &str) -> Result<Template> {
        self.c.send_empty("POST", &format!("/templates/{}/duplicate", seg(id))).await
    }

    pub async fn publish(&self, id: &str) -> Result<Template> {
        self.c.send_empty("POST", &format!("/templates/{}/publish", seg(id))).await
    }

    pub async fn unpublish(&self, id: &str) -> Result<Template> {
        self.c.send_empty("POST", &format!("/templates/{}/unpublish", seg(id))).await
    }

    /// Render with `variables` without sending anything.
    pub async fn preview(&self, id: &str, variables: Object) -> Result<TemplatePreview> {
        self.c
            .send_json(
                "POST",
                &format!("/templates/{}/preview", seg(id)),
                &json!({ "variables": variables }),
            )
            .await
    }

    /// Replace the template's tags with these tag ids.
    pub async fn set_tags(&self, id: &str, tag_ids: &[&str]) -> Result<Vec<TemplateTag>> {
        #[derive(serde::Deserialize)]
        struct Tags {
            tags: Vec<TemplateTag>,
        }
        let res: Tags = self
            .c
            .send_json("PUT", &format!("/templates/{}/tags", seg(id)), &json!({ "tagIds": tag_ids }))
            .await?;
        Ok(res.tags)
    }

    /// The projects (domains) templates can belong to.
    pub async fn projects(&self) -> Result<Vec<NamedRef>> {
        self.c.get("/templates/projects", &[]).await
    }
}

/// Manages template tags.
pub struct TemplateTags<'a> {
    pub(crate) c: &'a Client,
}

impl TemplateTags<'_> {
    /// Every tag, each with its `template_count`.
    pub async fn list(&self) -> Result<Vec<TemplateTag>> {
        self.c.get("/templates/tags", &[]).await
    }

    /// `color`: gray, red, orange, amber, green, teal, blue, purple or pink.
    pub async fn create(&self, name: &str, color: &str) -> Result<TemplateTag> {
        self.c
            .send_json("POST", "/templates/tags", &json!({ "name": name, "color": color }))
            .await
    }

    /// Rename or recolour; `None` leaves that field alone.
    pub async fn update(&self, id: &str, name: Option<&str>, color: Option<&str>) -> Result<TemplateTag> {
        let mut body = serde_json::Map::new();
        if let Some(n) = name {
            body.insert("name".into(), n.into());
        }
        if let Some(c) = color {
            body.insert("color".into(), c.into());
        }
        self.c.send_json("PATCH", &format!("/templates/tags/{}", seg(id)), &body).await
    }

    /// Deletes a tag and returns how many templates it was taken off.
    pub async fn remove(&self, id: &str) -> Result<u32> {
        let res: Value = self.c.send_empty("DELETE", &format!("/templates/tags/{}", seg(id))).await?;
        Ok(res.get("templateCount").and_then(Value::as_u64).unwrap_or(0) as u32)
    }
}

/// Manages images templates can show.
pub struct TemplateAssets<'a> {
    pub(crate) c: &'a Client,
}

impl TemplateAssets<'_> {
    pub async fn list(&self) -> Result<Vec<TemplateAsset>> {
        self.c.get("/templates/assets", &[]).await
    }

    /// Upload an image and get its public URL. `data` is the file's bytes
    /// (base64-encoded for you); `mime_type` is image/png, image/jpeg,
    /// image/gif or image/webp.
    pub async fn upload(&self, data: &[u8], mime_type: &str, name: Option<&str>) -> Result<TemplateAsset> {
        let mut body = json!({ "mimeType": mime_type, "base64": base64::engine::general_purpose::STANDARD.encode(data) });
        if let Some(n) = name.filter(|n| !n.is_empty()) {
            body["name"] = n.into();
        }
        self.c.send_json("POST", "/templates/assets", &body).await
    }
}

// ------------------------------------------------------------- sandboxes

/// Manages sandboxes: inboxes that trap mail while you develop.
pub struct Sandboxes<'a> {
    pub(crate) c: &'a Client,
}

impl<'a> Sandboxes<'a> {
    pub fn messages(&self) -> SandboxMessages<'a> {
        SandboxMessages { c: self.c }
    }

    pub async fn list(&self) -> Result<Vec<Sandbox>> {
        self.c.get("/sandboxes", &[]).await
    }

    pub async fn get(&self, id: &str) -> Result<Sandbox> {
        self.c.get(&format!("/sandboxes/{}", seg(id)), &[]).await
    }

    /// `name` is required.
    pub async fn create(&self, params: &SandboxParams) -> Result<Sandbox> {
        self.c.send_json("POST", "/sandboxes", params).await
    }

    /// Renaming never changes the address or the credentials.
    pub async fn update(&self, id: &str, params: &SandboxParams) -> Result<Sandbox> {
        self.c.send_json("PATCH", &format!("/sandboxes/{}", seg(id)), params).await
    }

    /// Deletes the sandbox and every message in it; returns how many messages went.
    pub async fn remove(&self, id: &str) -> Result<u64> {
        let res: Value = self.c.send_empty("DELETE", &format!("/sandboxes/{}", seg(id))).await?;
        Ok(res.get("deletedMessages").and_then(Value::as_u64).unwrap_or(0))
    }

    /// SMTP host, port, username and password to point a project at.
    pub async fn credentials(&self, id: &str) -> Result<SandboxCredentials> {
        self.c.get(&format!("/sandboxes/{}/credentials", seg(id)), &[]).await
    }

    /// Replace BOTH the SMTP password and the `sc_sbx_` token. The token is shown only here.
    pub async fn reset_credentials(&self, id: &str) -> Result<ResetSandboxCredentials> {
        self.c.send_empty("POST", &format!("/sandboxes/{}/credentials", seg(id))).await
    }

    /// Send to a sandbox through its JSON door, with the sandbox's own
    /// `sc_sbx_...` token. Same fields as `emails().send()`; nothing is delivered.
    pub async fn send(&self, sandbox_token: &str, params: &SendEmailParams) -> Result<SandboxSendResult> {
        self.c.with_bearer(sandbox_token).send_json("POST", "/sandbox/emails", params).await
    }
}

/// Reads what a sandbox trapped.
pub struct SandboxMessages<'a> {
    pub(crate) c: &'a Client,
}

impl SandboxMessages<'_> {
    pub async fn list(&self, sandbox_id: &str, params: &ListSandboxMessagesParams) -> Result<Vec<SandboxMessageSummary>> {
        let q = Q::new().s("search", &params.search).n("limit", params.limit);
        self.c.get(&format!("/sandboxes/{}/messages", seg(sandbox_id)), &q.0).await
    }

    pub async fn get(&self, sandbox_id: &str, message_id: &str) -> Result<SandboxMessage> {
        self.c
            .get(&format!("/sandboxes/{}/messages/{}", seg(sandbox_id), seg(message_id)), &[])
            .await
    }

    /// The message's raw MIME source.
    pub async fn raw(&self, sandbox_id: &str, message_id: &str) -> Result<String> {
        self.c
            .text(&format!("/sandboxes/{}/messages/{}/raw", seg(sandbox_id), seg(message_id)), &[])
            .await
    }

    pub async fn remove(&self, sandbox_id: &str, message_id: &str) -> Result<()> {
        self.c
            .send_empty::<Value>("DELETE", &format!("/sandboxes/{}/messages/{}", seg(sandbox_id), seg(message_id)))
            .await
            .map(drop)
    }

    /// Empty the sandbox; returns how many messages went.
    pub async fn clear(&self, sandbox_id: &str) -> Result<u64> {
        let res: Value = self
            .c
            .send_empty("DELETE", &format!("/sandboxes/{}/messages", seg(sandbox_id)))
            .await?;
        Ok(res.get("deleted").and_then(Value::as_u64).unwrap_or(0))
    }

    /// Mark every message read; returns how many changed.
    pub async fn mark_all_read(&self, sandbox_id: &str) -> Result<u64> {
        let res: Value = self
            .c
            .send_empty("POST", &format!("/sandboxes/{}/read-all", seg(sandbox_id)))
            .await?;
        Ok(res.get("updated").and_then(Value::as_u64).unwrap_or(0))
    }

    /// Poll every `interval` until a message arrives that `matches` accepts,
    /// then return it in full. Handy in end-to-end tests. After `timeout` the
    /// error's name is `timeout`.
    pub async fn wait_for<F>(&self, sandbox_id: &str, matches: F, timeout: Duration, interval: Duration) -> Result<SandboxMessage>
    where
        F: Fn(&SandboxMessageSummary) -> bool,
    {
        let deadline = tokio::time::Instant::now() + timeout;
        loop {
            let page = self
                .list(
                    sandbox_id,
                    &ListSandboxMessagesParams {
                        limit: Some(50),
                        ..Default::default()
                    },
                )
                .await?;
            if let Some(m) = page.iter().find(|m| matches(m)) {
                return self.get(sandbox_id, &m.id).await;
            }
            if tokio::time::Instant::now() >= deadline {
                return Err(Error::new(
                    "timeout",
                    format!("No matching message reached sandbox {sandbox_id} within {timeout:?}."),
                    None,
                ));
            }
            tokio::time::sleep(interval).await;
        }
    }

    /// [`SandboxMessages::wait_for`] for any message trapped after the call started.
    pub async fn wait_for_next(&self, sandbox_id: &str, timeout: Duration) -> Result<SandboxMessage> {
        let started = SystemTime::now() - Duration::from_secs(1);
        let started = httpdate_like(started);
        self.wait_for(
            sandbox_id,
            move |m| m.trapped_at.as_str() >= started.as_str(),
            timeout,
            Duration::from_secs(1),
        )
        .await
    }
}

/// An ISO 8601 UTC timestamp (`2026-10-16T10:00:00.000Z`) that compares
/// lexically with the API's own.
fn httpdate_like(t: SystemTime) -> String {
    let secs = t.duration_since(SystemTime::UNIX_EPOCH).map(|d| d.as_secs() as i64).unwrap_or(0);
    let (days, rem) = (secs.div_euclid(86_400), secs.rem_euclid(86_400));
    // Civil-from-days (Howard Hinnant).
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = if m <= 2 { y + 1 } else { y };
    format!("{y:04}-{m:02}-{d:02}T{:02}:{:02}:{:02}.000Z", rem / 3600, rem % 3600 / 60, rem % 60)
}

// -------------------------------------------------------------- webhooks

/// Manages webhook endpoints.
pub struct Webhooks<'a> {
    pub(crate) c: &'a Client,
}

impl<'a> Webhooks<'a> {
    pub fn dispatches(&self) -> WebhookDispatches<'a> {
        WebhookDispatches { c: self.c }
    }

    pub async fn list(&self) -> Result<Vec<WebhookEndpoint>> {
        self.c.get("/webhook-endpoints", &[]).await
    }

    /// `url` and `events` are required. The response carries the signing `secret` once: store it.
    pub async fn create(&self, params: &WebhookParams) -> Result<WebhookEndpoint> {
        self.c.send_json("POST", "/webhook-endpoints", params).await
    }

    pub async fn update(&self, id: &str, params: &WebhookParams) -> Result<WebhookEndpoint> {
        self.c.send_json("PATCH", &format!("/webhook-endpoints/{}", seg(id)), params).await
    }

    pub async fn remove(&self, id: &str) -> Result<()> {
        self.c
            .send_empty::<Value>("DELETE", &format!("/webhook-endpoints/{}", seg(id)))
            .await
            .map(drop)
    }

    /// A new secret. The old one keeps signing alongside it for the grace window.
    pub async fn rotate_secret(&self, id: &str) -> Result<WebhookEndpoint> {
        self.c
            .send_empty("POST", &format!("/webhook-endpoints/{}/rotate-secret", seg(id)))
            .await
    }

    /// Recent delivery attempts.
    pub async fn deliveries(&self, params: &ListWebhookDeliveriesParams) -> Result<Vec<Object>> {
        let q = Q::new()
            .s("endpointId", &params.endpoint_id)
            .b("finalOnly", params.final_only)
            .n("limit", params.limit);
        self.c.get("/webhook-deliveries", &q.0).await
    }

    /// Same as [`crate::verify_webhook`]. Makes no request.
    pub fn verify<'h>(
        &self,
        payload: &[u8],
        headers: impl Into<Headers<'h>>,
        secret: &str,
    ) -> std::result::Result<WebhookEvent, crate::WebhookVerificationError> {
        verify_webhook(payload, headers, secret, VerifyOptions::default())
    }
}

/// Reads and replays undelivered events.
pub struct WebhookDispatches<'a> {
    pub(crate) c: &'a Client,
}

impl<'a> WebhookDispatches<'a> {
    /// Undelivered events: dead letters by default, or `state: "retrying"`.
    pub async fn list(&self, params: &ListWebhookDispatchesParams) -> Result<WebhookDispatchPage> {
        let q = Q::new()
            .s("state", &params.state)
            .s("endpointId", &params.endpoint_id)
            .s("cursor", &params.cursor)
            .n("limit", params.limit);
        self.c.get("/webhook-dispatches", &q.0).await
    }

    /// Every dispatch matching `params`, following the cursor.
    pub fn list_all(&self, params: ListWebhookDispatchesParams) -> impl Stream<Item = Result<WebhookDispatch>> + 'a {
        let client = self.c;
        cursor(move |c| {
            let mut p = params.clone();
            p.cursor = c;
            async move {
                let res = WebhookDispatches { c: client }.list(&p).await?;
                Ok((res.data, res.page))
            }
        })
    }

    pub async fn get(&self, id: &str) -> Result<WebhookDispatch> {
        self.c.get(&format!("/webhook-dispatches/{}", seg(id)), &[]).await
    }

    /// One more immediate attempt for a dead-lettered event.
    pub async fn replay(&self, id: &str) -> Result<WebhookDispatch> {
        self.c.send_empty("POST", &format!("/webhook-dispatches/{}/replay", seg(id))).await
    }

    /// Replay every dead letter (of one endpoint, when given), up to the server's cap.
    pub async fn replay_all(&self, endpoint_id: Option<&str>) -> Result<ReplayAllResult> {
        let body = match endpoint_id {
            Some(id) => json!({ "endpointId": id }),
            None => json!({}),
        };
        self.c.send_json("POST", "/webhook-dispatches/replay-all", &body).await
    }
}

// ------------------------------------------------- contacts & audiences

/// Manages contacts.
pub struct Contacts<'a> {
    pub(crate) c: &'a Client,
}

impl<'a> Contacts<'a> {
    pub async fn list(&self, params: &ListContactsParams) -> Result<ContactList> {
        let q = Q::new()
            .s("audienceId", &params.audience_id)
            .s("status", &params.status)
            .n("page", params.page)
            .n("pageSize", params.page_size);
        self.c.get("/contacts", &q.0).await
    }

    /// Every contact, page after page.
    pub fn list_all(&self, params: ListContactsParams) -> impl Stream<Item = Result<Contact>> + 'a {
        let c = self.c;
        let size = params.page_size.unwrap_or(200);
        numbered(move |page| {
            let mut p = params.clone();
            p.page = Some(page);
            p.page_size = Some(size);
            async move {
                let res = Contacts { c }.list(&p).await?;
                let done = page >= res.total_pages || res.items.is_empty();
                Ok((res.items, done))
            }
        })
    }

    pub async fn create(&self, params: &CreateContactParams) -> Result<Contact> {
        self.c.send_json("POST", "/contacts", params).await
    }

    /// Import a CSV or vCard file. Importing never suppresses anyone.
    pub async fn import(&self, params: &ImportContactsParams) -> Result<Object> {
        let (body, ctype) = import_form(params, true);
        self.c.json("POST", "/contacts/import", &[], Body::Form(body, ctype)).await
    }

    /// What an import would do, without importing.
    pub async fn import_preview(&self, params: &ImportContactsParams) -> Result<Object> {
        let (body, ctype) = import_form(params, false);
        self.c.json("POST", "/contacts/import/preview", &[], Body::Form(body, ctype)).await
    }

    /// Every contact as CSV (re-importable) or vCard 4.0 (`format = "vcf"`).
    pub async fn export(&self, format: Option<&str>, audience: Option<&str>) -> Result<String> {
        let q = Q::new()
            .s("format", &format.map(str::to_owned))
            .s("audience", &audience.map(str::to_owned));
        self.c.text("/contacts/export", &q.0).await
    }
}

fn import_form(p: &ImportContactsParams, with_options: bool) -> (Vec<u8>, String) {
    let boundary = format!("----sendclipper{}", uuid::Uuid::new_v4().simple());
    let quote = |s: &str| s.replace('\\', "\\\\").replace('"', "\\\"").replace(['\r', '\n'], " ");
    let mut out: Vec<u8> = Vec::new();
    let mut field = |name: &str, value: &str| {
        out.extend_from_slice(
            format!(
                "--{boundary}\r\nContent-Disposition: form-data; name=\"{}\"\r\n\r\n{value}\r\n",
                quote(name)
            )
            .as_bytes(),
        );
    };
    if with_options {
        if let Some(v) = &p.audience_id {
            field("audienceId", v);
        }
        if let Some(v) = &p.new_audience {
            field("newAudience", v);
        }
        if let Some(v) = p.create_audiences {
            field("createAudiences", if v { "true" } else { "false" });
        }
        if let Some(v) = &p.on_duplicate {
            field("onDuplicate", v);
        }
        if let Some(v) = &p.mapping {
            field("mapping", &Value::Object(v.clone()).to_string());
        }
    }
    let name = p.filename.clone().unwrap_or_else(|| "contacts.csv".into());
    let ctype = if name.to_ascii_lowercase().ends_with(".vcf") {
        "text/vcard"
    } else {
        "text/csv"
    };
    out.extend_from_slice(
        format!(
            "--{boundary}\r\nContent-Disposition: form-data; name=\"file\"; filename=\"{}\"\r\nContent-Type: {ctype}\r\n\r\n",
            quote(&name)
        )
        .as_bytes(),
    );
    out.extend_from_slice(&p.file);
    out.extend_from_slice(format!("\r\n--{boundary}--\r\n").as_bytes());
    (out, format!("multipart/form-data; boundary={boundary}"))
}

/// Lists audiences.
pub struct Audiences<'a> {
    pub(crate) c: &'a Client,
}

impl Audiences<'_> {
    pub async fn list(&self) -> Result<AudienceList> {
        self.c.get("/audiences", &[]).await
    }
}

/// Manages the suppression list.
pub struct Suppressions<'a> {
    pub(crate) c: &'a Client,
}

impl<'a> Suppressions<'a> {
    pub async fn list(&self, params: &ListSuppressionsParams) -> Result<SuppressionList> {
        let q = Q::new()
            .s("q", &params.q)
            .s("reason", &params.reason)
            .s("cursor", &params.cursor)
            .n("limit", params.limit);
        self.c.get("/suppressions", &q.0).await
    }

    /// Every suppression matching `params`, following the cursor.
    pub fn list_all(&self, params: ListSuppressionsParams) -> impl Stream<Item = Result<Suppression>> + 'a {
        let client = self.c;
        cursor(move |c| {
            let mut p = params.clone();
            p.cursor = c;
            async move {
                let res = Suppressions { c: client }.list(&p).await?;
                Ok((res.data, res.page))
            }
        })
    }

    /// Stop all mail to an address (reason `manual`).
    pub async fn create(&self, email: &str, note: Option<&str>) -> Result<SuppressionCreated> {
        let mut body = json!({ "email": email });
        if let Some(n) = note {
            body["note"] = n.into();
        }
        self.c.send_json("POST", "/suppressions", &body).await
    }

    /// Lift a suppression. `reason` (10+ characters) is recorded. Complaint and
    /// provider suppressions cannot be removed (`suppression_not_removable`).
    pub async fn remove(&self, id: &str, reason: &str) -> Result<()> {
        self.c
            .send_json::<Value, _>("DELETE", &format!("/suppressions/{}", seg(id)), &json!({ "reason": reason }))
            .await
            .map(drop)
    }

    /// The list as CSV.
    pub async fn export(&self, q: Option<&str>, reason: Option<&str>) -> Result<String> {
        let query = Q::new().s("q", &q.map(str::to_owned)).s("reason", &reason.map(str::to_owned));
        self.c.text("/suppressions/export", &query.0).await
    }
}

// ------------------------------------------------------------- mailboxes

/// Manages addresses on receiving domains.
pub struct Mailboxes<'a> {
    pub(crate) c: &'a Client,
}

impl<'a> Mailboxes<'a> {
    pub fn messages(&self) -> MailboxMessages<'a> {
        MailboxMessages { c: self.c }
    }

    /// `include_archived` lists archived addresses too.
    pub async fn list(&self, include_archived: bool) -> Result<Vec<Mailbox>> {
        let q = if include_archived {
            vec![("includeArchived", "true".to_owned())]
        } else {
            vec![]
        };
        self.c.get("/mailboxes", &q).await
    }

    pub async fn get(&self, id: &str) -> Result<Mailbox> {
        self.c.get(&format!("/mailboxes/{}", seg(id)), &[]).await
    }

    /// A new address on a receiving domain: `domain_id` and `local_part` are required.
    pub async fn create(&self, params: &MailboxParams) -> Result<Mailbox> {
        self.c.send_json("POST", "/mailboxes", params).await
    }

    pub async fn update(&self, id: &str, params: &MailboxParams) -> Result<Mailbox> {
        self.c.send_json("PATCH", &format!("/mailboxes/{}", seg(id)), params).await
    }

    /// Archives the address. Its mail is kept.
    pub async fn archive(&self, id: &str) -> Result<Mailbox> {
        self.c.send_empty("DELETE", &format!("/mailboxes/{}", seg(id))).await
    }
}

/// Reads mail filed under an address. Reading never marks anything read.
pub struct MailboxMessages<'a> {
    pub(crate) c: &'a Client,
}

impl<'a> MailboxMessages<'a> {
    pub async fn list(&self, mailbox_id: &str, params: &ListMailboxMessagesParams) -> Result<MailboxMessageList> {
        let q = Q::new()
            .s("folder", &params.folder)
            .s("search", &params.search)
            .s("label", &params.label)
            .n("limit", params.limit)
            .s("cursor", &params.cursor);
        self.c.get(&format!("/mailboxes/{}/messages", seg(mailbox_id)), &q.0).await
    }

    /// An address's mail, following the cursor.
    pub fn list_all(
        &self,
        mailbox_id: impl Into<String>,
        params: ListMailboxMessagesParams,
    ) -> impl Stream<Item = Result<MailboxMessageRow>> + 'a {
        let client = self.c;
        let mailbox_id = mailbox_id.into();
        cursor(move |c| {
            let mut p = params.clone();
            p.cursor = c;
            let mailbox_id = mailbox_id.clone();
            async move {
                let res = MailboxMessages { c: client }.list(&mailbox_id, &p).await?;
                Ok((res.items, res.page))
            }
        })
    }

    pub async fn get(&self, mailbox_id: &str, message_id: &str) -> Result<MailboxMessage> {
        self.c
            .get(&format!("/mailboxes/{}/messages/{}", seg(mailbox_id), seg(message_id)), &[])
            .await
    }

    /// Download one attachment.
    pub async fn attachment(&self, mailbox_id: &str, message_id: &str, attachment_id: &str) -> Result<DownloadedFile> {
        let path = format!(
            "/mailboxes/{}/messages/{}/parts/{}",
            seg(mailbox_id),
            seg(message_id),
            seg(attachment_id)
        );
        let (content, headers) = self.c.bytes(&path, &[("download", "1".to_owned())]).await?;
        Ok(DownloadedFile {
            content,
            content_type: headers.get("content-type").cloned(),
            filename: headers.get("content-disposition").and_then(|d| filename_of(d)),
        })
    }
}

fn filename_of(disposition: &str) -> Option<String> {
    let lower = disposition.to_ascii_lowercase();
    if let Some(i) = lower.find("filename*=") {
        let raw = disposition[i + 10..].split(';').next().unwrap_or("").trim().trim_matches('"');
        let raw = raw.strip_prefix("UTF-8''").or_else(|| raw.strip_prefix("utf-8''")).unwrap_or(raw);
        if let Ok(decoded) = percent_encoding::percent_decode_str(raw).decode_utf8() {
            return Some(decoded.into_owned());
        }
    }
    let i = lower.find("filename=")?;
    let raw = disposition[i + 9..].split(';').next().unwrap_or("").trim().trim_matches('"');
    (!raw.is_empty()).then(|| raw.to_owned())
}

// ------------------------------------------------------------- workspace

/// Reads the key's workspace.
pub struct WorkspaceResource<'a> {
    pub(crate) c: &'a Client,
}

impl WorkspaceResource<'_> {
    /// The workspace this key belongs to.
    pub async fn get(&self) -> Result<crate::types::Workspace> {
        self.c.get("/workspace", &[]).await
    }
}

/// Lists API keys. Keys are created and revoked in the dashboard only.
pub struct ApiKeys<'a> {
    pub(crate) c: &'a Client,
}

impl ApiKeys<'_> {
    /// The workspace's keys, masked.
    pub async fn list(&self) -> Result<Vec<ApiKey>> {
        self.c.get("/api-keys", &[]).await
    }
}

/// Starts automations.
pub struct Automations<'a> {
    pub(crate) c: &'a Client,
}

impl Automations<'_> {
    /// Start a Live automation with a `manual` trigger for one contact.
    pub async fn enroll(&self, automation_id: &str, email: &str) -> Result<Object> {
        self.c
            .send_json(
                "POST",
                &format!("/automations/{}/enrollments", seg(automation_id)),
                &json!({ "email": email }),
            )
            .await
    }
}

// ---------------------------------------------------------------- paging

type Buffer<T> = std::collections::VecDeque<T>;

/// Pages addressed by number: `fetch(page)` answers the page's items and
/// whether it was the last.
fn numbered<T, F, Fut>(fetch: F) -> impl Stream<Item = Result<T>>
where
    F: Fn(u32) -> Fut,
    Fut: std::future::Future<Output = Result<(Vec<T>, bool)>>,
{
    stream::unfold((fetch, Some(1u32), Buffer::<T>::new()), |(fetch, next, mut buf)| async move {
        if let Some(item) = buf.pop_front() {
            return Some((Ok(item), (fetch, next, buf)));
        }
        let page = next?;
        match fetch(page).await {
            Ok((items, done)) => {
                buf.extend(items);
                let item = buf.pop_front()?;
                let next = if done { None } else { Some(page + 1) };
                Some((Ok(item), (fetch, next, buf)))
            }
            Err(e) => Some((Err(e), (fetch, None, buf))),
        }
    })
}

/// Pages addressed by cursor: `fetch(cursor)` answers the page's items and its paging block.
fn cursor<T, F, Fut>(fetch: F) -> impl Stream<Item = Result<T>>
where
    F: Fn(Option<String>) -> Fut,
    Fut: std::future::Future<Output = Result<(Vec<T>, CursorInfo)>>,
{
    stream::unfold(
        (fetch, Some(None::<String>), Buffer::<T>::new()),
        |(fetch, next, mut buf)| async move {
            if let Some(item) = buf.pop_front() {
                return Some((Ok(item), (fetch, next, buf)));
            }
            let c = next?;
            match fetch(c).await {
                Ok((items, page)) => {
                    buf.extend(items);
                    let item = buf.pop_front()?;
                    let next = match page.cursor {
                        Some(c) if page.has_more && !c.is_empty() => Some(Some(c)),
                        _ => None,
                    };
                    Some((Ok(item), (fetch, next, buf)))
                }
                Err(e) => Some((Err(e), (fetch, None, buf))),
            }
        },
    )
}
