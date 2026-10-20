# SendClipper Rust SDK

The official Rust library for the [SendClipper](https://sendclipper.com) email API.

```bash
cargo add sendclipper
cargo add tokio --features macros,rt-multi-thread
```

In a workspace with several crates, `cargo add` at the root cannot pick one: add `--package <crate>` (the crate that sends mail) to both commands.

Async, on tokio and reqwest. TLS through rustls by default (`default-features = false, features = ["native-tls"]` for the platform's own). Rust 1.85 or newer. API reference on [docs.rs](https://docs.rs/sendclipper).

## Setup

Create an API key in the dashboard under **API keys**.

```rust
let client = sendclipper::Client::new("sc_live_xxxxxxxxxxxxxxxx");
// or: sendclipper::Client::from_env()  reads SENDCLIPPER_API_KEY
```

A `Client` is cheap to clone and safe to share across tasks.

## Send an email

```rust
use sendclipper::{Client, SendEmailParams};

#[tokio::main]
async fn main() -> Result<(), sendclipper::Error> {
    let client = Client::from_env();
    let email = client
        .emails()
        .send(&SendEmailParams {
            from: "hello@send.acme.io".into(),
            to: "ada@lumen.io".into(),
            subject: Some("Your receipt".into()),
            html: Some("<p>Thanks for your order.</p>".into()),
            text: Some("Thanks for your order.".into()),
            ..Default::default()
        })
        .await?;
    println!("Email {} accepted", email.id);
    Ok(())
}
```

`from` must be a bare address on a domain you verified in SendClipper. Each message has one recipient in `to`; for several, use `batch().send()`.

## Reply-To and attachments

```rust
use sendclipper::{Attachment, Client, SendEmailParams};

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let client = Client::from_env();
    let pdf = std::fs::read("booking-42.pdf")?;
    client
        .emails()
        .send(&SendEmailParams {
            from: "bookings@acme.io".into(),
            to: "ada@lumen.io".into(),
            subject: Some("Your booking".into()),
            html: Some("<p>Your confirmation is attached.</p>".into()),
            reply_to: Some(vec!["support@acme.io".into()]),
            attachments: Some(vec![Attachment::new("booking-42.pdf", pdf)]),
            ..Default::default()
        })
        .await?;
    Ok(())
}
```

`reply_to` is a list of 1 to 10 bare addresses. `Attachment::new` takes the file's bytes (any `Into<Vec<u8>>`) and the SDK sends them as base64; add `.content_type("application/pdf")` to set the type, otherwise it is guessed from the filename. Limits: 10 MiB per file, 25 MiB of files per message, and your sending provider's own. Attachments are not accepted on a batch item; send those emails on their own.

## Send with a template

Design and publish a template in the dashboard, then send it by id or by its send name:

```rust
let mut variables = sendclipper::Object::new();
variables.insert("first_name".into(), "Ada".into());

client
    .emails()
    .send(&SendEmailParams {
        from: "hello@send.acme.io".into(),
        to: "ada@lumen.io".into(),
        template_id: Some("tpl_9f2b7c10ad42".into()), // or template: Some("welcome".into())
        variables: Some(variables),
        ..Default::default()
    })
    .await?;
```

A tag with no value and no `{{tag|fallback}}` is refused by name (`template_variables_missing`), never sent blank.

## Send a batch

Up to 500 messages in one request. Each item is accepted or refused on its own:

```rust
let res = client.batch().send(&[first, second]).await?;
for item in &res.data {
    if let Some(err) = &item.error {
        eprintln!("{} {}", item.index, err.code);
    }
}
```

## Errors

Every method returns `Result<T, sendclipper::Error>`. A refusal from the API, or a request that never got an answer, is an `Error`:

```rust
pub struct Error {
    pub name: String,                    // SendClipper's code: "validation_failed", "rate_limited", "network_error", ...
    pub message: String,                 // a sentence for a person
    pub status_code: Option<u16>,        // None when no answer arrived (network, timeout)
    pub detail: Option<serde_json::Value>, // code-specific facts
    pub headers: BTreeMap<String, String>, // the response headers
}
```

```rust
match client.emails().send(&params).await {
    Ok(email) => println!("{}", email.id),
    Err(e) if e.name == "rate_limited" => retry_later(),
    Err(e) => return Err(e.into()),
}
```

## Retries and idempotency

Network errors, `429` and `5xx` answers are retried twice with backoff, honouring `Retry-After`. Every JSON write carries an `Idempotency-Key`, and the API answers a repeated key with the first result for 24 hours. A retried send therefore never goes out twice.

To make your own retries safe as well (a job that runs again after a crash), pass a key that names the logical operation:

```rust
client.with_idempotency_key(format!("order-{}-receipt", order.id)).emails().send(&params).await?;
```

## Options

Per call, on a cheap copy of the client:

| Method | Meaning |
|---|---|
| `with_idempotency_key(key)` | Your own key for a write |
| `with_timeout(duration)` | Per-attempt timeout (default 30 s) |
| `with_max_retries(n)` | Retries (default 2) |
| `with_header(name, value)` | An extra request header |

Client: `Client::builder().api_key(..).base_url(..).timeout(..).max_retries(..).default_header(..).http_client(reqwest_client).build()`. The base URL also reads `SENDCLIPPER_BASE_URL`.

## Stream every page

Lists that span pages have a `list_all` method that returns a `Stream`:

```rust
use futures_util::TryStreamExt;

let bounced = client.emails().list_all(sendclipper::ListEmailsParams { status: Some("bounced".into()), ..Default::default() });
futures_util::pin_mut!(bounced);
while let Some(email) = bounced.try_next().await? {
    println!("{}", email.to);
}
```

## Verify webhooks

SendClipper signs each delivery. Verify it against the **raw** body, before parsing. With axum:

```rust
use axum::{body::Bytes, http::{HeaderMap, StatusCode}};

async fn sendclipper_hook(headers: HeaderMap, body: Bytes) -> StatusCode {
    let secret = std::env::var("SENDCLIPPER_WEBHOOK_SECRET").unwrap_or_default();
    match sendclipper::verify_webhook(&body, &headers, &secret, Default::default()) {
        Ok(event) => {
            // event.id is stable across retries: deduplicate on it
            println!("{} {}", event.event, event.data["to"]);
            StatusCode::OK
        }
        Err(e) => {
            eprintln!("refused: {}", e.reason);
            StatusCode::BAD_REQUEST
        }
    }
}
```

The headers can be an `http::HeaderMap` (axum, actix-web, hyper), name/value pairs, or the header's value itself. During a secret rotation a delivery carries two signatures and either one is accepted. A delivery signed more than five minutes away from now is refused. `client.webhooks().verify(...)` is the same check.

To test your handler locally, `sendclipper::webhook_signature_header(secret, body, None)` builds a valid header.

## Sandboxes

A sandbox traps mail while you develop: nothing is delivered.

```rust
use std::time::Duration;

let sandbox = client.sandboxes().create(&sendclipper::SandboxParams { name: Some("my-app-dev".into()), ..Default::default() }).await?;
let creds = client.sandboxes().credentials(&sandbox.id).await?; // SMTP host, port, username, password

// Or send through the sandbox's JSON door with its own token, using the same fields as emails().send():
let token = client.sandboxes().reset_credentials(&sandbox.id).await?;
client.sandboxes().send(&token.api_token, &params).await?;

// In a test: wait for the message to arrive
let msg = client
    .sandboxes()
    .messages()
    .wait_for(&sandbox.id, |m| m.subject == "Hi", Duration::from_secs(30), Duration::from_secs(1))
    .await?;
```

## Everything else

| Resource | Methods |
|---|---|
| `emails()` | `send`, `get`, `list`, `list_all`, `stats` |
| `batch()` | `send` |
| `domains()` | `list`, `create`, `records`, `verify`, `enable_receiving`, `disable_receiving`, `set_region`, `deletion_preview`, `remove` |
| `templates()` | `list`, `get`, `html`, `create`, `update`, `remove`, `duplicate`, `publish`, `unpublish`, `preview`, `set_tags`, `projects` |
| `templates().tags()` | `list`, `create`, `update`, `remove` |
| `templates().assets()` | `list`, `upload` |
| `sandboxes()` | `list`, `get`, `create`, `update`, `remove`, `credentials`, `reset_credentials`, `send` |
| `sandboxes().messages()` | `list`, `get`, `raw`, `remove`, `clear`, `mark_all_read`, `wait_for`, `wait_for_next` |
| `webhooks()` | `list`, `create`, `update`, `remove`, `rotate_secret`, `deliveries`, `verify` |
| `webhooks().dispatches()` | `list`, `list_all`, `get`, `replay`, `replay_all` |
| `contacts()` | `list`, `list_all`, `create`, `import`, `import_preview`, `export` |
| `audiences()` | `list` |
| `suppressions()` | `list`, `list_all`, `create`, `remove`, `export` |
| `mailboxes()` | `list`, `get`, `create`, `update`, `archive` |
| `mailboxes().messages()` | `list`, `list_all`, `get`, `attachment` |
| `automations()` | `enroll` |
| `workspace()` | `get` |
| `api_keys()` | `list` |

Management routes need a key with **Full access**. A sending-only key can call `emails().send` and `batch().send`.

Full API reference: <https://sendclipper.com/docs>.

## Other languages

SendClipper has an official SDK for each of these languages, all calling the same API with the same conventions.

| Language | Package | Registry | Repository |
|---|---|---|---|
| Node.js | `@sendclipper/node` | [npm](https://www.npmjs.com/package/@sendclipper/node) | [sendclipper/sendclipper-node](https://github.com/sendclipper/sendclipper-node) |
| Python | `sendclipper` | [PyPI](https://pypi.org/project/sendclipper/) | [sendclipper/sendclipper-python](https://github.com/sendclipper/sendclipper-python) |
| PHP | `sendclipper/sendclipper-php` | [Packagist](https://packagist.org/packages/sendclipper/sendclipper-php) | [sendclipper/sendclipper-php](https://github.com/sendclipper/sendclipper-php) |
| Go | `github.com/sendclipper/sendclipper-go` | [pkg.go.dev](https://pkg.go.dev/github.com/sendclipper/sendclipper-go) | [sendclipper/sendclipper-go](https://github.com/sendclipper/sendclipper-go) |
| Ruby | `sendclipper` | [RubyGems](https://rubygems.org/gems/sendclipper) | [sendclipper/sendclipper-ruby](https://github.com/sendclipper/sendclipper-ruby) |
| Rust | `sendclipper` | [crates.io](https://crates.io/crates/sendclipper) | this library |
| Java | `com.sendclipper:sendclipper-java` | [Maven Central](https://central.sonatype.com/artifact/com.sendclipper/sendclipper-java) | [sendclipper/sendclipper-java](https://github.com/sendclipper/sendclipper-java) |

Documentation for all of them: <https://sendclipper.com/docs>.

## License

MIT
