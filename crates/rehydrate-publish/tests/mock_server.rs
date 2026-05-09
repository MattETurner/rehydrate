//! Integration tests against a tiny in-process HTTP server. Asserts
//! the auth headers, request body shape, and response handling for
//! both Ghost and WordPress.
//!
//! `tiny_http` is dev-only; the production crate has no test
//! dependencies bleeding in.

use std::io::Read;
use std::thread;

use rehydrate_publish::{DraftPost, GhostClient, PublishError, Publisher, WordpressClient};

struct Capture {
    method: String,
    path: String,
    auth: Option<String>,
    body: String,
}

fn capture_request(req: &mut tiny_http::Request) -> Capture {
    let mut body = String::new();
    let _ = req.as_reader().read_to_string(&mut body);
    let auth = req
        .headers()
        .iter()
        .find(|h| h.field.equiv("Authorization"))
        .map(|h| h.value.as_str().to_string());
    Capture {
        method: req.method().as_str().to_string(),
        path: req.url().to_string(),
        auth,
        body,
    }
}

fn json_response(status: u16, body: &str) -> tiny_http::Response<std::io::Cursor<Vec<u8>>> {
    tiny_http::Response::from_string(body.to_string())
        .with_status_code(status)
        .with_header(
            tiny_http::Header::from_bytes(&b"Content-Type"[..], &b"application/json"[..]).unwrap(),
        )
}

#[test]
fn ghost_publish_draft_sends_jwt_and_html_body() {
    let server = tiny_http::Server::http("127.0.0.1:0").unwrap();
    let addr = format!("http://{}", server.server_addr());
    let server_thread = thread::spawn(move || {
        let mut req = server.recv().unwrap();
        let cap = capture_request(&mut req);
        let _ = req.respond(json_response(
            201,
            r#"{"posts":[{"id":"abc123","url":"https://blog.example.com/p/abc123"}]}"#,
        ));
        cap
    });

    let client = GhostClient::new(rehydrate_publish::GhostCredentials {
        base_url: addr.clone(),
        admin_api_key: "kid:0011223344556677889900aabbccddeeff".into(),
    })
    .unwrap();
    let result = client
        .publish_draft(&DraftPost {
            title: "Title".into(),
            html: "<p>Hi</p>".into(),
            tags: vec!["from-rehydrate".into()],
        })
        .expect("publish");
    let cap = server_thread.join().unwrap();

    assert_eq!(result.post_id, "abc123");
    assert_eq!(cap.method, "POST");
    assert!(
        cap.path.contains("/ghost/api/admin/posts/?source=html"),
        "request path was {:?}",
        cap.path
    );
    let auth = cap.auth.expect("Authorization header missing");
    assert!(auth.starts_with("Ghost "), "auth was {auth:?}");
    let token = auth.trim_start_matches("Ghost ");
    assert_eq!(
        token.matches('.').count(),
        2,
        "expected JWT-shaped token, got {token:?}"
    );

    let body: serde_json::Value = serde_json::from_str(&cap.body).expect("body json");
    assert_eq!(body["posts"][0]["title"], "Title");
    assert_eq!(body["posts"][0]["status"], "draft");
    assert_eq!(body["posts"][0]["html"], "<p>Hi</p>");
}

#[test]
fn wordpress_publish_sends_basic_auth_and_html_content() {
    let server = tiny_http::Server::http("127.0.0.1:0").unwrap();
    let addr = format!("http://{}", server.server_addr());
    let server_thread = thread::spawn(move || {
        let mut req = server.recv().unwrap();
        let cap = capture_request(&mut req);
        let _ = req.respond(json_response(
            201,
            r#"{"id":42,"link":"https://blog.example.com/?p=42"}"#,
        ));
        cap
    });

    let client = WordpressClient::new(rehydrate_publish::WordpressCredentials {
        base_url: addr.clone(),
        username: "alice".into(),
        application_password: "abcd efgh ijkl mnop".into(),
    })
    .unwrap();
    let result = client
        .publish_draft(&DraftPost {
            title: "Hello".into(),
            html: "<p>world</p>".into(),
            tags: vec![],
        })
        .expect("publish");
    let cap = server_thread.join().unwrap();

    assert_eq!(result.post_id, "42");
    assert_eq!(cap.method, "POST");
    assert!(
        cap.path.contains("/wp-json/wp/v2/posts"),
        "path was {:?}",
        cap.path
    );
    let auth = cap.auth.expect("Authorization header missing");
    assert!(auth.starts_with("Basic "), "auth was {auth:?}");
    let body: serde_json::Value = serde_json::from_str(&cap.body).expect("body json");
    assert_eq!(body["title"], "Hello");
    assert_eq!(body["content"], "<p>world</p>");
    assert_eq!(body["status"], "draft");
}

#[test]
fn ghost_surfaces_401_as_auth_failed() {
    let server = tiny_http::Server::http("127.0.0.1:0").unwrap();
    let addr = format!("http://{}", server.server_addr());
    let _t = thread::spawn(move || {
        let req = server.recv().unwrap();
        let _ = req.respond(json_response(401, r#"{"errors":[{"message":"Auth failed"}]}"#));
    });
    let client = GhostClient::new(rehydrate_publish::GhostCredentials {
        base_url: addr,
        admin_api_key: "kid:00112233445566778899aabbccddeeff".into(),
    })
    .unwrap();
    let r = client.publish_draft(&DraftPost {
        title: "x".into(),
        html: "x".into(),
        tags: vec![],
    });
    assert!(matches!(r, Err(PublishError::AuthFailed(401))));
}
