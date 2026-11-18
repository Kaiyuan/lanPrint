mod hub;
mod store;

use std::collections::HashMap;
use std::net::SocketAddr;
use std::sync::Arc;

use axum::extract::ws::{Message, WebSocket, WebSocketUpgrade};
use axum::extract::{ConnectInfo, Query, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use futures_util::{SinkExt, StreamExt};
use lanprint_core::{
    CloudConnectRequest, CloudConnectResponse, CloudError, CloudRegisterRequest,
    CloudRegisterResponse, RelayFrame,
};
use serde_json::json;
use tokio::sync::mpsc;
use tracing::{debug, info};

use hub::Hub;
use store::{random_token, HostRecord, Store};

/// The bridge server is versioned independently from the desktop app, so the
/// Docker image is only rebuilt when this version changes.
const VERSION: &str = env!("CARGO_PKG_VERSION");

#[derive(Clone)]
struct AppState {
    store: Arc<Store>,
    hub: Arc<Hub>,
    auth_code: String,
    public_url: String,
}

impl AppState {
    fn auth_ok(&self, supplied: &str) -> bool {
        constant_time_eq(supplied.as_bytes(), self.auth_code.as_bytes())
    }
}

fn constant_time_eq(a: &[u8], b: &[u8]) -> bool {
    if a.len() != b.len() {
        return false;
    }
    let mut diff = 0u8;
    for (x, y) in a.iter().zip(b) {
        diff |= x ^ y;
    }
    diff == 0
}

fn bearer(headers: &HeaderMap) -> Option<&str> {
    headers
        .get("authorization")
        .and_then(|v| v.to_str().ok())
        .and_then(|v| {
            v.strip_prefix("Bearer ")
                .or_else(|| v.strip_prefix("bearer "))
        })
        .map(str::trim)
        .filter(|s| !s.is_empty())
}

#[tokio::main]
async fn main() {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| "info".into()),
        )
        .with_target(false)
        .init();

    let mut addr = std::env::var("LANPRINT_BRIDGE_ADDR").unwrap_or_else(|_| "0.0.0.0:52334".into());
    let mut auth_code = std::env::var("LANPRINT_BRIDGE_AUTH_CODE").unwrap_or_default();
    let mut data_path =
        std::env::var("LANPRINT_BRIDGE_DATA").unwrap_or_else(|_| "data/hosts.json".into());
    let mut public_url = std::env::var("LANPRINT_BRIDGE_PUBLIC_URL").unwrap_or_default();

    let mut args = std::env::args().skip(1);
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--addr" => addr = args.next().unwrap_or(addr),
            "--auth-code" => auth_code = args.next().unwrap_or_default(),
            "--data" => data_path = args.next().unwrap_or(data_path),
            "--public-url" => public_url = args.next().unwrap_or_default(),
            "--version" | "-v" => {
                println!("lanprint-bridge {}", VERSION);
                return;
            }
            "--help" | "-h" => {
                println!(
                    "lanprint-bridge {}\n\n\
                     Environment:\n  LANPRINT_BRIDGE_ADDR (default 0.0.0.0:52334)\n  \
                     LANPRINT_BRIDGE_AUTH_CODE (required)\n  LANPRINT_BRIDGE_DATA (default data/hosts.json)\n  \
                     LANPRINT_BRIDGE_PUBLIC_URL (optional, used in client responses)\n\n\
                     Flags: --addr --auth-code --data --public-url",
                    VERSION
                );
                return;
            }
            _ => {}
        }
    }
    if auth_code.len() < 6 {
        eprintln!("LANPRINT_BRIDGE_AUTH_CODE must be set to at least 6 characters.");
        std::process::exit(2);
    }
    let store = match Store::load(data_path.into()) {
        Ok(store) => Arc::new(store),
        Err(err) => {
            eprintln!("cannot open bridge data store: {err}");
            std::process::exit(2);
        }
    };
    store.sync_auth_code(&auth_code);
    let state = AppState {
        store,
        hub: Arc::new(Hub::default()),
        auth_code,
        public_url,
    };

    let cleanup = state.hub.clone();
    tokio::spawn(async move {
        loop {
            tokio::time::sleep(std::time::Duration::from_secs(30)).await;
            cleanup.cleanup();
        }
    });

    let app = Router::new()
        .route("/health", get(health))
        .route("/api/v1/stats", get(stats))
        .route("/api/v1/hosts/register", post(register))
        .route("/api/v1/hosts/heartbeat", post(heartbeat))
        .route("/api/v1/clients/connect", post(connect))
        .route("/ws/host", get(ws_host))
        .route("/ws/client", get(ws_client))
        .with_state(state);

    let listener = match tokio::net::TcpListener::bind(&addr).await {
        Ok(listener) => listener,
        Err(err) => {
            eprintln!("cannot bind {addr}: {err}");
            std::process::exit(2);
        }
    };
    info!("lanPrint bridge {} listening on {addr}", VERSION);
    if let Err(err) = axum::serve(
        listener,
        app.into_make_service_with_connect_info::<SocketAddr>(),
    )
    .await
    {
        eprintln!("bridge stopped: {err}");
    }
}

async fn health() -> Json<serde_json::Value> {
    Json(json!({"status": "ok", "version": VERSION, "service": "lanprint-bridge"}))
}

async fn stats(State(state): State<AppState>, headers: HeaderMap) -> Response {
    let Some(token) = bearer(&headers) else {
        return err(
            StatusCode::UNAUTHORIZED,
            "unauthorized",
            "server access code required",
        );
    };
    if !state.auth_ok(token) {
        return err(
            StatusCode::UNAUTHORIZED,
            "unauthorized",
            "invalid server access code",
        );
    }
    let (total, _) = state.store.stats();
    let connected = state.hub.hosts.read().unwrap().len();
    Json(json!({
        "hosts": total,
        "online": connected,
        "connected": connected,
        "version": VERSION,
    }))
    .into_response()
}

async fn register(
    State(state): State<AppState>,
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
    headers: HeaderMap,
    Json(req): Json<CloudRegisterRequest>,
) -> Response {
    let Some(token) = bearer(&headers) else {
        return err(
            StatusCode::UNAUTHORIZED,
            "unauthorized",
            "server access code required",
        );
    };
    if !state.auth_ok(token) {
        return err(
            StatusCode::UNAUTHORIZED,
            "unauthorized",
            "invalid server access code",
        );
    }
    if req.device_id.is_empty() {
        return err(
            StatusCode::BAD_REQUEST,
            "invalid_request",
            "device_id is required",
        );
    }
    let share_code = state.store.ensure_code(&req);
    let host_token = random_token();
    let now = chrono::Utc::now().timestamp();
    state.store.upsert(HostRecord {
        device_id: req.device_id.clone(),
        device_name: req.device_name.clone(),
        version: req.version.clone(),
        share_code: share_code.clone(),
        host_token: host_token.clone(),
        api_port: req.api_port,
        created_at: now,
        last_seen: now,
        public_ip: peer.ip().to_string(),
    });
    info!(
        "host {} registered with share code {share_code}",
        req.device_id
    );
    Json(CloudRegisterResponse {
        share_code,
        host_token,
        expires_at: now + 30 * 24 * 3600,
        server_version: VERSION.to_string(),
    })
    .into_response()
}

async fn heartbeat(
    State(state): State<AppState>,
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
    headers: HeaderMap,
) -> Response {
    let Some(token) = bearer(&headers) else {
        return err(
            StatusCode::UNAUTHORIZED,
            "unauthorized",
            "host token required",
        );
    };
    let Some(host) = state.store.by_token(token) else {
        return err(
            StatusCode::UNAUTHORIZED,
            "unauthorized",
            "invalid host token",
        );
    };
    state.store.touch(&host.device_id, &peer.ip().to_string());
    Json(json!({"status": "ok"})).into_response()
}

async fn connect(State(state): State<AppState>, Json(req): Json<CloudConnectRequest>) -> Response {
    let Some(host) = state.store.by_share_code(req.share_code.trim()) else {
        return err(
            StatusCode::NOT_FOUND,
            "share_code_not_found",
            "unknown share code",
        );
    };
    if !state.hub.host_online(&host.device_id) {
        return err(
            StatusCode::SERVICE_UNAVAILABLE,
            "host_offline",
            "the host is not online",
        );
    }
    let session_token = random_token();
    let exp = chrono::Utc::now().timestamp() + 600;
    state
        .hub
        .add_session(session_token.clone(), host.device_id.clone(), exp);
    let public_url = if state.public_url.is_empty() {
        String::new()
    } else {
        state.public_url.clone()
    };
    Json(CloudConnectResponse {
        session_token,
        device_id: host.device_id,
        device_name: host.device_name,
        server_url: public_url,
        address_hint: host.public_ip,
        port_hint: host.api_port,
        expires_at: exp,
    })
    .into_response()
}

async fn ws_host(
    State(state): State<AppState>,
    Query(query): Query<HashMap<String, String>>,
    upgrade: WebSocketUpgrade,
) -> Response {
    let Some(token) = query.get("token") else {
        return err(StatusCode::UNAUTHORIZED, "unauthorized", "token required");
    };
    let Some(host) = state.store.by_token(token) else {
        return err(
            StatusCode::UNAUTHORIZED,
            "unauthorized",
            "invalid host token",
        );
    };
    state.store.touch(&host.device_id, "");
    upgrade.on_upgrade(move |socket| host_socket(state, socket, host))
}

async fn host_socket(state: AppState, socket: WebSocket, host: HostRecord) {
    let (mut sink, mut stream) = socket.split();
    let (tx, mut rx) = mpsc::unbounded_channel::<RelayFrame>();
    state.hub.register_host(
        &host.device_id,
        tx.clone(),
        host.device_name.clone(),
        host.api_port,
    );
    info!("host {} connected to bridge websocket", host.device_id);

    let writer = tokio::spawn(async move {
        while let Some(frame) = rx.recv().await {
            let Ok(text) = serde_json::to_string(&frame) else {
                continue;
            };
            if sink.send(Message::Text(text.into())).await.is_err() {
                break;
            }
        }
    });

    while let Some(msg) = stream.next().await {
        let Ok(msg) = msg else {
            break;
        };
        let Message::Text(text) = msg else {
            continue;
        };
        let Ok(frame) = serde_json::from_str::<RelayFrame>(text.as_str()) else {
            continue;
        };
        match frame {
            RelayFrame::Ping { ts } => {
                let _ = tx.send(RelayFrame::Pong { ts });
            }
            RelayFrame::Response { id, .. } => {
                forward_to_client(&state.hub, id, frame, false);
            }
            RelayFrame::ResponseChunk { id, last, .. } => {
                forward_to_client(&state.hub, id, frame, last);
            }
            RelayFrame::Error { id, .. } => {
                forward_to_client(&state.hub, id, frame, true);
            }
            RelayFrame::Hello { name, .. } => {
                debug!("host {} hello: {name}", host.device_id);
            }
            _ => {}
        }
    }

    state.hub.unregister_host(&host.device_id);
    writer.abort();
    info!("host {} disconnected from bridge", host.device_id);
}

fn forward_to_client(hub: &Hub, id: u64, frame: RelayFrame, last: bool) {
    let pending = hub.pending.read().unwrap().get(&id).map(|p| p.tx.clone());
    if let Some(tx) = pending {
        let _ = tx.send(frame);
    }
    if last {
        hub.finish_pending(id);
    }
}

async fn ws_client(
    State(state): State<AppState>,
    Query(query): Query<HashMap<String, String>>,
    upgrade: WebSocketUpgrade,
) -> Response {
    let Some(token) = query.get("token").cloned() else {
        return err(
            StatusCode::UNAUTHORIZED,
            "unauthorized",
            "session token required",
        );
    };
    let Some(host_id) = state.hub.session_host(&token) else {
        return err(
            StatusCode::UNAUTHORIZED,
            "unauthorized",
            "invalid or expired session",
        );
    };
    if !state.hub.host_online(&host_id) {
        return err(
            StatusCode::SERVICE_UNAVAILABLE,
            "host_offline",
            "the host is not online",
        );
    }
    upgrade.on_upgrade(move |socket| client_socket(state, socket, host_id, token))
}

async fn client_socket(state: AppState, socket: WebSocket, host_id: String, client_id: String) {
    let (mut sink, mut stream) = socket.split();
    let (tx, mut rx) = mpsc::unbounded_channel::<RelayFrame>();
    let writer = tokio::spawn(async move {
        while let Some(frame) = rx.recv().await {
            let Ok(text) = serde_json::to_string(&frame) else {
                continue;
            };
            if sink.send(Message::Text(text.into())).await.is_err() {
                break;
            }
        }
    });

    while let Some(msg) = stream.next().await {
        let Ok(msg) = msg else {
            break;
        };
        let Message::Text(text) = msg else {
            continue;
        };
        let Ok(frame) = serde_json::from_str::<RelayFrame>(text.as_str()) else {
            continue;
        };
        match &frame {
            RelayFrame::Ping { ts } => {
                let _ = tx.send(RelayFrame::Pong { ts: *ts });
            }
            RelayFrame::Request { id, .. } => {
                let Some(host_tx) = state.hub.host_sender(&host_id) else {
                    let _ = tx.send(RelayFrame::Error {
                        id: *id,
                        status: 503,
                        message: "host went offline".into(),
                    });
                    continue;
                };
                state.hub.add_pending(*id, &client_id, tx.clone());
                if host_tx.send(frame.clone()).is_err() {
                    state.hub.finish_pending(*id);
                    let _ = tx.send(RelayFrame::Error {
                        id: *id,
                        status: 503,
                        message: "host went offline".into(),
                    });
                }
            }
            RelayFrame::RequestChunk { id: _, .. } => {
                if let Some(host_tx) = state.hub.host_sender(&host_id) {
                    let _ = host_tx.send(frame.clone());
                }
            }
            _ => {}
        }
    }

    state.hub.drop_client(&client_id);
    writer.abort();
    debug!("client {client_id} disconnected");
}

fn err(status: StatusCode, error: &str, message: &str) -> Response {
    (
        status,
        Json(CloudError {
            error: error.to_string(),
            message: message.to_string(),
        }),
    )
        .into_response()
}
