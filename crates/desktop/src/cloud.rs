//! Cloud bridge client (host side).
//!
//! The host keeps an outbound WebSocket to the bridge. Requests from remote
//! clients arrive as multiplexed frames; we replay them against the local API
//! so that access codes, printer sharing rules and audit logging stay exactly
//! the same regardless of whether the client is on the LAN or on the internet.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use futures_util::{SinkExt, StreamExt};
use lanprint_core::{CloudConfig, CloudRegisterRequest, CloudRegisterResponse, RelayFrame};
use tokio::sync::mpsc;
use tokio_tungstenite::tungstenite::client::IntoClientRequest;
use tokio_tungstenite::tungstenite::Message;
use tracing::{debug, info, warn};

use crate::app::App;
use crate::client::{base64_decode, base64_encode, url_encode};
use crate::error::{AppError, Result};

const CHUNK: usize = 256 * 1024;

pub async fn run_cloud_host(app: Arc<App>) {
    let mut backoff = 3u64;
    loop {
        let cfg = app.config.read().cloud.clone();
        if !cfg.enabled || cfg.server_url.is_empty() {
            app.set_cloud_status("");
            app.config.edit(|c| c.cloud.connected = false);
            return;
        }
        match connect_once(&app, &cfg).await {
            Ok(()) => {
                backoff = 3;
                app.set_cloud_status("cloud_disconnected");
            }
            Err(err) => {
                warn!("cloud bridge connection failed: {err}");
                app.set_cloud_status(format!("error:{err}"));
                app.config.edit(|c| {
                    c.cloud.connected = false;
                    c.cloud.last_error = err.to_string();
                });
                app.emit(crate::app::AppEvent::Refresh);
            }
        }
        tokio::time::sleep(Duration::from_secs(backoff)).await;
        backoff = (backoff * 2).min(60);
    }
}

fn token_valid(cfg: &CloudConfig) -> bool {
    !cfg.host_token.is_empty() && cfg.host_token_exp > chrono::Utc::now().timestamp() + 300
}

async fn connect_once(app: &Arc<App>, cfg: &CloudConfig) -> Result<()> {
    let http = reqwest::Client::builder()
        .timeout(Duration::from_secs(20))
        .build()
        .map_err(AppError::from)?;
    let server = cfg.server_url.trim_end_matches('/').to_string();
    let mut host_token = if token_valid(cfg) {
        cfg.host_token.clone()
    } else {
        String::new()
    };
    if host_token.is_empty() {
        let info = app.info();
        let body = CloudRegisterRequest {
            device_id: info.device_id,
            device_name: info.device_name,
            version: info.version,
            api_port: info.api_port,
            old_share_code: cfg.share_code.clone(),
        };
        let resp = http
            .post(format!("{server}/api/v1/hosts/register"))
            .bearer_auth(&cfg.server_code)
            .json(&body)
            .send()
            .await
            .map_err(|e| AppError::Cloud(format!("register: {e}")))?;
        let status = resp.status();
        if !status.is_success() {
            let text = resp.text().await.unwrap_or_default();
            return Err(AppError::Cloud(format!(
                "register rejected ({status}): {}",
                text.chars().take(200).collect::<String>()
            )));
        }
        let reg: CloudRegisterResponse = resp
            .json()
            .await
            .map_err(|e| AppError::Cloud(format!("register response: {e}")))?;
        host_token = reg.host_token.clone();
        app.config.edit(|c| {
            c.cloud.share_code = reg.share_code.clone();
            c.cloud.host_token = reg.host_token;
            c.cloud.host_token_exp = reg.expires_at;
            c.cloud.last_connected_at = chrono::Utc::now().timestamp();
            c.cloud.last_error.clear();
        });
        app.refresh_mdns().await;
        app.emit(crate::app::AppEvent::Toast("cloud_registered".into()));
        info!("registered with bridge, share code {}", reg.share_code);
    }

    let ws_base = server
        .replace("https://", "wss://")
        .replace("http://", "ws://");
    let url = format!("{ws_base}/ws/host?token={}", url_encode(&host_token));
    let request = url
        .into_client_request()
        .map_err(|e| AppError::Cloud(format!("websocket url: {e}")))?;
    let (socket, _) = match tokio::time::timeout(
        Duration::from_secs(15),
        tokio_tungstenite::connect_async(request),
    )
    .await
    {
        Ok(Ok(pair)) => pair,
        Ok(Err(err)) => {
            app.config.edit(|c| c.cloud.host_token.clear());
            return Err(AppError::Cloud(format!("websocket: {err}")));
        }
        Err(_) => {
            app.config.edit(|c| c.cloud.host_token.clear());
            return Err(AppError::Cloud("websocket timed out".into()));
        }
    };

    app.config.edit(|c| {
        c.cloud.connected = true;
        c.cloud.last_connected_at = chrono::Utc::now().timestamp();
        c.cloud.last_error.clear();
    });
    app.set_cloud_status("cloud_connected");
    app.emit(crate::app::AppEvent::Refresh);

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
    app.set_relay_sender(Some(tx.clone())).await;

    let hello = RelayFrame::Hello {
        role: "host".into(),
        device_id: app.config.read().device_id.clone(),
        name: app.config.read().device_name.clone(),
    };
    let _ = tx.send(hello);

    let local_http = reqwest::Client::builder()
        .timeout(Duration::from_secs(600))
        .build()
        .map_err(AppError::from)?;
    let mut pending: HashMap<u64, PendingRequest> = HashMap::new();
    let mut heartbeat = tokio::time::interval(Duration::from_secs(25));
    let api_port = app.config.read().api_port;

    loop {
        tokio::select! {
            _ = heartbeat.tick() => {
                let ping = RelayFrame::Ping { ts: chrono::Utc::now().timestamp() };
                let _ = tx.send(ping);
            }
            msg = stream.next() => {
                let Some(msg) = msg else { break; };
                let msg = msg.map_err(|e| AppError::Cloud(e.to_string()))?;
                let Message::Text(text) = msg else { continue; };
                let frame: RelayFrame = match serde_json::from_str(text.as_str()) {
                    Ok(f) => f,
                    Err(err) => { debug!("bad relay frame: {err}"); continue; }
                };
                match frame {
                    RelayFrame::Request { id, method, path, headers, content_length } => {
                        pending.insert(id, PendingRequest {
                            method,
                            path,
                            headers,
                            body: Vec::with_capacity(content_length.min(1024 * 1024) as usize),
                            expected: content_length,
                        });
                    }
                    RelayFrame::RequestChunk { id, data, last } => {
                        if let Some(req) = pending.get_mut(&id) {
                            match base64_decode(&data) {
                                Ok(chunk) => req.body.extend_from_slice(&chunk),
                                Err(err) => warn!("cloud chunk decode failed: {err}"),
                            }
                            if last {
                                let req = pending.remove(&id).unwrap();
                                let app = app.clone();
                                let client = local_http.clone();
                                tokio::spawn(async move {
                                    if let Err(err) = serve_relay_request(app, client, id, api_port, req).await {
                                        warn!("relay request failed: {err}");
                                    }
                                });
                            }
                        }
                    }
                    RelayFrame::Ping { ts } => {
                        let _ = tx.send(RelayFrame::Pong { ts });
                    }
                    RelayFrame::Bye { reason } => {
                        info!("bridge closed the host session: {reason}");
                        break;
                    }
                    _ => {}
                }
            }
        }
    }
    app.set_relay_sender(None).await;
    writer.abort();
    app.config.edit(|c| c.cloud.connected = false);
    app.set_cloud_status("cloud_disconnected");
    Ok(())
}

struct PendingRequest {
    method: String,
    path: String,
    headers: Vec<(String, String)>,
    body: Vec<u8>,
    expected: u64,
}

async fn serve_relay_request(
    app: Arc<App>,
    http: reqwest::Client,
    id: u64,
    api_port: u16,
    req: PendingRequest,
) -> Result<()> {
    if req.expected > 0 && req.body.len() as u64 != req.expected {
        debug!(
            "relay body length {} differs from announced {}",
            req.body.len(),
            req.expected
        );
    }
    let url = format!(
        "http://127.0.0.1:{api_port}{}",
        ensure_leading_slash(&req.path)
    );
    let method = reqwest::Method::from_bytes(req.method.as_bytes())
        .map_err(|e| AppError::Cloud(format!("method: {e}")))?;
    let mut builder = http.request(method, url).body(req.body);
    for (key, value) in &req.headers {
        let lower = key.to_ascii_lowercase();
        if matches!(
            lower.as_str(),
            "host" | "content-length" | "connection" | "transfer-encoding" | "accept-encoding"
        ) {
            continue;
        }
        builder = builder.header(lower, value);
    }
    builder = builder.header("x-lanprint-cloud", "1");
    let resp = match builder.send().await {
        Ok(resp) => resp,
        Err(err) => {
            send_error(&app, id, 502, &format!("local service unreachable: {err}")).await;
            return Ok(());
        }
    };
    let status = resp.status().as_u16();
    let headers: Vec<(String, String)> = resp
        .headers()
        .iter()
        .filter_map(|(k, v)| {
            let name = k.as_str().to_ascii_lowercase();
            if matches!(
                name.as_str(),
                "content-length" | "transfer-encoding" | "connection"
            ) {
                None
            } else {
                v.to_str().ok().map(|v| (name, v.to_string()))
            }
        })
        .collect();
    let body = match resp.bytes().await {
        Ok(body) => body.to_vec(),
        Err(err) => {
            send_error(&app, id, 502, &format!("local response failed: {err}")).await;
            return Ok(());
        }
    };
    send_response(&app, id, status, headers, body).await;
    Ok(())
}

fn ensure_leading_slash(path: &str) -> String {
    if path.starts_with('/') {
        path.to_string()
    } else {
        format!("/{path}")
    }
}

async fn send_response(
    app: &Arc<App>,
    id: u64,
    status: u16,
    headers: Vec<(String, String)>,
    body: Vec<u8>,
) {
    let Some(tx) = app.relay_sender().await else {
        debug!("relay sender is gone; dropping response {id}");
        return;
    };
    let head = RelayFrame::Response {
        id,
        status,
        headers,
        content_length: body.len() as u64,
    };
    if tx.send(head).is_err() {
        return;
    }
    for (index, chunk) in body.chunks(CHUNK).enumerate() {
        let last = (index + 1) * CHUNK >= body.len();
        let frame = RelayFrame::ResponseChunk {
            id,
            data: base64_encode(chunk),
            last,
        };
        if tx.send(frame).is_err() {
            return;
        }
    }
    if body.is_empty() {
        let _ = tx.send(RelayFrame::ResponseChunk {
            id,
            data: String::new(),
            last: true,
        });
    }
}

async fn send_error(app: &Arc<App>, id: u64, status: u16, message: &str) {
    if let Some(tx) = app.relay_sender().await {
        let _ = tx.send(RelayFrame::Error {
            id,
            status,
            message: message.to_string(),
        });
    }
}
