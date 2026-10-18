# Changelog

## 0.2.0 — 2026-10-18

- `SendEmailParams.reply_to: Option<Vec<String>>` — 1 to 10 addresses, sent as `Reply-To` (`replyTo` on the wire). Works on `emails().send`, `sandboxes().send` and batch items.
- `SendEmailParams.attachments: Option<Vec<Attachment>>` and the new `Attachment { filename, content: Vec<u8>, content_type }` type, with `Attachment::new(filename, bytes)` and `.content_type(..)`. The bytes are sent as standard base64. Limits: 10 MiB per file, 25 MiB per message, 20 files. Not accepted on a batch item (`attachments_not_batchable`).
- Adding two public fields to `SendEmailParams` breaks a struct literal that names every field without `..Default::default()`, hence the minor bump.
- Needs the SendClipper API from 2026-10-18 on; an older API ignores both fields.

## 0.1.1 — 2026-10-17

- README: how to install in a workspace of several crates (`cargo add --package`). No code change.

## 0.1.0 — 2026-10-16

First release.

- `emails().send`, `batch().send`, `emails().get`, `emails().list`, `emails().list_all`, `emails().stats`.
- Templates (with tags and image assets), domains, sandboxes (including the JSON door and `wait_for`), webhooks and their dispatches, contacts, audiences, suppressions, mailboxes, automations, workspace and API keys.
- Async (tokio + reqwest, rustls by default); typed params and responses; every failure is a `sendclipper::Error` whose `name` is the API's code.
- Automatic `Idempotency-Key` on every JSON write; retries on network errors, 429 and 5xx.
- `verify_webhook()` for `X-SendClipper-Signature-V2`, rotation-aware.
