//! The official Rust client for the [SendClipper](https://sendclipper.com) email API.
//!
//! ```no_run
//! # async fn run() -> sendclipper::Result<()> {
//! use sendclipper::{Client, SendEmailParams};
//!
//! let client = Client::new("sc_live_xxxxxxxxxxxxxxxx"); // or Client::from_env()
//! let email = client
//!     .emails()
//!     .send(&SendEmailParams {
//!         from: "hello@send.acme.io".into(),
//!         to: "ada@lumen.io".into(),
//!         subject: Some("Your receipt".into()),
//!         html: Some("<p>Thanks for your order.</p>".into()),
//!         ..Default::default()
//!     })
//!     .await?;
//! println!("Email {} accepted", email.id);
//! # Ok(())
//! # }
//! ```
//!
//! Every failure is a [`Error`] whose `name` is SendClipper's machine-readable
//! code (`validation_failed`, `rate_limited`, `network_error`, ...). Network
//! errors, 429 and 5xx answers are retried with backoff, honouring
//! `Retry-After`, and every JSON write carries an Idempotency-Key, so a
//! retried send never goes out twice. Verify webhook deliveries with
//! [`verify_webhook`].

mod client;
mod error;
mod resources;
pub mod types;
mod webhook;

pub use client::{Client, ClientBuilder, DEFAULT_BASE_URL};
pub use error::{Error, Result, WebhookVerificationError};
pub use resources::*;
pub use types::*;
pub use webhook::{
    parse_signature_header, sign_payload, verify_webhook, webhook_signature_header, Headers, VerifyOptions, SIGNATURE_HEADER,
    WEBHOOK_TOLERANCE_SECONDS,
};

/// This SDK's version, as on crates.io.
pub const VERSION: &str = env!("CARGO_PKG_VERSION");
