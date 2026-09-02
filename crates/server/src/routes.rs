//! Router, handlers, and structured error mapping for the local API server.

use std::path::{Path, PathBuf};

use axum::Json;
use axum::extract::{FromRequest, Query, Request, State};
use axum::extract::rejection::JsonRejection;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use formatwright_core::domain::PlanRequest;
use formatwright_core::error::FormatWrightError;
use formatwright_core::{
    ApplicationStateService, ConversionService, ErrorCode, Plan, Probe, ReportService,
    SqliteJobStore, capability_snapshot_for_input, prepare_conversion,
};
use serde::Deserialize;
use serde::de::DeserializeOwned;
use serde_json::{Value, json};
use tokio_util::sync::CancellationToken;

/// Maximum accepted request body size (1 MiB) for this local API.
pub const MAX_BODY_BYTES: usize = 1024 * 1024;

/// Shared per-server state: the durable job-store database backing conversions.
#[derive(Clone, Debug)]
pub struct AppState {
    state_db: PathBuf,
}

impl AppState {
    #[must_use]
    pub fn new(state_db: impl Into<PathBuf>) -> Self {
        Self {
            state_db: state_db.into(),
        }
    }
}

/// Structured API error body: the wire form of `FormatWrightError`.
#[derive(Debug)]
pub struct ApiError {
    status: StatusCode,
    code: ErrorCode,
    stage: String,
    message: String,
    action: String,
    retryable: bool,
    diagnostic: Option<String>,
}

impl From<FormatWrightError> for ApiError {
    fn from(error: FormatWrightError) -> Self {
        let status = match error.code {
            ErrorCode::InputInvalid => StatusCode::BAD_REQUEST,
            ErrorCode::OutputConflict => StatusCode::CONFLICT,
            ErrorCode::Unsupported
            | ErrorCode::EngineMissing
            | ErrorCode::EngineIncompatible
            | ErrorCode::PolicyBlocked
            | ErrorCode::ResourceExhausted => StatusCode::UNPROCESSABLE_ENTITY,
            ErrorCode::InputChanged
            | ErrorCode::ExecutionFailed
            | ErrorCode::Cancelled
            | ErrorCode::ValidationFailed
            | ErrorCode::StorageFailed
            | ErrorCode::Internal => StatusCode::INTERNAL_SERVER_ERROR,
        };
        Self {
            status,
            code: error.code,
            stage: format!("{:?}", error.stage),
            message: error.message,
            action: error.user_action,
            retryable: error.retryable,
            diagnostic: error.diagnostic,
        }
    }
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        let body = json!({
            "code": self.code,
            "stage": self.stage,
            "message": self.message,
            "action": self.action,
            "retryable": self.retryable,
            "diagnostic": self.diagnostic,
        });
        (self.status, Json(body)).into_response()
    }
}

/// JSON request-body extractor that keeps rejections on the unified error
/// contract.
///
/// axum's built-in `Json` rejects malformed bodies with a `text/plain`
/// response that bypasses the `{code, stage, message, action}` shape; every
/// rejection here is remapped to `INPUT_INVALID` / stage `Inspect` (HTTP 400)
/// so API clients only ever see one error schema.
#[derive(Debug)]
struct ValidJson<T>(T);

impl<S, T> FromRequest<S> for ValidJson<T>
where
    S: Send + Sync,
    T: DeserializeOwned,
{
    type Rejection = ApiError;

    async fn from_request(req: Request, state: &S) -> Result<Self, Self::Rejection> {
        Json::<T>::from_request(req, state)
            .await
            .map(|Json(value)| ValidJson(value))
            .map_err(|rejection| json_rejection_error(&rejection))
    }
}

/// Maps an axum `Json` extractor rejection onto the structured `ApiError`.
fn json_rejection_error(rejection: &JsonRejection) -> ApiError {
    invalid_input(
        format!("invalid request body: {}", rejection.body_text()),
        "Fix the JSON request body and retry.",
    )
}

/// Request body for `/v1/plan` and `/v1/convert`.
///
/// `inputPath`/`outputPath` are camelCase at the wire level; all remaining
/// optional fields flatten into the core `PlanRequest` (`snake_case`, matching
/// the shared domain schema).
#[derive(Debug, Deserialize)]
pub struct ConversionBody {
    #[serde(rename = "inputPath", alias = "input_path")]
    input_path: PathBuf,
    #[serde(rename = "outputPath", alias = "output_path", default)]
    output_path: Option<PathBuf>,
    #[serde(flatten)]
    fields: serde_json::Map<String, Value>,
}

impl ConversionBody {
    /// Builds a `PlanRequest` from the remaining body fields. The core schema
    /// does not default every required field at the serde layer, so the
    /// provided fields are overlaid on a serialized `PlanRequest::default()`.
    fn into_plan_request(self) -> Result<PlanRequest, ApiError> {
        let mut merged = serde_json::to_value(PlanRequest::default())
            .map_err(|error| internal(format!("cannot serialize default plan request: {error}")))?;
        if let Value::Object(base) = &mut merged {
            base.extend(self.fields);
        }
        let mut request: PlanRequest = serde_json::from_value(merged).map_err(|error| {
            invalid_input(
                format!("invalid PlanRequest fields: {error}"),
                "Fix the request body fields and retry.",
            )
        })?;
        if request.output_path.is_none() {
            request.output_path = self.output_path;
        }
        Ok(request)
    }
}

#[derive(Debug, Deserialize)]
pub struct CapabilitiesQuery {
    input: PathBuf,
}

fn internal(message: impl Into<String>) -> ApiError {
    FormatWrightError::new(
        ErrorCode::Internal,
        formatwright_core::Stage::Plan,
        message,
        "This is a server bug; please report it with the request body.",
    )
    .into()
}

fn invalid_input(message: impl Into<String>, action: impl Into<String>) -> ApiError {
    FormatWrightError::new(
        ErrorCode::InputInvalid,
        formatwright_core::Stage::Inspect,
        message,
        action,
    )
    .into()
}

/// Validates that the given path is an absolute, existing file path.
fn require_absolute_input(path: &Path) -> Result<(), ApiError> {
    if !path.is_absolute() {
        return Err(invalid_input(
            format!("input path must be absolute: {}", path.display()),
            "Provide an explicit absolute path to the input file.",
        ));
    }
    if !path.exists() {
        return Err(invalid_input(
            format!("input file does not exist: {}", path.display()),
            "Check the input path and retry.",
        ));
    }
    if !path.is_file() {
        return Err(invalid_input(
            format!("input path is not a regular file: {}", path.display()),
            "Point inputPath at a file, not a directory.",
        ));
    }
    Ok(())
}

/// Builds the full API router with the 1 MiB body limit applied.
pub fn build_router(state: AppState) -> axum::Router {
    use axum::routing::{get, post};

    axum::Router::new()
        .route("/health", get(health))
        .route("/openapi.json", get(openapi))
        .route("/v1/plan", post(plan))
        .route("/v1/convert", post(convert))
        .route("/v1/capabilities", get(capabilities))
        // The demo page (website/demo.html) runs from file://; this is a
        // loopback-only local service, so a permissive CORS layer keeps the
        // browser from blocking the calls without any real exposure.
        .layer(axum::middleware::from_fn(cors_local_demo))
        .layer(axum::extract::DefaultBodyLimit::max(MAX_BODY_BYTES))
        .with_state(state)
}

async fn cors_local_demo(
    request: axum::extract::Request,
    next: axum::middleware::Next,
) -> axum::response::Response {
    // Preflight for application/json bodies arrives as OPTIONS; answer it
    // in the middleware so routes stay method-clean.
    if request.method() == axum::http::Method::OPTIONS {
        let mut response = axum::http::Response::new(axum::body::Body::empty());
        apply_cors_headers(response.headers_mut());
        return response.into();
    }
    let mut response = next.run(request).await;
    apply_cors_headers(response.headers_mut());
    response
}

fn apply_cors_headers(headers: &mut axum::http::HeaderMap) {
    headers.insert(
        "Access-Control-Allow-Origin",
        "*".parse().expect("static header value"),
    );
    headers.insert(
        "Access-Control-Allow-Headers",
        "Content-Type".parse().expect("static header value"),
    );
    headers.insert(
        "Access-Control-Allow-Methods",
        "GET, POST, OPTIONS".parse().expect("static header value"),
    );
}

#[allow(clippy::unused_async)]
async fn health() -> Json<Value> {
    Json(json!({
        "status": "ok",
        "version": env!("CARGO_PKG_VERSION"),
    }))
}

#[allow(clippy::unused_async)]
async fn openapi() -> Json<Value> {
    Json(openapi_document())
}

async fn plan(
    State(_state): State<AppState>,
    ValidJson(body): ValidJson<ConversionBody>,
) -> Result<Json<Value>, ApiError> {
    require_absolute_input(&body.input_path)?;
    let input_path = body.input_path.clone();
    let request = body.into_plan_request()?;
    let (probe, plan, _engine) = prepare_conversion(&input_path, &request)
        .await
        .map_err(ApiError::from)?;
    Ok(Json(plan_response(&probe, &plan)))
}

async fn convert(
    State(state): State<AppState>,
    ValidJson(body): ValidJson<ConversionBody>,
) -> Result<Json<Value>, ApiError> {
    require_absolute_input(&body.input_path)?;
    let input_path = body.input_path.clone();
    let request = body.into_plan_request()?;
    if request.output_path.is_none() {
        return Err(invalid_input(
            "immediate conversion requires an explicit outputPath",
            "Provide an absolute outputPath for the converted artifact.",
        ));
    }
    let (probe, plan, engine) = prepare_conversion(&input_path, &request)
        .await
        .map_err(ApiError::from)?;
    let mut store = open_job_store(&state.state_db)?;
    let reports = ReportService::new(default_reports_directory(&state.state_db));
    let result = ConversionService::run_prepared(
        &mut store,
        &reports,
        &probe,
        &plan,
        &engine,
        &plan.plan_hash,
        CancellationToken::new(),
        |_| {},
    )
    .await
    .map_err(ApiError::from)?;
    Ok(Json(json!({
        "outputPath": result.output_path,
        "jobId": result.job.id,
        "validation": result.report,
    })))
}

async fn capabilities(
    State(_state): State<AppState>,
    Query(query): Query<CapabilitiesQuery>,
) -> Result<Json<Value>, ApiError> {
    require_absolute_input(&query.input)?;
    let snapshot = capability_snapshot_for_input(
        &query.input,
        formatwright_core::EngineDiscoveryPolicy::for_current_build(),
    )
    .await;
    Ok(Json(
        serde_json::to_value(snapshot).unwrap_or_else(|_| json!({})),
    ))
}

fn plan_response(probe: &Probe, plan: &Plan) -> Value {
    json!({
        "probe": probe,
        "plan": plan,
        "planHash": plan.plan_hash,
    })
}

fn open_job_store(database_path: &Path) -> Result<SqliteJobStore, ApiError> {
    if let Some(parent) = database_path.parent()
        && !parent.as_os_str().is_empty()
    {
        std::fs::create_dir_all(parent).map_err(|error| {
            FormatWrightError::new(
                ErrorCode::StorageFailed,
                formatwright_core::Stage::Store,
                format!("cannot create state directory: {}", parent.display()),
                "Choose a writable state database path.",
            )
            .with_diagnostic(error.to_string())
        })?;
    }
    ApplicationStateService::from_database(database_path)?.recover_interrupted_restore()?;
    SqliteJobStore::open(database_path).map_err(ApiError::from)
}

fn default_reports_directory(database_path: &Path) -> PathBuf {
    database_path
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."))
        .join("reports")
}

/// Hand-written `OpenAPI` 3.0 document describing the local API surface.
#[allow(clippy::too_many_lines)]
pub fn openapi_document() -> Value {
    json!({
        "openapi": "3.0.3",
        "info": {
            "title": "FormatWright Local API",
            "version": env!("CARGO_PKG_VERSION"),
            "description": "Local REST API exposing probe/plan/convert with a ValidationReport on every conversion response. All paths must be absolute. Binds 127.0.0.1 by default."
        },
        "servers": [{ "url": "http://127.0.0.1:8787" }],
        "paths": {
            "/health": {
                "get": {
                    "summary": "Liveness probe",
                    "responses": {
                        "200": {
                            "description": "Server is healthy",
                            "content": {
                                "application/json": {
                                    "schema": {
                                        "type": "object",
                                        "properties": {
                                            "status": { "type": "string", "enum": ["ok"] },
                                            "version": { "type": "string" }
                                        }
                                    }
                                }
                            }
                        }
                    }
                }
            },
            "/openapi.json": {
                "get": { "summary": "This OpenAPI document", "responses": { "200": { "description": "OpenAPI 3.0 document" } } }
            },
            "/v1/plan": {
                "post": {
                    "summary": "Probe the input and build a conversion Plan without executing it",
                    "requestBody": {
                        "required": true,
                        "content": {
                            "application/json": {
                                "schema": { "$ref": "#/components/schemas/ConversionRequest" }
                            }
                        }
                    },
                    "responses": {
                        "200": {
                            "description": "Probe and Plan (not executed)",
                            "content": {
                                "application/json": {
                                    "schema": {
                                        "type": "object",
                                        "properties": {
                                            "probe": { "type": "object" },
                                            "plan": { "type": "object" },
                                            "planHash": { "type": "string" }
                                        }
                                    }
                                }
                            }
                        },
                        "400": { "$ref": "#/components/responses/ApiError" },
                        "422": { "$ref": "#/components/responses/ApiError" },
                        "500": { "$ref": "#/components/responses/ApiError" }
                    }
                }
            },
            "/v1/convert": {
                "post": {
                    "summary": "Execute a conversion and return the full ValidationReport",
                    "requestBody": {
                        "required": true,
                        "content": {
                            "application/json": {
                                "schema": { "$ref": "#/components/schemas/ConversionRequest" }
                            }
                        }
                    },
                    "responses": {
                        "200": {
                            "description": "Conversion completed; response always carries the acceptance report",
                            "content": {
                                "application/json": {
                                    "schema": {
                                        "type": "object",
                                        "properties": {
                                            "outputPath": { "type": "string" },
                                            "jobId": { "type": "string", "format": "uuid" },
                                            "validation": { "$ref": "#/components/schemas/ValidationReport" }
                                        }
                                    }
                                }
                            }
                        },
                        "400": { "$ref": "#/components/responses/ApiError" },
                        "409": { "$ref": "#/components/responses/ApiError" },
                        "422": { "$ref": "#/components/responses/ApiError" },
                        "500": { "$ref": "#/components/responses/ApiError" }
                    }
                }
            },
            "/v1/capabilities": {
                "get": {
                    "summary": "Capability snapshot for a given input file",
                    "parameters": [
                        {
                            "name": "input",
                            "in": "query",
                            "required": true,
                            "schema": { "type": "string" },
                            "description": "Absolute path to the input file"
                        }
                    ],
                    "responses": {
                        "200": { "description": "CapabilitySnapshot for the input" },
                        "400": { "$ref": "#/components/responses/ApiError" }
                    }
                }
            }
        },
        "components": {
            "schemas": {
                "ConversionRequest": {
                    "type": "object",
                    "required": ["inputPath", "targetFormat"],
                    "properties": {
                        "inputPath": { "type": "string", "description": "Absolute path to the input file" },
                        "outputPath": { "type": "string", "description": "Absolute path for the converted artifact (required for /v1/convert)" },
                        "targetFormat": { "type": "string", "description": "Wire alias: target_format" },
                        "output_path": { "type": "string" },
                        "preserve_all_streams": { "type": "boolean" },
                        "operation": { "type": "string", "nullable": true },
                        "page_range": { "type": "string", "nullable": true },
                        "dpi": { "type": "integer", "nullable": true },
                        "quality": { "type": "integer", "nullable": true },
                        "allow_lossy_data": { "type": "boolean" }
                    },
                    "additionalProperties": true,
                    "description": "inputPath/outputPath are camelCase; remaining fields follow the core PlanRequest snake_case schema."
                },
                "ValidationReport": {
                    "type": "object",
                    "description": "Acceptance evidence produced by the shared validation pipeline.",
                    "properties": {
                        "plan_hash": { "type": "string" },
                        "status": { "type": "string" }
                    },
                    "additionalProperties": true
                }
            },
            "responses": {
                "ApiError": {
                    "description": "Structured error",
                    "content": {
                        "application/json": {
                            "schema": {
                                "type": "object",
                                "required": ["code", "stage", "message", "action"],
                                "properties": {
                                    "code": { "type": "string" },
                                    "stage": { "type": "string" },
                                    "message": { "type": "string" },
                                    "action": { "type": "string" },
                                    "retryable": { "type": "boolean" },
                                    "diagnostic": { "type": "string", "nullable": true }
                                }
                            }
                        }
                    }
                }
            }
        }
    })
}
