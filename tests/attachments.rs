#![allow(clippy::unwrap_used)]
//! Tier-1 attachment bodies: base64-inlined binary, file-backed bodies,
//! Content-Type guessing, explicit overrides, and the `filename` ->
//! Content-Disposition sugar. Served through the real router via oneshot.

mod common;

use axum::http::header::{CONTENT_DISPOSITION, CONTENT_TYPE};
use common::{app, send};
use http_body_util::BodyExt;

fn cfg() -> axum::Router {
    app("petstore.yaml", Some("config-attachments.yaml"))
}

async fn body_bytes(resp: axum::http::Response<axum::body::Body>) -> Vec<u8> {
    resp.into_body()
        .collect()
        .await
        .unwrap()
        .to_bytes()
        .to_vec()
}

#[tokio::test]
async fn base64_inline_body_with_filename() {
    let resp = send(&cfg(), "GET", "/download/inline").await;
    assert_eq!(resp.status(), 200);
    assert_eq!(resp.headers()[CONTENT_TYPE], "application/octet-stream");
    assert_eq!(
        resp.headers()[CONTENT_DISPOSITION],
        "attachment; filename=\"blob.bin\""
    );
    assert_eq!(body_bytes(resp).await, b"BINARY\x00\x01\x02\xff");
}

#[tokio::test]
async fn file_body_guesses_pdf_content_type() {
    let resp = send(&cfg(), "GET", "/reports/42/download").await;
    assert_eq!(resp.status(), 200);
    assert_eq!(resp.headers()[CONTENT_TYPE], "application/pdf");
    assert_eq!(
        resp.headers()[CONTENT_DISPOSITION],
        "attachment; filename=\"report.pdf\""
    );
    let body = body_bytes(resp).await;
    assert!(body.starts_with(b"%PDF-1.4"));
}

#[tokio::test]
async fn file_body_guesses_csv_content_type() {
    let resp = send(&cfg(), "GET", "/export.csv").await;
    assert_eq!(resp.status(), 200);
    assert_eq!(resp.headers()[CONTENT_TYPE], "text/csv; charset=utf-8");
    // no filename set -> no Content-Disposition
    assert!(!resp.headers().contains_key(CONTENT_DISPOSITION));
    let body = String::from_utf8(body_bytes(resp).await).unwrap();
    assert!(body.starts_with("id,name,email"));
}

#[tokio::test]
async fn explicit_content_type_overrides_extension() {
    let resp = send(&cfg(), "GET", "/raw").await;
    assert_eq!(resp.status(), 200);
    assert_eq!(resp.headers()[CONTENT_TYPE], "image/png");
    assert_eq!(body_bytes(resp).await, b"BINARY\x00\x01\x02\xff");
}
