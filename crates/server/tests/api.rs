//! Integration tests for the local REST API surface (no real port bound).

use std::io::Write as _;

use axum::Router;
use axum::body::Body;
use axum::http::{Request, StatusCode};
use formatwright_server::routes::{AppState, build_router};
use serde_json::Value;
use tempfile::{Builder, TempDir};
use tower::ServiceExt;

struct TestServer {
    router: Router,
    dir: TempDir,
    // Keeps the input fixture alive for the test duration.
    #[allow(dead_code)]
    input_file: tempfile::NamedTempFile,
    input: std::path::PathBuf,
}

fn test_server() -> TestServer {
    let dir = tempfile::tempdir().expect("temp dir");
    let mut input = Builder::new()
        .suffix(".json")
        .tempfile_in(dir.path())
        .expect("temp json fixture");
    input
        .write_all(br#"[{"id":1,"name":"alpha"}]"#)
        .expect("write fixture");
    let input_path = input.path().to_path_buf();
    let state = AppState::new(dir.path().join("jobs.sqlite3"));
    TestServer {
        router: build_router(state),
        dir,
        input_file: input,
        input: input_path,
    }
}

async fn post_json(router: &Router, uri: &str, body: Value) -> (StatusCode, Value) {
    let response = router
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(uri)
                .header("content-type", "application/json")
                .body(Body::from(body.to_string()))
                .expect("request"),
        )
        .await
        .expect("response");
    let status = response.status();
    let bytes = axum::body::to_bytes(response.into_body(), 1 << 20)
        .await
        .expect("body");
    (status, parse_json_body(status, &bytes))
}

async fn get(router: &Router, uri: &str) -> (StatusCode, Value) {
    let response = router
        .clone()
        .oneshot(
            Request::builder()
                .uri(uri)
                .body(Body::empty())
                .expect("request"),
        )
        .await
        .expect("response");
    let status = response.status();
    let bytes = axum::body::to_bytes(response.into_body(), 1 << 20)
        .await
        .expect("body");
    (status, parse_json_body(status, &bytes))
}

fn parse_json_body(status: StatusCode, bytes: &[u8]) -> Value {
    serde_json::from_slice(bytes).unwrap_or_else(|error| {
        panic!(
            "non-JSON body ({status}): {error}; body: {:?}",
            String::from_utf8_lossy(bytes)
        )
    })
}

#[tokio::test]
async fn health_reports_ok_and_version() {
    let server = test_server();
    let (status, body) = get(&server.router, "/health").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["status"], "ok");
    assert!(body["version"].as_str().is_some_and(|v| !v.is_empty()));
}

#[tokio::test]
async fn openapi_documents_all_endpoints() {
    let server = test_server();
    let (status, body) = get(&server.router, "/openapi.json").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["openapi"], "3.0.3");
    for path in [
        "/health",
        "/openapi.json",
        "/v1/plan",
        "/v1/convert",
        "/v1/capabilities",
    ] {
        assert!(
            body["paths"][path].is_object(),
            "openapi missing path {path}"
        );
    }
}

#[tokio::test]
async fn plan_returns_probe_and_plan_for_builtin_structured_route() {
    let server = test_server();
    let request = serde_json::json!({
        "inputPath": server.input,
        "target_format": "yaml",
        "output_path": server.input.with_extension("yaml"),
    });
    let (status, body) = post_json(&server.router, "/v1/plan", request).await;
    assert_eq!(status, StatusCode::OK, "body: {body}");
    assert_eq!(body["probe"]["format"]["id"], "json");
    assert_eq!(body["plan"]["target_format"], "yaml");
    assert!(body["planHash"].as_str().is_some_and(|h| !h.is_empty()));
}

#[tokio::test]
async fn plan_rejects_relative_input_with_structured_error() {
    let server = test_server();
    let request = serde_json::json!({
        "inputPath": "relative/input.json",
        "target_format": "yaml",
    });
    let (status, body) = post_json(&server.router, "/v1/plan", request).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(body["code"], "INPUT_INVALID");
    assert!(body["stage"].as_str().is_some());
    assert!(body["message"].as_str().is_some());
    assert!(body["action"].as_str().is_some());
}

#[tokio::test]
async fn plan_rejects_missing_input_file_with_structured_error() {
    let server = test_server();
    let missing = server.dir.path().join("missing.json");
    let request = serde_json::json!({
        "inputPath": missing,
        "target_format": "yaml",
    });
    let (status, body) = post_json(&server.router, "/v1/plan", request).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(body["code"], "INPUT_INVALID");
}

#[tokio::test]
async fn plan_maps_unsupported_target_to_422() {
    let server = test_server();
    let request = serde_json::json!({
        "inputPath": server.input,
        "target_format": "mp4",
    });
    let (status, body) = post_json(&server.router, "/v1/plan", request).await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "body: {body}");
    assert!(
        body["code"] == "UNSUPPORTED" || body["code"] == "ENGINE_MISSING",
        "unexpected code: {body}"
    );
}

#[tokio::test]
async fn convert_requires_output_path() {
    let server = test_server();
    let request = serde_json::json!({
        "inputPath": server.input,
        "target_format": "yaml",
    });
    let (status, body) = post_json(&server.router, "/v1/convert", request).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(body["code"], "INPUT_INVALID");
}

#[tokio::test]
async fn convert_maps_missing_input_to_400_with_structured_code() {
    let server = test_server();
    let missing = server.dir.path().join("nope.json");
    let request = serde_json::json!({
        "inputPath": missing,
        "target_format": "yaml",
        "output_path": server.dir.path().join("out.yaml"),
    });
    let (status, body) = post_json(&server.router, "/v1/convert", request).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(body["code"], "INPUT_INVALID");
    assert!(body["action"].as_str().is_some());
}

#[tokio::test]
async fn convert_builtin_structured_route_returns_validation_report() {
    let server = test_server();
    let output = server.dir.path().join("out.yaml");
    let request = serde_json::json!({
        "inputPath": server.input,
        "target_format": "yaml",
        "output_path": output,
    });
    let (status, body) = post_json(&server.router, "/v1/convert", request).await;
    assert_eq!(status, StatusCode::OK, "body: {body}");
    // Windows may canonicalize to the 8.3 short form; compare canonically.
    let reported =
        std::path::PathBuf::from(body["outputPath"].as_str().expect("outputPath string"));
    assert_eq!(
        reported.canonicalize().unwrap_or(reported.clone()),
        output.canonicalize().unwrap_or(output.clone())
    );
    assert!(
        body["jobId"].as_str().is_some_and(|id| !id.is_empty()),
        "expected durable job id"
    );
    // Product contract: every conversion response carries the acceptance report.
    assert!(body["validation"].is_object(), "missing validation report");
    assert!(
        body["validation"]["status"].as_str().is_some(),
        "validation report lacks status: {body}"
    );
}

#[tokio::test]
async fn capabilities_returns_snapshot_for_existing_input() {
    let server = test_server();
    let uri = format!(
        "/v1/capabilities?input={}",
        url_escape(server.input.to_string_lossy().as_ref())
    );
    let (status, body) = get(&server.router, &uri).await;
    assert_eq!(status, StatusCode::OK, "body: {body}");
    assert!(body.as_object().is_some());
}

#[tokio::test]
async fn capabilities_rejects_relative_path() {
    let server = test_server();
    let (status, body) = get(&server.router, "/v1/capabilities?input=rel.json").await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(body["code"], "INPUT_INVALID");
}

/// Minimal percent-encoding for query parameters on Windows paths.
fn url_escape(value: &str) -> String {
    const HEX: &[u8; 16] = b"0123456789ABCDEF";
    let mut out = String::new();
    for byte in value.bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(byte as char);
            }
            _ => {
                out.push('%');
                out.push(HEX[usize::from(byte / 16)] as char);
                out.push(HEX[usize::from(byte % 16)] as char);
            }
        }
    }
    out
}
