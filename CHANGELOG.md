# Changelog

## 0.1.1 — 2026-10-17

- README: how to install in a workspace of several crates (`cargo add --package`). No code change.

## 0.1.0 — 2026-10-16

First release.

- `emails().send`, `batch().send`, `emails().get`, `emails().list`, `emails().list_all`, `emails().stats`.
- Templates (with tags and image assets), domains, sandboxes (including the JSON door and `wait_for`), webhooks and their dispatches, contacts, audiences, suppressions, mailboxes, automations, workspace and API keys.
- Async (tokio + reqwest, rustls by default); typed params and responses; every failure is a `sendclipper::Error` whose `name` is the API's code.
- Automatic `Idempotency-Key` on every JSON write; retries on network errors, 429 and 5xx.
- `verify_webhook()` for `X-SendClipper-Signature-V2`, rotation-aware.
