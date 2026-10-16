//! Live end-to-end: this crate against a real SendClipper API, with an
//! `sc_live_` key minted for the run.
//!
//!     LIVE_API_URL=http://localhost:4000 cargo test --test live -- --nocapture
//!
//! It refuses anything that is not localhost: it creates and deletes data and
//! sends mail through whatever transport the API has, which must be the local
//! SES emulator (checked on /v1/health first). Skipped when LIVE_API_URL is unset.

use std::time::Duration;

use futures_util::TryStreamExt;
use sendclipper::*;
use serde_json::{json, Value};

struct Ctx {
    api: String,
    jwt: String,
    key_id: String,
    run: String,
    from: String,
    client: Client,
}

async fn raw(api: &str, method: reqwest::Method, path: &str, body: Option<Value>, token: Option<&str>) -> Value {
    let mut rb = reqwest::Client::new().request(method.clone(), format!("{api}/v1{path}"));
    if let Some(t) = token {
        rb = rb.bearer_auth(t);
    }
    if let Some(b) = body {
        rb = rb.json(&b);
    }
    let res = rb.send().await.expect("request");
    let status = res.status();
    let text = res.text().await.unwrap_or_default();
    assert!(status.is_success(), "{method} {path} -> {status} {text}");
    serde_json::from_str(&text).unwrap_or(Value::Null)
}

async fn setup() -> Option<Ctx> {
    let api = std::env::var("LIVE_API_URL").ok().filter(|s| !s.is_empty())?;
    let host = reqwest::Url::parse(&api).expect("LIVE_API_URL").host_str().unwrap_or("").to_owned();
    assert!(host == "localhost" || host == "127.0.0.1", "the live test runs against a local API only, not {host}");
    let health = raw(&api, reqwest::Method::GET, "/health", None, None).await;
    let b = &health["backends"];
    assert!(b["sesSend"] == "local" && b["objectStore"] == "filesystem", "refusing a non-local API: {b}");

    let email = std::env::var("LIVE_EMAIL").unwrap_or_else(|_| "enis@sendclipper.dev".into());
    let password = std::env::var("LIVE_PASSWORD").unwrap_or_else(|_| "password".into());
    let login = raw(&api, reqwest::Method::POST, "/auth/login", Some(json!({"email": email, "password": password})), None).await;
    let jwt = login["token"].as_str().expect("login token").to_owned();
    let run = format!("sdkrs{:x}", std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_millis());
    let key = raw(&api, reqwest::Method::POST, "/api-keys", Some(json!({"name": format!("{run}-full"), "permission": "full"})), Some(&jwt)).await;
    let client = Client::builder().api_key(key["token"].as_str().unwrap()).base_url(&api).build();
    let domains = client.domains().list().await.expect("domains");
    let domain = domains.iter().find(|d| d.status == "verified" && !d.name.ends_with(".example.com")).expect("a verified domain");
    Some(Ctx { from: format!("sdk-live@{}", domain.name), key_id: key["id"].as_str().unwrap().to_owned(), api, jwt, run, client })
}

#[tokio::test]
async fn live() {
    let Some(ctx) = setup().await else {
        eprintln!("skipped: LIVE_API_URL is not set");
        return;
    };
    let result = std::panic::AssertUnwindSafe(scenarios(&ctx));
    let outcome = futures_util::FutureExt::catch_unwind(result).await;
    raw(&ctx.api, reqwest::Method::DELETE, &format!("/api-keys/{}", ctx.key_id), None, Some(&ctx.jwt)).await;
    if let Err(panic) = outcome {
        std::panic::resume_unwind(panic);
    }
}

async fn scenarios(ctx: &Ctx) {
    let c = &ctx.client;
    let run = &ctx.run;

    // Every list and read decodes into its typed struct.
    assert!(!c.workspace().get().await.unwrap().public_id.is_empty());
    assert!(c.api_keys().list().await.unwrap().iter().any(|k| k.id == ctx.key_id));
    let domains = c.domains().list().await.unwrap();
    c.domains().records(&domains[0].id).await.unwrap();
    c.emails().stats().await.unwrap();
    c.emails().list(&ListEmailsParams { page_size: Some(5), ..Default::default() }).await.unwrap();
    let tpls = c.templates().list(&ListTemplatesParams::default()).await.unwrap();
    if let Some(t) = tpls.first() {
        c.templates().get(&t.id).await.unwrap();
    }
    c.templates().tags().list().await.unwrap();
    c.templates().assets().list().await.unwrap();
    c.templates().projects().await.unwrap();
    let boxes = c.sandboxes().list().await.unwrap();
    if let Some(b) = boxes.first() {
        c.sandboxes().credentials(&b.id).await.unwrap();
        let msgs = c.sandboxes().messages().list(&b.id, &ListSandboxMessagesParams { limit: Some(5), ..Default::default() }).await.unwrap();
        if let Some(m) = msgs.first() {
            c.sandboxes().messages().get(&b.id, &m.id).await.unwrap();
        }
    }
    c.webhooks().list().await.unwrap();
    c.webhooks().dispatches().list(&ListWebhookDispatchesParams { state: Some("all".into()), limit: Some(5), ..Default::default() }).await.unwrap();
    c.contacts().list(&ListContactsParams { page_size: Some(5), ..Default::default() }).await.unwrap();
    c.audiences().list().await.unwrap();
    c.suppressions().list(&ListSuppressionsParams { limit: Some(5), ..Default::default() }).await.unwrap();
    for mb in c.mailboxes().list(false).await.unwrap() {
        let page = c.mailboxes().messages().list(&mb.id, &ListMailboxMessagesParams { limit: Some(5), ..Default::default() }).await.unwrap();
        if let Some(m) = page.items.first() {
            c.mailboxes().messages().get(&mb.id, &m.id).await.unwrap();
            break;
        }
    }

    // Send, get, and a repeated idempotency key answers the first send.
    let p = SendEmailParams {
        from: ctx.from.clone(),
        to: format!("ada+{run}@lumen.io"),
        subject: Some(format!("SDK live {run} Grüße <ok>")),
        html: Some("<p>Hello from Rust</p>".into()),
        text: Some("Hello from Rust".into()),
        ..Default::default()
    };
    let keyed = c.with_idempotency_key(format!("{run}-welcome"));
    let first = keyed.emails().send(&p).await.unwrap();
    let again = keyed.emails().send(&p).await.unwrap();
    assert_eq!(first.id, again.id);
    let detail = c.emails().get(&first.id).await.unwrap();
    assert_eq!(detail.row.subject, p.subject.clone().unwrap());

    // A refusal is an Error with the API's code.
    let err = c.emails().send(&SendEmailParams { from: "nobody@not-a-domain-we-own.test".into(), ..p.clone() }).await.unwrap_err();
    assert_eq!(err.status_code, Some(403));
    let err = c.emails().send(&SendEmailParams { to: "not-an-address".into(), ..p.clone() }).await.unwrap_err();
    assert_eq!(err.name, "validation_failed");

    // Batch accepts and refuses items independently.
    let res = c
        .batch()
        .send(&[
            SendEmailParams { to: format!("b1+{run}@lumen.io"), ..p.clone() },
            SendEmailParams { from: ctx.from.clone(), to: format!("b2+{run}@lumen.io"), template_id: Some("tpl_doesnotexist0".into()), ..Default::default() },
        ])
        .await
        .unwrap();
    assert_eq!(res.summary.submitted, 2);
    assert_eq!((res.data[0].status.as_str(), res.data[1].status.as_str()), ("accepted", "failed"));

    // Templates: create, preview, publish, send by id, remove.
    let mut block = Object::new();
    block.insert("type".into(), "text".into());
    block.insert("text".into(), "Hi {{first_name|there}}, welcome aboard.".into());
    let t = c
        .templates()
        .create(&TemplateParams { name: Some(format!("{run} welcome")), subject: Some("Welcome {{first_name|there}}".into()), blocks: Some(vec![block]), ..Default::default() })
        .await
        .unwrap();
    let mut vars = Object::new();
    vars.insert("first_name".into(), "Ada".into());
    assert_eq!(c.templates().preview(&t.summary.id, vars.clone()).await.unwrap().subject, "Welcome Ada");
    assert_eq!(c.templates().preview(&t.summary.id, Object::new()).await.unwrap().subject, "Welcome there");
    c.templates().publish(&t.summary.id).await.unwrap();
    let sent = c
        .emails()
        .send(&SendEmailParams { from: ctx.from.clone(), to: format!("t+{run}@lumen.io"), template_id: Some(t.summary.public_id.clone()), variables: Some(vars), ..Default::default() })
        .await
        .unwrap();
    assert_eq!(sent.subject, "Welcome Ada");
    assert!(c.templates().html(&t.summary.id).await.unwrap().contains("Hi"));
    c.templates().remove(&t.summary.id).await.unwrap();

    // Sandboxes: reset credentials, send with the token, wait, raw.
    let b = c.sandboxes().create(&SandboxParams { name: Some(format!("{run}-box")), ..Default::default() }).await.unwrap();
    let creds = c.sandboxes().reset_credentials(&b.id).await.unwrap();
    assert!(creds.api_token.starts_with("sc_sbx_"));
    let subject = format!("trapped {run}");
    c.sandboxes()
        .send(&creds.api_token, &SendEmailParams { from: "app@example.com".into(), to: "someone@example.com".into(), subject: Some(subject.clone()), html: Some("<p>trap</p>".into()), ..Default::default() })
        .await
        .unwrap();
    let m = c.sandboxes().messages().wait_for(&b.id, |m| m.subject == subject, Duration::from_secs(15), Duration::from_millis(300)).await.unwrap();
    assert!(c.sandboxes().messages().raw(&b.id, &m.summary.id).await.unwrap().contains(&format!("Subject: {subject}")));
    c.sandboxes().remove(&b.id).await.unwrap();

    // Webhooks: the secret comes once and verify() accepts it.
    let hook = c
        .webhooks()
        .create(&WebhookParams { url: Some("http://127.0.0.1:9/sdk-live-rs".into()), events: Some(vec!["email.delivered".into(), "email.bounced".into()]), allow_insecure_http: Some(true), ..Default::default() })
        .await
        .unwrap();
    let secret = hook.secret.clone().expect("secret on create");
    assert!(secret.starts_with("whsec_"));
    let body = br#"{"id":"evt_live","event":"email.delivered","createdAt":"2026-10-16T10:00:00Z","data":{}}"#;
    let h = webhook_signature_header(&secret, body, None);
    assert_eq!(c.webhooks().verify(body, &h, &secret).unwrap().id, "evt_live");
    let up = c.webhooks().update(&hook.id, &WebhookParams { events: Some(vec!["email.opened".into()]), ..Default::default() }).await.unwrap();
    assert_eq!(up.events, vec!["email.opened"]);
    assert_ne!(c.webhooks().rotate_secret(&hook.id).await.unwrap().secret, Some(secret));
    c.webhooks().remove(&hook.id).await.unwrap();

    // Suppressions: add, find, remove with a reason.
    let email = format!("blocked+{run}@lumen.io");
    let created = c.suppressions().create(&email, Some("sdk live test")).await.unwrap();
    assert!(created.created);
    let found: Vec<Suppression> = c.suppressions().list_all(ListSuppressionsParams { q: Some(email.clone()), ..Default::default() }).try_collect().await.unwrap();
    assert!(found.iter().any(|s| s.email == email));
    c.suppressions().remove(&created.row.unwrap().id, "Removed by the Rust SDK live test run").await.unwrap();

    // Contacts, an import preview and a stream.
    let ct = c.contacts().create(&CreateContactParams { name: "SDK Live".into(), email: format!("contact+{run}@lumen.io"), ..Default::default() }).await.unwrap();
    assert_eq!(ct.email, format!("contact+{run}@lumen.io"));
    let preview = c.contacts().import_preview(&ImportContactsParams { file: format!("name,email\nRust Live,rs+{run}@lumen.io\n").into_bytes(), ..Default::default() }).await.unwrap();
    assert!(!preview.is_empty());
    let mine: Vec<EmailRow> = c.emails().list_all(ListEmailsParams { search: Some(run.clone()), page_size: Some(2), ..Default::default() }).try_collect().await.unwrap();
    assert!(mine.len() >= 3, "expected this run's sends in the log, saw {}", mine.len());
    eprintln!("live: every scenario passed ({run})");
}
