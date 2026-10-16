use sendclipper::*;

const SECRET: &str = "whsec_test_secret";
const T: u64 = 1_756_800_000;
const BODY: &str = r#"{"id":"evt_1","event":"email.delivered","createdAt":"2026-09-06T09:41:02.518Z","data":{"emailId":"e1","messageId":"m1","to":"user@acme.io","domain":"send.acme.io","subject":"Your receipt"}}"#;
/// Computed with `openssl dgst -sha256 -hmac`, not with this SDK (the Node SDK's vector).
const SIG: &str = "76921f5ba77ae7f38686e8f147da3903c0ff87100cbd47ee466b7d646d198192";

fn header() -> String {
    format!("t={T},v1={SIG}")
}

fn at(now: u64) -> VerifyOptions {
    VerifyOptions {
        now: Some(now),
        ..Default::default()
    }
}

fn reason(r: std::result::Result<WebhookEvent, WebhookVerificationError>) -> &'static str {
    r.expect_err("expected a refusal").reason
}

#[test]
fn a_genuine_delivery_verifies_and_returns_the_parsed_event() {
    let ev = verify_webhook(BODY.as_bytes(), &header(), SECRET, at(T + 10)).unwrap();
    assert_eq!(ev.id, "evt_1");
    assert_eq!(ev.event, "email.delivered");
    assert_eq!(ev.data["to"], "user@acme.io");
}

#[test]
fn a_header_map_and_name_value_pairs_both_work() {
    let mut map = http::HeaderMap::new();
    map.insert("X-SendClipper-Signature-V2", header().parse().unwrap());
    assert_eq!(verify_webhook(BODY.as_bytes(), &map, SECRET, at(T)).unwrap().id, "evt_1");
    let h = header();
    let pairs = vec![
        ("content-type", "application/json"),
        ("HTTP_X_SENDCLIPPER_SIGNATURE_V2", h.as_str()),
    ];
    assert_eq!(verify_webhook(BODY.as_bytes(), pairs, SECRET, at(T)).unwrap().id, "evt_1");
}

#[test]
fn during_a_rotation_either_signature_is_accepted() {
    let other = "a".repeat(64);
    for h in [format!("t={T},v1={other},v1={SIG}"), format!("t={T},v1={SIG},v1={other}")] {
        assert_eq!(verify_webhook(BODY.as_bytes(), &h, SECRET, at(T)).unwrap().id, "evt_1");
    }
    let mut map = http::HeaderMap::new();
    map.append("x-sendclipper-signature-v2", format!("t={T},v1={other}").parse().unwrap());
    map.append("x-sendclipper-signature-v2", format!("v1={SIG}").parse().unwrap());
    assert_eq!(verify_webhook(BODY.as_bytes(), &map, SECRET, at(T)).unwrap().id, "evt_1");
}

#[test]
fn a_changed_body_a_wrong_secret_or_a_wrong_signature_is_refused() {
    assert_eq!(
        reason(verify_webhook(BODY.replace("user@", "evil@").as_bytes(), &header(), SECRET, at(T))),
        "signature_mismatch"
    );
    assert_eq!(
        reason(verify_webhook(BODY.as_bytes(), &header(), "whsec_other", at(T))),
        "signature_mismatch"
    );
    assert_eq!(
        reason(verify_webhook(
            BODY.as_bytes(),
            &format!("t={T},v1={}", "0".repeat(64)),
            SECRET,
            at(T)
        )),
        "signature_mismatch"
    );
    assert_eq!(
        reason(verify_webhook(BODY.as_bytes(), &format!("t={T},v1=not-hex"), SECRET, at(T))),
        "signature_mismatch"
    );
}

#[test]
fn re_encoded_json_fails_because_the_signature_covers_the_raw_bytes() {
    let v: serde_json::Value = serde_json::from_str(BODY).unwrap();
    let pretty = serde_json::to_string_pretty(&v).unwrap();
    assert_eq!(
        reason(verify_webhook(pretty.as_bytes(), &header(), SECRET, at(T))),
        "signature_mismatch"
    );
}

#[test]
fn a_timestamp_outside_the_tolerance_is_refused() {
    assert_eq!(
        reason(verify_webhook(BODY.as_bytes(), &header(), SECRET, at(T + 301))),
        "timestamp_out_of_tolerance"
    );
    assert_eq!(
        reason(verify_webhook(BODY.as_bytes(), &header(), SECRET, at(T - 301))),
        "timestamp_out_of_tolerance"
    );
    let wide = VerifyOptions {
        now: Some(T + 3600),
        tolerance_seconds: Some(3600),
    };
    assert_eq!(verify_webhook(BODY.as_bytes(), &header(), SECRET, wide).unwrap().id, "evt_1");
}

#[test]
fn a_missing_or_malformed_header_is_refused() {
    assert_eq!(
        reason(verify_webhook(BODY.as_bytes(), &http::HeaderMap::new(), SECRET, at(T))),
        "missing_header"
    );
    assert_eq!(reason(verify_webhook(BODY.as_bytes(), &header(), "", at(T))), "missing_header");
    for h in [
        format!("v1={SIG}"),
        format!("t={T}"),
        format!("t=abc,v1={SIG}"),
        format!("t=-5,v1={SIG}"),
    ] {
        assert_eq!(
            reason(verify_webhook(BODY.as_bytes(), &h, SECRET, at(T))),
            "malformed_header",
            "{h}"
        );
    }
}

#[test]
fn a_signed_body_that_is_not_an_event_is_refused() {
    let h = webhook_signature_header(SECRET, b"not json", Some(T));
    assert_eq!(reason(verify_webhook(b"not json", &h, SECRET, at(T))), "invalid_payload");
}

#[test]
fn webhook_signature_header_builds_what_verify_accepts() {
    let h = webhook_signature_header(SECRET, BODY.as_bytes(), Some(T));
    assert_eq!(h, header());
    assert_eq!(parse_signature_header(&h), (Some(T), vec![SIG.to_owned()]));
}

#[test]
fn the_clients_webhooks_verify_is_the_same_check() {
    let client = Client::builder().api_key("k").base_url("http://127.0.0.1:9").build();
    let h = webhook_signature_header(SECRET, BODY.as_bytes(), None);
    assert_eq!(client.webhooks().verify(BODY.as_bytes(), &h, SECRET).unwrap().id, "evt_1");
}
