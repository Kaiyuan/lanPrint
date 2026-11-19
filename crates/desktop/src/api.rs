use std::net::SocketAddr;
use std::sync::Arc;

use axum::body::Bytes;
use axum::extract::{ConnectInfo, DefaultBodyLimit, Path, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use lanprint_core::{
    AuthRequest, AuthResponse, DuplexMode, ErrorResponse, InfoResponse, JobRecord, JobsResponse,
    PrintResult, PrinterPreferences, PrinterTokenRequest, PrinterTokenResponse, PrintersResponse,
    ScanRequest, StatusResponse, MAX_JOB_BYTES,
};
use serde_json::json;
use tokio::net::TcpListener;
use tracing::{info, warn};

use crate::app::App;
use crate::error::AppError;
use crate::platform;

pub type ApiState = Arc<App>;

pub fn router(app: ApiState) -> Router {
    Router::new()
        .route("/api/v1/info", get(info))
        .route("/api/v1/health", get(health))
        .route("/api/v1/auth", post(auth))
        .route("/api/v1/printers", get(list_printers))
        .route("/api/v1/printers/{name}/token", post(printer_token))
        .route("/api/v1/printers/{name}/jobs", post(print_job))
        .route("/api/v1/printers/{name}/status", get(printer_status))
        .route("/api/v1/printers/{name}/scan", post(scan))
        .route("/api/v1/jobs", get(jobs))
        .route("/api/v1/ui/open", get(open_ui))
        .layer(DefaultBodyLimit::max(MAX_JOB_BYTES))
        .with_state(app)
}

pub async fn serve(app: ApiState, port: u16) -> std::io::Result<()> {
    let listener = TcpListener::bind(("0.0.0.0", port)).await?;
    info!("lanPrint API listening on 0.0.0.0:{port}");
    axum::serve(
        listener,
        router(app).into_make_service_with_connect_info::<SocketAddr>(),
    )
    .await
}

async fn info(State(app): State<ApiState>) -> Json<InfoResponse> {
    Json(app.info())
}

async fn health() -> Json<serde_json::Value> {
    Json(json!({"status": "ok", "version": lanprint_core::VERSION}))
}

async fn auth(
    State(app): State<ApiState>,
    Json(req): Json<AuthRequest>,
) -> Result<Json<AuthResponse>, ApiError> {
    if !app.config.read().has_host_password() {
        return Err(ApiError::new(
            StatusCode::FORBIDDEN,
            "host_not_configured",
            "host access code is not configured",
        ));
    }
    if req.password.len() > 128 || !app.verify_host_password(&req.password) {
        tokio::time::sleep(std::time::Duration::from_millis(300)).await;
        return Err(ApiError::new(
            StatusCode::UNAUTHORIZED,
            "wrong_password",
            "invalid access code",
        ));
    }
    let (token, expires_at) = app.issue_host_token()?;
    Ok(Json(AuthResponse { token, expires_at }))
}

async fn list_printers(
    State(app): State<ApiState>,
    headers: HeaderMap,
) -> Result<Json<PrintersResponse>, ApiError> {
    let token = bearer(&headers).ok_or_else(|| ApiError::unauthorized())?;
    app.authorize_host_token(token)?;
    Ok(Json(PrintersResponse {
        printers: app.shared_printers(),
    }))
}

async fn printer_token(
    State(app): State<ApiState>,
    Path(name): Path<String>,
    headers: HeaderMap,
    Json(req): Json<PrinterTokenRequest>,
) -> Result<Json<PrinterTokenResponse>, ApiError> {
    let token = bearer(&headers).ok_or_else(|| ApiError::unauthorized())?;
    app.authorize_host_token(token)?;
    let cfg = app.config.read();
    let Some(shared) = cfg.shared_printer(&name) else {
        return Err(ApiError::new(
            StatusCode::NOT_FOUND,
            "printer_not_found",
            "printer is not shared",
        ));
    };
    if !shared.enabled {
        return Err(ApiError::new(
            StatusCode::FORBIDDEN,
            "printer_disabled",
            "printer sharing is disabled",
        ));
    }
    let has_password = shared.has_password();
    drop(cfg);
    if has_password
        && (req.password.len() > 128 || !app.verify_printer_password(&name, &req.password))
    {
        tokio::time::sleep(std::time::Duration::from_millis(300)).await;
        return Err(ApiError::new(
            StatusCode::UNAUTHORIZED,
            "wrong_password",
            "invalid printer access code",
        ));
    }
    let (token, expires_at) = app.issue_printer_token(&name)?;
    Ok(Json(PrinterTokenResponse {
        token,
        expires_at,
        local_password_required: has_password,
    }))
}

/// Job preferences sent by the client, clamped to what the host printer can do.
fn job_preferences(
    headers: &HeaderMap,
    printer: &str,
    base: &PrinterPreferences,
    app: &App,
) -> PrinterPreferences {
    use base64::Engine as _;
    let decoded = headers
        .get("x-lanprint-preferences")
        .and_then(|v| v.to_str().ok())
        .and_then(|v| base64::engine::general_purpose::STANDARD.decode(v).ok())
        .and_then(|b| serde_json::from_slice::<PrinterPreferences>(&b).ok());
    let Some(mut prefs) = decoded else {
        return base.clone();
    };
    let caps = app.printer_capabilities(printer);

    if !caps.color {
        prefs.color = false;
    }
    if prefs.duplex != DuplexMode::Simplex && caps.duplex {
        let wanted = match prefs.duplex {
            DuplexMode::LongEdge => "long_edge",
            _ => "short_edge",
        };
        if !caps.duplex_modes.is_empty()
            && !caps
                .duplex_modes
                .iter()
                .any(|m| m.eq_ignore_ascii_case(wanted))
        {
            prefs.duplex = if caps.duplex_modes[0].eq_ignore_ascii_case("long_edge") {
                DuplexMode::LongEdge
            } else {
                DuplexMode::ShortEdge
            };
        }
    } else if !caps.duplex {
        prefs.duplex = DuplexMode::Simplex;
    }
    if prefs.booklet && !caps.booklet {
        prefs.booklet = false;
    }
    if prefs.booklet {
        prefs.duplex = DuplexMode::LongEdge;
    }
    if caps.max_copies > 0 {
        prefs.copies = prefs.copies().min(caps.max_copies);
    }
    if !caps.papers.is_empty() {
        let known = |paper: &str| caps.papers.iter().any(|p| p.eq_ignore_ascii_case(paper));
        if !known(&prefs.paper) {
            prefs.paper = if known(&base.paper) {
                base.paper.clone()
            } else {
                caps.papers[0].clone()
            };
        }
    }
    if !caps.sources.is_empty() && !prefs.media_source.is_empty() {
        if let Some(source) = caps
            .sources
            .iter()
            .find(|s| s.eq_ignore_ascii_case(&prefs.media_source))
        {
            prefs.media_source = source.clone();
        } else {
            prefs.media_source.clear();
        }
    }
    if prefs.resolution > 0 && !caps.resolutions.is_empty() {
        if !caps.resolutions.contains(&prefs.resolution) {
            prefs.resolution = if caps.resolutions.contains(&base.resolution) {
                base.resolution
            } else {
                0
            };
        }
    }
    prefs
}

#[allow(clippy::too_many_arguments)]
async fn print_job(
    State(app): State<ApiState>,
    Path(name): Path<String>,
    headers: HeaderMap,
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
    body: Bytes,
) -> Result<Json<PrintResult>, ApiError> {
    let token = token_header(&headers).ok_or_else(|| ApiError::unauthorized())?;
    app.authorize_printer_token(token, &name)?;
    let shared = app
        .shared_config(&name)
        .filter(|p| p.enabled)
        .ok_or_else(|| {
            ApiError::new(
                StatusCode::NOT_FOUND,
                "printer_not_found",
                "printer is not shared",
            )
        })?;
    if body.is_empty() {
        return Err(ApiError::new(
            StatusCode::BAD_REQUEST,
            "empty_job",
            "print data is empty",
        ));
    }
    let content_type = headers
        .get("content-type")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("application/octet-stream")
        .to_string();
    let filename = headers
        .get("x-lanprint-filename")
        .and_then(|v| v.to_str().ok())
        .map(decode_component)
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| "job".to_string());
    let prefs = job_preferences(&headers, &name, &shared.preferences, &app);
    let printer = name.clone();
    let len = body.len() as u64;
    let data = body.to_vec();
    let print_name = name.clone();
    let print_filename = filename.clone();
    let print_type = content_type.clone();
    let result = tokio::task::spawn_blocking(move || {
        platform::print_document(&print_name, &data, &print_type, &print_filename, &prefs)
    })
    .await
    .map_err(|e| AppError::Print(e.to_string()))?;
    let job_id = uuid::Uuid::new_v4().to_string();
    if let Err(err) = result {
        warn!("print job for {printer} failed: {err}");
        app.record_job(JobRecord {
            id: job_id,
            printer,
            source: peer.ip().to_string(),
            filename,
            bytes: len,
            status: "failed".into(),
            message: err.to_string(),
            created_at: chrono::Utc::now().timestamp(),
            finished_at: chrono::Utc::now().timestamp(),
        });
        return Err(ApiError::new(
            StatusCode::INTERNAL_SERVER_ERROR,
            "print_failed",
            err.to_string(),
        ));
    }
    app.record_job(JobRecord {
        id: job_id.clone(),
        printer: printer.clone(),
        source: peer.ip().to_string(),
        filename,
        bytes: len,
        status: "sent".into(),
        message: String::new(),
        created_at: chrono::Utc::now().timestamp(),
        finished_at: chrono::Utc::now().timestamp(),
    });
    Ok(Json(PrintResult {
        job_id,
        status: "queued".into(),
    }))
}

async fn printer_status(
    State(app): State<ApiState>,
    Path(name): Path<String>,
    headers: HeaderMap,
) -> Result<Json<StatusResponse>, ApiError> {
    let token = token_header(&headers).ok_or_else(|| ApiError::unauthorized())?;
    app.authorize_printer_token(token, &name)?;
    let printers = app.snapshot().local_printers;
    let status = printers
        .iter()
        .find(|p| p.name == name)
        .map(|p| p.status.clone())
        .unwrap_or_else(|| "offline".into());
    Ok(Json(StatusResponse {
        online: status != "offline",
        status,
        message: String::new(),
    }))
}

async fn scan(
    State(app): State<ApiState>,
    Path(name): Path<String>,
    headers: HeaderMap,
    Json(request): Json<ScanRequest>,
) -> Result<Response, ApiError> {
    let token = token_header(&headers).ok_or_else(|| ApiError::unauthorized())?;
    app.authorize_printer_token(token, &name)?;
    let _shared = app
        .shared_config(&name)
        .filter(|p| p.enabled)
        .ok_or_else(|| {
            ApiError::new(
                StatusCode::NOT_FOUND,
                "printer_not_found",
                "printer is not shared",
            )
        })?;
    let caps = app
        .snapshot()
        .local_printers
        .iter()
        .find(|p| p.name == name)
        .map(|p| p.capabilities.clone())
        .unwrap_or_default();
    if !caps.supports_scan {
        return Err(ApiError::new(
            StatusCode::NOT_IMPLEMENTED,
            "no_scanner",
            "this printer has no scanner",
        ));
    }
    let scanner_id = if request.scanner_id.is_empty() {
        caps.scanner_id.clone()
    } else {
        request.scanner_id.clone()
    };
    if scanner_id.is_empty() {
        return Err(ApiError::new(
            StatusCode::NOT_IMPLEMENTED,
            "no_scanner",
            "scanner id is missing",
        ));
    }
    let mut request = request;
    request.resolution = request.resolution.clamp(75, 1200);
    request.scanner_id = scanner_id.clone();
    let result = tokio::task::spawn_blocking(move || platform::scan(&request, &scanner_id))
        .await
        .map_err(|e| AppError::Scan(e.to_string()))?;
    let data = result.map_err(|e| {
        ApiError::new(
            StatusCode::INTERNAL_SERVER_ERROR,
            "scan_failed",
            e.to_string(),
        )
    })?;
    Ok((
        StatusCode::OK,
        [
            ("content-type", "image/png"),
            ("x-lanprint-scanner", name.as_str()),
        ],
        data,
    )
        .into_response())
}

async fn jobs(
    State(app): State<ApiState>,
    headers: HeaderMap,
) -> Result<Json<JobsResponse>, ApiError> {
    let token = bearer(&headers).ok_or_else(|| ApiError::unauthorized())?;
    app.authorize_host_token(token)?;
    Ok(Json(JobsResponse {
        jobs: app.snapshot().jobs,
    }))
}

async fn open_ui(
    State(app): State<ApiState>,
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
) -> Result<Json<serde_json::Value>, ApiError> {
    if !peer.ip().is_loopback() {
        return Err(ApiError::new(
            StatusCode::FORBIDDEN,
            "local_only",
            "this endpoint is only available locally",
        ));
    }
    app.emit(crate::app::AppEvent::ShowWindow);
    Ok(Json(json!({"status": "ok"})))
}

fn bearer(headers: &HeaderMap) -> Option<&str> {
    if let Some(value) = headers.get("authorization").and_then(|v| v.to_str().ok()) {
        if let Some(token) = value
            .strip_prefix("Bearer ")
            .or_else(|| value.strip_prefix("bearer "))
        {
            return Some(token.trim());
        }
    }
    token_header(headers)
}

fn token_header(headers: &HeaderMap) -> Option<&str> {
    headers
        .get("x-lanprint-token")
        .and_then(|v| v.to_str().ok())
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .or_else(|| {
            headers
                .get("authorization")
                .and_then(|v| v.to_str().ok())
                .and_then(|v| {
                    v.strip_prefix("Bearer ")
                        .or_else(|| v.strip_prefix("bearer "))
                })
        })
}

fn decode_component(value: &str) -> String {
    urlencoding::decode(value)
        .map(|v| v.into_owned())
        .unwrap_or_else(|_| value.to_string())
}

pub struct ApiError {
    status: StatusCode,
    error: String,
    message: String,
}

impl ApiError {
    pub fn new(status: StatusCode, error: impl Into<String>, message: impl Into<String>) -> Self {
        Self {
            status,
            error: error.into(),
            message: message.into(),
        }
    }

    fn unauthorized() -> Self {
        Self::new(
            StatusCode::UNAUTHORIZED,
            "unauthorized",
            "access code is required",
        )
    }
}

impl From<AppError> for ApiError {
    fn from(value: AppError) -> Self {
        let status = match &value {
            AppError::Auth(_) => StatusCode::UNAUTHORIZED,
            _ => StatusCode::INTERNAL_SERVER_ERROR,
        };
        Self::new(status, "error", value.to_string())
    }
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        (
            self.status,
            Json(ErrorResponse {
                error: self.error,
                message: self.message,
                code: String::new(),
            }),
        )
            .into_response()
    }
}
