use std::time::Duration;

use futures_util::TryStreamExt;
use sendclipper::*;
use serde_json::{json, Value};
use wiremock::matchers::{any, method, path};
use wiremock::{Mock, MockServer, Request, ResponseTemplate};

fn sent() -> Value {
    json!({"id": "0b6c0000-0000-4000-8000-000000000001", "messageId": "msg_9f2b7c10ad", "sesMessageId": null,
           "from": "hello@acme.io", "to": "ada@lumen.io", "domain": "acme.io", "subject": "Hi",
           "status": "queued", "createdAt": "2026-10-16T10:00:00.000Z", "templateId": null})
}

fn msg() -> SendEmailParams {
    SendEmailParams {
        from: "a@acme.io".into(),
        to: "b@lumen.io".into(),
        subject: Some("s".into()),
        html: Some("h".into()),
        ..Default::default()
    }
}

/// A client pointed at the mock server, with retries that wait no time worth
/// waiting for (Retry-After: 0 in the canned answers).
async fn setup() -> (MockServer, Client) {
    let server = MockServer::start().await;
    let client = Client::builder().api_key("sc_live_test").base_url(server.uri()).build();
    (server, client)
}

/// Answers each request with the next reply in order.
struct Seq(std::sync::Mutex<std::collections::VecDeque<ResponseTemplate>>);

impl wiremock::Respond for Seq {
    fn respond(&self, _: &Request) -> ResponseTemplate {
        self.0.lock().unwrap().pop_front().unwrap_or_else(|| ResponseTemplate::new(599))
    }
}

async fn replies(server: &MockServer, list: Vec<ResponseTemplate>) {
    Mock::given(any())
        .respond_with(Seq(std::sync::Mutex::new(list.into())))
        .mount(server)
        .await;
}

fn ok(body: Value) -> ResponseTemplate {
    ResponseTemplate::new(200).set_body_json(body)
}

fn api_err(status: u16, code: &str, message: &str) -> ResponseTemplate {
    ResponseTemplate::new(status)
        .set_body_json(json!({"error": {"code": code, "message": message}}))
        .insert_header("Retry-After", "0")
}

async fn calls(server: &MockServer) -> Vec<Request> {
    server.received_requests().await.unwrap_or_default()
}

fn body(r: &Request) -> Value {
    serde_json::from_slice(&r.body).unwrap_or(Value::Null)
}

fn query(r: &Request) -> Vec<(String, String)> {
    r.url.query_pairs().map(|(k, v)| (k.into_owned(), v.into_owned())).collect()
}

fn header<'a>(r: &'a Request, name: &str) -> Option<&'a str> {
    r.headers.get(name).and_then(|v| v.to_str().ok())
}

#[tokio::test]
async fn send_posts_to_v1_emails_with_the_key_and_an_idempotency_key() {
    let (server, client) = setup().await;
    replies(&server, vec![ResponseTemplate::new(201).set_body_json(sent())]).await;
    let email = client
        .emails()
        .send(&SendEmailParams {
            from: "hello@acme.io".into(),
            to: "ada@lumen.io".into(),
            subject: Some("Hi".into()),
            html: Some("<p>Hi</p>".into()),
            ..Default::default()
        })
        .await
        .unwrap();
    assert_eq!(email.id, "0b6c0000-0000-4000-8000-000000000001");
    assert_eq!(email.status, "queued");
    assert_eq!(email.template_id, None);
    let c = &calls(&server).await[0];
    assert_eq!(c.method.as_str(), "POST");
    assert_eq!(c.url.path(), "/v1/emails");
    assert_eq!(header(c, "authorization"), Some("Bearer sc_live_test"));
    assert_eq!(header(c, "content-type"), Some("application/json"));
    assert_eq!(header(c, "user-agent").unwrap(), format!("sendclipper-rust/{VERSION}"));
    let key = header(c, "idempotency-key").unwrap();
    assert!(key.starts_with("sc-rust-") && key.len() == "sc-rust-".len() + 36, "{key}");
    assert_eq!(
        std::str::from_utf8(&c.body).unwrap(),
        r#"{"from":"hello@acme.io","to":"ada@lumen.io","subject":"Hi","html":"<p>Hi</p>"}"#
    );
}

#[tokio::test]
async fn a_template_send_carries_template_id_and_variables_and_a_caller_key() {
    let (server, client) = setup().await;
    replies(&server, vec![ResponseTemplate::new(201).set_body_json(sent())]).await;
    let mut vars = Object::new();
    vars.insert("first_name".into(), "Ada".into());
    client
        .with_idempotency_key("order-42-receipt")
        .emails()
        .send(&SendEmailParams {
            from: "hello@acme.io".into(),
            to: "ada@lumen.io".into(),
            template_id: Some("tpl_9f2b7c10ad42".into()),
            variables: Some(vars),
            track_opens: Some(false),
            ..Default::default()
        })
        .await
        .unwrap();
    let c = &calls(&server).await[0];
    assert_eq!(header(c, "idempotency-key"), Some("order-42-receipt"));
    assert_eq!(
        body(c),
        json!({"from": "hello@acme.io", "to": "ada@lumen.io", "templateId": "tpl_9f2b7c10ad42", "variables": {"first_name": "Ada"}, "trackOpens": false})
    );
}

#[tokio::test]
async fn empty_bodies_travel_as_json_objects() {
    let (server, client) = setup().await;
    replies(&server, vec![ok(json!({})), ok(json!({})), ok(json!({})), ok(json!({}))]).await;
    client.templates().create(&TemplateParams::default()).await.unwrap();
    client.templates().preview("t1", Object::new()).await.unwrap();
    client.webhooks().dispatches().replay_all(None).await.unwrap();
    client.domains().verify("d1").await.unwrap();
    let c = calls(&server).await;
    let bodies: Vec<&str> = c.iter().map(|r| std::str::from_utf8(&r.body).unwrap()).collect();
    assert_eq!(bodies, vec!["{}", r#"{"variables":{}}"#, "{}", "{}"]);
}

#[tokio::test]
async fn a_503_is_retried_with_the_same_idempotency_key() {
    let (server, client) = setup().await;
    replies(
        &server,
        vec![
            api_err(503, "unavailable", "down"),
            ResponseTemplate::new(201).set_body_json(sent()),
        ],
    )
    .await;
    client.emails().send(&msg()).await.unwrap();
    let c = calls(&server).await;
    assert_eq!(c.len(), 2);
    assert_eq!(header(&c[0], "idempotency-key"), header(&c[1], "idempotency-key"));
}

#[tokio::test]
async fn a_network_error_answers_network_error_with_no_status() {
    let client = Client::builder().api_key("k").base_url("http://127.0.0.1:9").max_retries(0).build();
    let err = client.emails().get("x").await.unwrap_err();
    assert_eq!(err.name, "network_error");
    assert_eq!(err.status_code, None);
    assert!(
        err.message.contains("Could not reach SendClipper at http://127.0.0.1:9"),
        "{}",
        err.message
    );
}

#[tokio::test]
async fn a_timeout_names_the_limit_and_is_retried() {
    let (server, client) = setup().await;
    replies(
        &server,
        vec![
            ok(json!({})).set_delay(Duration::from_secs(2)),
            ok(json!({})).set_delay(Duration::from_secs(2)),
        ],
    )
    .await;
    let err = client
        .with_timeout(Duration::from_millis(100))
        .with_max_retries(1)
        .workspace()
        .get()
        .await
        .unwrap_err();
    assert_eq!(err.name, "network_error");
    assert!(err.message.contains("did not answer within 100ms"), "{}", err.message);
    assert_eq!(calls(&server).await.len(), 2);
}

#[tokio::test]
async fn retry_after_is_honoured() {
    let (server, client) = setup().await;
    replies(
        &server,
        vec![
            api_err(429, "rate_limited", "slow").insert_header("Retry-After", "1"),
            ok(json!([])),
        ],
    )
    .await;
    let started = std::time::Instant::now();
    assert!(client.domains().list().await.unwrap().is_empty());
    assert!(started.elapsed() >= Duration::from_millis(950), "waited {:?}", started.elapsed());
    assert_eq!(calls(&server).await.len(), 2);
}

#[tokio::test]
async fn idempotency_key_in_flight_is_retried_and_any_other_409_is_not() {
    let (server, client) = setup().await;
    replies(
        &server,
        vec![
            api_err(409, "idempotency_key_in_flight", "wait"),
            ResponseTemplate::new(201).set_body_json(sent()),
        ],
    )
    .await;
    client.emails().send(&msg()).await.unwrap();
    assert_eq!(calls(&server).await.len(), 2);

    let (server, client) = setup().await;
    replies(&server, vec![api_err(409, "sandbox_name_taken", "taken")]).await;
    let err = client
        .sandboxes()
        .create(&SandboxParams {
            name: Some("x".into()),
            ..Default::default()
        })
        .await
        .unwrap_err();
    assert_eq!(
        (err.name.as_str(), err.message.as_str(), err.status_code),
        ("sandbox_name_taken", "taken", Some(409))
    );
    assert_eq!(calls(&server).await.len(), 1);
}

#[tokio::test]
async fn a_4xx_is_not_retried_and_the_envelope_becomes_a_typed_error() {
    let (server, client) = setup().await;
    replies(&server, vec![ResponseTemplate::new(400).set_body_json(json!({"error": {"code": "template_variables_missing", "message": "Missing first_name", "detail": {"missing": ["first_name"]}}}))]).await;
    let err = client
        .emails()
        .send(&SendEmailParams {
            from: "a@acme.io".into(),
            to: "b@lumen.io".into(),
            template_id: Some("tpl_x".into()),
            ..Default::default()
        })
        .await
        .unwrap_err();
    assert_eq!(err.name, "template_variables_missing");
    assert_eq!(err.message, "Missing first_name");
    assert_eq!(err.status_code, Some(400));
    assert_eq!(err.detail, Some(json!({"missing": ["first_name"]})));
    assert_eq!(err.to_string(), "sendclipper: template_variables_missing (400): Missing first_name");
    assert_eq!(calls(&server).await.len(), 1);
}

#[tokio::test]
async fn a_validation_error_in_nests_default_shape_becomes_validation_failed() {
    let (server, client) = setup().await;
    replies(
        &server,
        vec![ResponseTemplate::new(400)
            .set_body_json(json!({"statusCode": 400, "message": ["to must be a valid email address"], "error": "Bad Request"}))],
    )
    .await;
    let err = client
        .emails()
        .send(&SendEmailParams {
            to: "nope".into(),
            ..msg()
        })
        .await
        .unwrap_err();
    assert_eq!(err.name, "validation_failed");
    assert_eq!(err.message, "to must be a valid email address");
}

#[tokio::test]
async fn a_non_json_error_body_still_answers_a_code() {
    let (server, client) = setup().await;
    replies(
        &server,
        vec![ResponseTemplate::new(502).set_body_string("<html>Bad gateway</html>")],
    )
    .await;
    let err = client.with_max_retries(0).domains().list().await.unwrap_err();
    assert_eq!(err.name, "http_502");
    assert_eq!(err.message, "<html>Bad gateway</html>");
}

#[tokio::test]
async fn get_requests_carry_no_idempotency_key_and_drop_empty_query_values() {
    let (server, client) = setup().await;
    replies(&server, vec![ok(json!({"items": [], "total": 0}))]).await;
    client
        .emails()
        .list(&ListEmailsParams {
            status: Some("bounced".into()),
            search: Some(String::new()),
            page_size: Some(50),
            ..Default::default()
        })
        .await
        .unwrap();
    let c = &calls(&server).await[0];
    assert_eq!(header(c, "idempotency-key"), None);
    assert_eq!(c.url.query(), Some("status=bounced&pageSize=50"));
}

#[tokio::test]
async fn booleans_in_the_query_are_written_the_way_the_api_reads_them() {
    let (server, client) = setup().await;
    replies(&server, vec![ok(json!([])), ok(json!([]))]).await;
    client.mailboxes().list(true).await.unwrap();
    client
        .webhooks()
        .deliveries(&ListWebhookDeliveriesParams {
            final_only: Some(false),
            ..Default::default()
        })
        .await
        .unwrap();
    let c = calls(&server).await;
    assert_eq!(query(&c[0]), vec![("includeArchived".into(), "true".into())]);
    assert_eq!(query(&c[1]), vec![("finalOnly".into(), "false".into())]);
}

#[tokio::test]
async fn emails_list_all_walks_every_page_and_stops_on_the_last() {
    let (server, client) = setup().await;
    let row = |n: u32| {
        let mut r = sent();
        r["id"] = format!("id-{n}").into();
        r
    };
    replies(
        &server,
        vec![
            ok(json!({"items": [row(1), row(2)], "total": 3})),
            ok(json!({"items": [row(3)], "total": 3})),
        ],
    )
    .await;
    let ids: Vec<String> = client
        .emails()
        .list_all(ListEmailsParams {
            page_size: Some(2),
            ..Default::default()
        })
        .map_ok(|e| e.id)
        .try_collect()
        .await
        .unwrap();
    assert_eq!(ids, vec!["id-1", "id-2", "id-3"]);
    let c = calls(&server).await;
    assert_eq!(c.len(), 2);
    assert!(query(&c[1]).contains(&("page".into(), "2".into())));
}

#[tokio::test]
async fn suppressions_list_all_follows_the_cursor_and_stops_when_you_do() {
    let (server, client) = setup().await;
    let s = |e: &str| json!({"id": e, "email": e, "reason": "manual"});
    replies(
        &server,
        vec![
            ok(json!({"data": [s("a@x.io")], "page": {"cursor": "c1", "hasMore": true}})),
            ok(json!({"data": [s("b@x.io")], "page": {"cursor": null, "hasMore": false}})),
        ],
    )
    .await;
    let all: Vec<String> = client
        .suppressions()
        .list_all(ListSuppressionsParams {
            reason: Some("manual".into()),
            cursor: Some("ignored".into()),
            ..Default::default()
        })
        .map_ok(|s| s.email)
        .try_collect()
        .await
        .unwrap();
    assert_eq!(all, vec!["a@x.io", "b@x.io"]);
    let c = calls(&server).await;
    assert_eq!(query(&c[0]), vec![("reason".into(), "manual".into())]);
    assert_eq!(
        query(&c[1]),
        vec![("reason".into(), "manual".into()), ("cursor".into(), "c1".into())]
    );

    let (server, client) = setup().await;
    replies(
        &server,
        vec![ok(json!({"data": [s("a"), s("b")], "page": {"cursor": "c1", "hasMore": true}}))],
    )
    .await;
    let stream = client.suppressions().list_all(ListSuppressionsParams::default());
    futures_util::pin_mut!(stream);
    assert_eq!(stream.try_next().await.unwrap().unwrap().email, "a");
    // Stop after one item: the stream must not have asked for a second page.
    assert_eq!(calls(&server).await.len(), 1);
}

#[tokio::test]
async fn a_list_all_yields_the_error_once_and_ends() {
    let (server, client) = setup().await;
    replies(&server, vec![api_err(403, "api_key_not_accepted", "no")]).await;
    let items: Vec<Result<Contact>> = futures_util::StreamExt::collect(client.contacts().list_all(ListContactsParams::default())).await;
    assert_eq!(items.len(), 1);
    let err = items.into_iter().next().unwrap().unwrap_err();
    assert_eq!((err.name.as_str(), err.status_code), ("api_key_not_accepted", Some(403)));
}

#[tokio::test]
async fn suppressions_remove_sends_the_recorded_reason_as_the_delete_body() {
    let (server, client) = setup().await;
    replies(&server, vec![ok(json!({"removed": true}))]).await;
    client
        .suppressions()
        .remove("sup-1", "Customer asked us to resume mail")
        .await
        .unwrap();
    let c = &calls(&server).await[0];
    assert_eq!((c.method.as_str(), c.url.path()), ("DELETE", "/v1/suppressions/sup-1"));
    assert_eq!(body(c), json!({"reason": "Customer asked us to resume mail"}));
}

#[tokio::test]
async fn sandboxes_send_uses_the_sandbox_token_instead_of_the_api_key() {
    let (server, client) = setup().await;
    replies(&server, vec![ResponseTemplate::new(201).set_body_json(json!({"id": "sbx-msg"}))]).await;
    assert_eq!(client.sandboxes().send("sc_sbx_abc", &msg()).await.unwrap().id, "sbx-msg");
    let c = &calls(&server).await[0];
    assert_eq!(c.url.path(), "/v1/sandbox/emails");
    assert_eq!(header(c, "authorization"), Some("Bearer sc_sbx_abc"));
}

#[tokio::test]
async fn reply_to_and_attachments_travel_with_base64_content() {
    let (server, client) = setup().await;
    replies(
        &server,
        vec![
            ResponseTemplate::new(201).set_body_json(sent()),
            ResponseTemplate::new(201).set_body_json(json!({"id": "sbx-msg"})),
        ],
    )
    .await;
    let params = SendEmailParams {
        reply_to: Some(vec!["support@acme.io".into(), "ops@acme.io".into()]),
        attachments: Some(vec![
            Attachment::new("hello.txt", b"Hello, world!".to_vec()).content_type("text/plain"),
            Attachment::new("raw.bin", vec![0xfb, 0xff, 0x00]),
        ]),
        ..msg()
    };
    client.emails().send(&params).await.unwrap();
    client.sandboxes().send("sc_sbx_abc", &params).await.unwrap();
    let all = calls(&server).await;
    assert_eq!(all[0].url.path(), "/v1/emails");
    assert_eq!(all[1].url.path(), "/v1/sandbox/emails");
    assert_eq!(header(&all[1], "authorization"), Some("Bearer sc_sbx_abc"));
    for c in &all {
        assert_eq!(
            std::str::from_utf8(&c.body).unwrap(),
            concat!(
                r#"{"from":"a@acme.io","to":"b@lumen.io","subject":"s","html":"h","#,
                r#""replyTo":["support@acme.io","ops@acme.io"],"#,
                r#""attachments":[{"filename":"hello.txt","content":"SGVsbG8sIHdvcmxkIQ==","contentType":"text/plain"},"#,
                r#"{"filename":"raw.bin","content":"+/8A"}]}"#
            )
        );
    }
}

#[test]
fn an_attachment_debug_prints_its_size_not_its_bytes() {
    let a = Attachment::new("a.pdf", vec![1u8; 2048]);
    assert_eq!(
        format!("{a:?}"),
        r#"Attachment { filename: "a.pdf", content: <2048 bytes>, content_type: None }"#
    );
}

#[tokio::test]
async fn contacts_import_is_multipart_with_no_idempotency_key_and_never_retried() {
    let (server, client) = setup().await;
    replies(&server, vec![api_err(503, "unavailable", "down")]).await;
    let err = client
        .contacts()
        .import(&ImportContactsParams {
            file: b"name,email\nAda,ada@lumen.io\n".to_vec(),
            filename: Some("contacts.csv".into()),
            on_duplicate: Some("update".into()),
            ..Default::default()
        })
        .await
        .unwrap_err();
    assert_eq!(err.status_code, Some(503));
    let c = calls(&server).await;
    assert_eq!(c.len(), 1);
    assert_eq!(header(&c[0], "idempotency-key"), None);
    let ctype = header(&c[0], "content-type").unwrap();
    let boundary = ctype.strip_prefix("multipart/form-data; boundary=").unwrap();
    let text = std::str::from_utf8(&c[0].body).unwrap();
    let parts: Vec<&str> = text.split(&format!("--{boundary}")).collect();
    assert_eq!(
        parts[1],
        "\r\nContent-Disposition: form-data; name=\"onDuplicate\"\r\n\r\nupdate\r\n"
    );
    assert_eq!(parts[2], "\r\nContent-Disposition: form-data; name=\"file\"; filename=\"contacts.csv\"\r\nContent-Type: text/csv\r\n\r\nname,email\nAda,ada@lumen.io\n\r\n");
}

#[tokio::test]
async fn template_assets_upload_base64_encodes_bytes() {
    let (server, client) = setup().await;
    replies(
        &server,
        vec![ResponseTemplate::new(201).set_body_json(json!({"id": "a", "url": "https://x/y.png"}))],
    )
    .await;
    let a = client
        .templates()
        .assets()
        .upload(&[1, 2, 3], "image/png", Some("logo"))
        .await
        .unwrap();
    assert_eq!(a.url, "https://x/y.png");
    assert_eq!(
        body(&calls(&server).await[0]),
        json!({"mimeType": "image/png", "base64": "AQID", "name": "logo"})
    );
}

#[tokio::test]
async fn text_endpoints_return_the_body_as_a_string() {
    let (server, client) = setup().await;
    replies(
        &server,
        vec![ResponseTemplate::new(200).set_body_raw("<html>hï</html>", "text/html; charset=utf-8")],
    )
    .await;
    assert_eq!(client.templates().html("tpl_1").await.unwrap(), "<html>hï</html>");
}

#[tokio::test]
async fn an_attachment_download_returns_bytes_type_and_filename() {
    let (server, client) = setup().await;
    replies(
        &server,
        vec![ResponseTemplate::new(200)
            .set_body_raw("%PDF", "application/pdf")
            .insert_header("Content-Disposition", "attachment; filename*=UTF-8''Rechnung%20M%C3%A4rz.pdf")],
    )
    .await;
    let f = client.mailboxes().messages().attachment("m1", "msg1", "p1").await.unwrap();
    assert_eq!(f.content, b"%PDF");
    assert_eq!(f.content_type.as_deref(), Some("application/pdf"));
    assert_eq!(f.filename.as_deref(), Some("Rechnung März.pdf"));
    let c = &calls(&server).await[0];
    assert_eq!(query(c), vec![("download".into(), "1".into())]);
    assert_eq!(header(c, "accept"), Some("*/*"));
}

#[tokio::test]
async fn path_segments_are_encoded() {
    let (server, client) = setup().await;
    replies(&server, vec![ok(json!({}))]).await;
    client.templates().get("a/b?c").await.unwrap();
    assert_eq!(calls(&server).await[0].url.path(), "/v1/templates/a%2Fb%3Fc");
}

#[tokio::test]
async fn no_key_anywhere_is_api_key_missing_and_no_request_is_made() {
    std::env::remove_var("SENDCLIPPER_API_KEY");
    let (server, _) = setup().await;
    let client = Client::builder().base_url(server.uri()).build();
    let err = client.domains().list().await.unwrap_err();
    assert_eq!((err.name.as_str(), err.status_code), ("api_key_missing", Some(401)));
    assert!(calls(&server).await.is_empty());
}

#[tokio::test]
async fn a_trailing_v1_in_the_base_url_is_tolerated() {
    let (server, _) = setup().await;
    replies(&server, vec![ok(json!([]))]).await;
    let client = Client::builder().api_key("k").base_url(format!("{}/v1/", server.uri())).build();
    client.domains().list().await.unwrap();
    assert_eq!(client.base_url(), server.uri());
    assert_eq!(calls(&server).await[0].url.path(), "/v1/domains");
}

#[tokio::test]
async fn default_and_per_call_headers_are_sent() {
    let server = MockServer::start().await;
    replies(&server, vec![ok(json!([]))]).await;
    let client = Client::builder()
        .api_key("k")
        .base_url(server.uri())
        .default_header("X-App", "shop")
        .build();
    client.with_header("X-Trace-Id", "t1").domains().list().await.unwrap();
    let c = &calls(&server).await[0];
    assert_eq!((header(c, "x-app"), header(c, "x-trace-id")), (Some("shop"), Some("t1")));
}

#[tokio::test]
async fn an_undecodable_answer_is_an_invalid_response_error() {
    let (server, client) = setup().await;
    replies(&server, vec![ok(json!({"total": "not a number"}))]).await;
    assert_eq!(client.emails().stats().await.unwrap_err().name, "invalid_response");
}

#[tokio::test]
async fn batch_send_posts_emails_and_returns_per_item_results() {
    let (server, client) = setup().await;
    let reply = json!({"data": [{"index": 0, "status": "accepted", "id": "e1", "messageId": "m1"},
                                {"index": 1, "status": "failed", "error": {"code": "template_not_published", "message": "draft"}}],
                       "summary": {"submitted": 2, "accepted": 1, "failed": 1}});
    replies(&server, vec![ok(reply.clone()), ok(reply)]).await;
    let items = vec![
        msg(),
        SendEmailParams {
            from: "a@acme.io".into(),
            to: "y@lumen.io".into(),
            template_id: Some("tpl_draft".into()),
            reply_to: Some(vec!["support@acme.io".into()]),
            ..Default::default()
        },
    ];
    let res = client.batch().send(&items).await.unwrap();
    assert_eq!(res.summary.accepted, 1);
    assert_eq!(res.data[1].error.as_ref().unwrap().code, "template_not_published");
    client.emails().send_batch(&items).await.unwrap();
    for c in calls(&server).await {
        assert_eq!(c.url.path(), "/v1/emails/batch");
        assert_eq!(
            body(&c),
            json!({"emails": [{"from": "a@acme.io", "to": "b@lumen.io", "subject": "s", "html": "h"}, {"from": "a@acme.io", "to": "y@lumen.io", "templateId": "tpl_draft", "replyTo": ["support@acme.io"]}]})
        );
        assert!(header(&c, "idempotency-key").is_some());
    }
}

#[tokio::test]
async fn sandbox_wait_for_polls_until_a_match_then_reads_it() {
    let (server, client) = setup().await;
    let summary = json!({"id": "m2", "subject": "Hi", "trappedAt": "2026-10-16T10:00:00.000Z"});
    let mut full = summary.clone();
    full["bodyHtml"] = "<p>Hi</p>".into();
    replies(&server, vec![ok(json!([])), ok(json!([summary])), ok(full)]).await;
    let m = client
        .sandboxes()
        .messages()
        .wait_for("sbx1", |m| m.subject == "Hi", Duration::from_secs(5), Duration::from_millis(1))
        .await
        .unwrap();
    assert_eq!(m.body_html.as_deref(), Some("<p>Hi</p>"));
    let c = calls(&server).await;
    let paths: Vec<&str> = c.iter().map(|r| r.url.path()).collect();
    assert_eq!(
        paths,
        vec![
            "/v1/sandboxes/sbx1/messages",
            "/v1/sandboxes/sbx1/messages",
            "/v1/sandboxes/sbx1/messages/m2"
        ]
    );
    assert_eq!(query(&c[0]), vec![("limit".into(), "50".into())]);
}

#[tokio::test]
async fn sandbox_wait_for_times_out_with_name_timeout() {
    let (server, client) = setup().await;
    replies(&server, (0..50).map(|_| ok(json!([]))).collect()).await;
    let err = client
        .sandboxes()
        .messages()
        .wait_for("sbx1", |_| true, Duration::ZERO, Duration::ZERO)
        .await
        .unwrap_err();
    assert_eq!((err.name.as_str(), err.status_code), ("timeout", None));
}

#[tokio::test]
async fn every_method_reaches_the_route_the_api_serves() {
    let (server, client) = setup().await;
    Mock::given(method("GET"))
        .and(path("/v1/templates"))
        .respond_with(ok(json!([])))
        .mount(&server)
        .await;
    for p in [
        "/v1/templates/projects",
        "/v1/templates/tags",
        "/v1/templates/assets",
        "/v1/sandboxes",
        "/v1/webhook-endpoints",
        "/v1/api-keys",
    ] {
        Mock::given(method("GET"))
            .and(path(p))
            .respond_with(ok(json!([])))
            .mount(&server)
            .await;
    }
    Mock::given(any()).respond_with(ok(json!({}))).mount(&server).await;
    let c = &client;
    macro_rules! route {
        ($m:literal, $p:literal, $e:expr) => {{
            let before = calls(&server).await.len();
            $e.await.unwrap();
            let all = calls(&server).await;
            let r = &all[before];
            assert_eq!((r.method.as_str(), r.url.path()), ($m, $p));
            if $m != "GET" {
                assert!(header(r, "idempotency-key").is_some(), "no idempotency key on {}", $p);
            }
        }};
    }
    route!(
        "POST",
        "/v1/domains",
        c.domains().create(&CreateDomainParams {
            name: "send.acme.io".into(),
            region: "eu-west-1".into()
        })
    );
    route!("GET", "/v1/domains/d1/records", c.domains().records("d1"));
    route!("POST", "/v1/domains/d1/receiving", c.domains().enable_receiving("d1"));
    route!("DELETE", "/v1/domains/d1/receiving", c.domains().disable_receiving("d1"));
    route!("PATCH", "/v1/domains/d1/region", c.domains().set_region("d1", "us-east-1"));
    route!("GET", "/v1/domains/d1/deletion", c.domains().deletion_preview("d1"));
    route!("DELETE", "/v1/domains/d1", c.domains().remove("d1"));
    route!("GET", "/v1/emails/stats/summary", c.emails().stats());
    route!("GET", "/v1/templates", c.templates().list(&ListTemplatesParams::default()));
    route!(
        "PATCH",
        "/v1/templates/t1",
        c.templates().update(
            "t1",
            &TemplateParams {
                name: Some("n".into()),
                ..Default::default()
            }
        )
    );
    route!("DELETE", "/v1/templates/t1", c.templates().remove("t1"));
    route!("POST", "/v1/templates/t1/duplicate", c.templates().duplicate("t1"));
    route!("POST", "/v1/templates/t1/publish", c.templates().publish("t1"));
    route!("POST", "/v1/templates/t1/unpublish", c.templates().unpublish("t1"));
    route!("GET", "/v1/templates/projects", c.templates().projects());
    route!("GET", "/v1/templates/tags", c.templates().tags().list());
    route!("POST", "/v1/templates/tags", c.templates().tags().create("n", "blue"));
    route!(
        "PATCH",
        "/v1/templates/tags/g1",
        c.templates().tags().update("g1", None, Some("red"))
    );
    route!("DELETE", "/v1/templates/tags/g1", c.templates().tags().remove("g1"));
    route!("GET", "/v1/templates/assets", c.templates().assets().list());
    route!("GET", "/v1/sandboxes", c.sandboxes().list());
    route!("GET", "/v1/sandboxes/s1", c.sandboxes().get("s1"));
    route!(
        "PATCH",
        "/v1/sandboxes/s1",
        c.sandboxes().update(
            "s1",
            &SandboxParams {
                name: Some("n".into()),
                ..Default::default()
            }
        )
    );
    route!("DELETE", "/v1/sandboxes/s1", c.sandboxes().remove("s1"));
    route!("GET", "/v1/sandboxes/s1/credentials", c.sandboxes().credentials("s1"));
    route!("POST", "/v1/sandboxes/s1/credentials", c.sandboxes().reset_credentials("s1"));
    route!(
        "DELETE",
        "/v1/sandboxes/s1/messages/m1",
        c.sandboxes().messages().remove("s1", "m1")
    );
    route!("DELETE", "/v1/sandboxes/s1/messages", c.sandboxes().messages().clear("s1"));
    route!("POST", "/v1/sandboxes/s1/read-all", c.sandboxes().messages().mark_all_read("s1"));
    route!("GET", "/v1/webhook-endpoints", c.webhooks().list());
    route!(
        "POST",
        "/v1/webhook-endpoints",
        c.webhooks().create(&WebhookParams {
            url: Some("https://x.io".into()),
            events: Some(vec!["email.delivered".into()]),
            ..Default::default()
        })
    );
    route!(
        "PATCH",
        "/v1/webhook-endpoints/w1",
        c.webhooks().update(
            "w1",
            &WebhookParams {
                active: Some(false),
                ..Default::default()
            }
        )
    );
    route!("DELETE", "/v1/webhook-endpoints/w1", c.webhooks().remove("w1"));
    route!("POST", "/v1/webhook-endpoints/w1/rotate-secret", c.webhooks().rotate_secret("w1"));
    route!(
        "GET",
        "/v1/webhook-dispatches",
        c.webhooks().dispatches().list(&ListWebhookDispatchesParams::default())
    );
    route!("GET", "/v1/webhook-dispatches/x1", c.webhooks().dispatches().get("x1"));
    route!("POST", "/v1/webhook-dispatches/x1/replay", c.webhooks().dispatches().replay("x1"));
    route!("GET", "/v1/contacts", c.contacts().list(&ListContactsParams::default()));
    route!(
        "POST",
        "/v1/contacts",
        c.contacts().create(&CreateContactParams {
            name: "Ada".into(),
            email: "a@x.io".into(),
            ..Default::default()
        })
    );
    route!("GET", "/v1/audiences", c.audiences().list());
    route!("POST", "/v1/suppressions", c.suppressions().create("a@x.io", None));
    route!("GET", "/v1/mailboxes/mb1", c.mailboxes().get("mb1"));
    route!(
        "POST",
        "/v1/mailboxes",
        c.mailboxes().create(&MailboxParams {
            domain_id: Some("d1".into()),
            local_part: Some("support".into()),
            ..Default::default()
        })
    );
    route!(
        "PATCH",
        "/v1/mailboxes/mb1",
        c.mailboxes().update(
            "mb1",
            &MailboxParams {
                archived: Some(true),
                ..Default::default()
            }
        )
    );
    route!("DELETE", "/v1/mailboxes/mb1", c.mailboxes().archive("mb1"));
    route!(
        "GET",
        "/v1/mailboxes/mb1/messages",
        c.mailboxes().messages().list("mb1", &ListMailboxMessagesParams::default())
    );
    route!("GET", "/v1/mailboxes/mb1/messages/m1", c.mailboxes().messages().get("mb1", "m1"));
    route!("POST", "/v1/automations/a1/enrollments", c.automations().enroll("a1", "ada@x.io"));
    route!("GET", "/v1/workspace", c.workspace().get());
    route!("GET", "/v1/api-keys", c.api_keys().list());
}

#[test]
fn version_matches_the_changelog() {
    let changelog = std::fs::read_to_string(concat!(env!("CARGO_MANIFEST_DIR"), "/CHANGELOG.md")).unwrap();
    assert!(
        changelog.contains(&format!("## {VERSION} ")),
        "CHANGELOG.md has no entry for {VERSION}"
    );
}
