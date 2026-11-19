use std::time::Duration;

use futures_util::{SinkExt, StreamExt};
use lanprint_core::{
    AuthRequest, AuthResponse, CloudConnectRequest, CloudConnectResponse, ErrorResponse,
    InfoResponse, PrintResult, PrinterPreferences, PrinterTokenRequest, PrinterTokenResponse,
    PrintersResponse, RelayFrame, ScanRequest, SharedPrinter,
};
use serde::de::DeserializeOwned;
use tokio_tungstenite::tungstenite::client::IntoClientRequest;
use tokio_tungstenite::tungstenite::Message;
use tracing::debug;

use crate::error::{AppError, Result};

const REQUEST_TIMEOUT: Duration = Duration::from_secs(30);
const CHUNK: usize = 256 * 1024;

#[derive(Clone, Debug)]
pub enum HostLink {
    /// Direct LAN/HTTP connection.
    Lan { base: String },
    /// Cloud bridge connection. A WebSocket is opened for each request; the
    /// bridge multiplexes frames and the host verifies every access code.
    Cloud { server: String, share_code: String },
}

impl HostLink {
    pub fn lan(address: &str, port: u16, https: bool) -> Self {
        let host = if address.contains(':') && !address.starts_with('[') {
            format!("[{address}]")
        } else {
            address.to_string()
        };
        Self::Lan {
            base: format!(
                "{}://{}:{}",
                if https { "https" } else { "http" },
                host,
                port
            ),
        }
    }

    pub fn cloud(server: &str, share_code: &str) -> Self {
        Self::Cloud {
            server: server.trim_end_matches('/').to_string(),
            share_code: share_code.to_string(),
        }
    }

    pub async fn info(&self, http: &reqwest::Client) -> Result<InfoResponse> {
        let bytes = self
            .request(http, "GET", "/api/v1/info", &[], Vec::new())
            .await?;
        parse_json(&bytes)
    }

    pub async fn auth(&self, http: &reqwest::Client, password: &str) -> Result<AuthResponse> {
        let body = serde_json::to_vec(&AuthRequest {
            password: password.to_string(),
        })?;
        let bytes = self
            .request(
                http,
                "POST",
                "/api/v1/auth",
                &[("content-type", "application/json")],
                body,
            )
            .await?;
        parse_json(&bytes)
    }

    pub async fn printers(
        &self,
        http: &reqwest::Client,
        token: &str,
    ) -> Result<Vec<SharedPrinter>> {
        let bytes = self
            .request(
                http,
                "GET",
                "/api/v1/printers",
                &[("authorization", &format!("Bearer {token}"))],
                Vec::new(),
            )
            .await?;
        let resp: PrintersResponse = parse_json(&bytes)?;
        Ok(resp.printers)
    }

    pub async fn printer_token(
        &self,
        http: &reqwest::Client,
        host_token: &str,
        printer: &str,
        password: &str,
    ) -> Result<PrinterTokenResponse> {
        let body = serde_json::to_vec(&PrinterTokenRequest {
            password: password.to_string(),
        })?;
        let path = format!("/api/v1/printers/{}/token", url_encode(printer));
        let bytes = self
            .request(
                http,
                "POST",
                &path,
                &[
                    ("authorization", &format!("Bearer {host_token}")),
                    ("content-type", "application/json"),
                ],
                body,
            )
            .await?;
        parse_json(&bytes)
    }

    #[allow(clippy::too_many_arguments)]
    pub async fn print(
        &self,
        http: &reqwest::Client,
        token: &str,
        printer: &str,
        data: Vec<u8>,
        content_type: &str,
        filename: &str,
        prefs: &PrinterPreferences,
    ) -> Result<PrintResult> {
        use base64::Engine as _;
        let encoded = base64::engine::general_purpose::STANDARD
            .encode(serde_json::to_vec(prefs).map_err(|e| AppError::Network(e.to_string()))?);
        let path = format!("/api/v1/printers/{}/jobs", url_encode(printer));
        let bytes = self
            .request(
                http,
                "POST",
                &path,
                &[
                    ("x-lanprint-token", token),
                    ("content-type", content_type),
                    ("x-lanprint-filename", &url_encode(filename)),
                    ("x-lanprint-preferences", &encoded),
                ],
                data,
            )
            .await?;
        parse_json(&bytes)
    }

    pub async fn scan(
        &self,
        http: &reqwest::Client,
        token: &str,
        printer: &str,
        request: &ScanRequest,
    ) -> Result<Vec<u8>> {
        let body = serde_json::to_vec(request)?;
        let path = format!("/api/v1/printers/{}/scan", url_encode(printer));
        self.request(
            http,
            "POST",
            &path,
            &[
                ("x-lanprint-token", token),
                ("content-type", "application/json"),
            ],
            body,
        )
        .await
    }

    async fn request(
        &self,
        http: &reqwest::Client,
        method: &str,
        path: &str,
        headers: &[(&str, &str)],
        body: Vec<u8>,
    ) -> Result<Vec<u8>> {
        match self {
            Self::Lan { base } => {
                let url = format!("{base}{path}");
                let method = reqwest::Method::from_bytes(method.as_bytes())
                    .map_err(|e| AppError::Network(e.to_string()))?;
                let mut req = http.request(method, &url).body(body);
                for (k, v) in headers {
                    req = req.header(*k, *v);
                }
                let resp = tokio::time::timeout(REQUEST_TIMEOUT, req.send())
                    .await
                    .map_err(|_| AppError::Network("request timed out".into()))??;
                let status = resp.status();
                let bytes = resp.bytes().await?.to_vec();
                if !status.is_success() {
                    return Err(error_from_body(status.as_u16(), &bytes));
                }
                Ok(bytes)
            }
            Self::Cloud { server, share_code } => {
                cloud_request(server, share_code, method, path, headers, body).await
            }
        }
    }
}

fn parse_json<T: DeserializeOwned>(bytes: &[u8]) -> Result<T> {
    serde_json::from_slice(bytes).map_err(|e| {
        AppError::Network(format!(
            "invalid response: {e} ({})",
            String::from_utf8_lossy(bytes)
                .chars()
                .take(200)
                .collect::<String>()
        ))
    })
}

fn error_from_body(status: u16, bytes: &[u8]) -> AppError {
    if let Ok(err) = serde_json::from_slice::<ErrorResponse>(bytes) {
        return AppError::Network(format!("{status}: {}", err.message));
    }
    AppError::Network(format!("{status}: {}", String::from_utf8_lossy(bytes)))
}

pub fn url_encode(value: &str) -> String {
    urlencoding::encode(value).into_owned()
}

async fn cloud_request(
    server: &str,
    share_code: &str,
    method: &str,
    path: &str,
    headers: &[(&str, &str)],
    body: Vec<u8>,
) -> Result<Vec<u8>> {
    let http = reqwest::Client::builder()
        .timeout(Duration::from_secs(10))
        .build()
        .map_err(AppError::from)?;
    let url = format!("{server}/api/v1/clients/connect");
    let resp = http
        .post(&url)
        .json(&CloudConnectRequest {
            share_code: share_code.to_string(),
            device_id: String::new(),
        })
        .send()
        .await
        .map_err(|e| AppError::Cloud(format!("bridge unreachable: {e}")))?;
    if !resp.status().is_success() {
        let status = resp.status().as_u16();
        let bytes = resp.bytes().await.unwrap_or_default();
        return Err(error_from_body(status, &bytes));
    }
    let connect: CloudConnectResponse = resp
        .json()
        .await
        .map_err(|e| AppError::Cloud(format!("invalid bridge response: {e}")))?;

    // Opportunistic direct connection: works when the host is reachable
    // (same LAN or a public address). If it fails we transparently fall back
    // to the bridge relay.
    if std::env::var("LANPRINT_ALLOW_DIRECT").ok().as_deref() == Some("1")
        && !connect.address_hint.is_empty()
        && connect.port_hint > 0
        && !connect.address_hint.starts_with("127.")
        && connect.address_hint != "::1"
        && body.len() <= 1024 * 1024
    {
        let hint = if connect.address_hint.contains(':') {
            format!("[{}]", connect.address_hint)
        } else {
            connect.address_hint.clone()
        };
        let direct_url = format!("http://{hint}:{}{}", connect.port_hint, path);
        let direct = reqwest::Client::builder()
            .connect_timeout(Duration::from_millis(1500))
            .timeout(Duration::from_secs(6))
            .build()
            .map_err(AppError::from)?;
        let method = reqwest::Method::from_bytes(method.as_bytes())
            .map_err(|e| AppError::Network(e.to_string()))?;
        let mut request = direct.request(method, direct_url).body(body.clone());
        for (k, v) in headers {
            request = request.header(*k, *v);
        }
        if let Ok(resp) = request.send().await {
            if resp.status().is_success() {
                let bytes = resp.bytes().await.unwrap_or_default().to_vec();
                if !bytes.is_empty() {
                    return Ok(bytes);
                }
            }
        }
    }

    // Direct connection is attempted by the caller; here we always relay.
    let ws_base = server
        .replace("https://", "wss://")
        .replace("http://", "ws://");
    let ws_url = format!(
        "{ws_base}/ws/client?token={}",
        url_encode(&connect.session_token)
    );
    let mut request = ws_url
        .into_client_request()
        .map_err(|e| AppError::Cloud(format!("invalid bridge URL: {e}")))?;
    request.headers_mut().insert(
        "user-agent",
        "lanPrint/2"
            .parse()
            .map_err(|_| AppError::Cloud("header".into()))?,
    );
    let (mut socket, _) = tokio::time::timeout(
        Duration::from_secs(15),
        tokio_tungstenite::connect_async(request),
    )
    .await
    .map_err(|_| AppError::Cloud("bridge websocket timed out".into()))?
    .map_err(|e| AppError::Cloud(format!("bridge websocket failed: {e}")))?;

    let hello = RelayFrame::Hello {
        role: "client".into(),
        device_id: String::new(),
        name: "lanPrint client".into(),
    };
    socket
        .send(Message::Text(serde_json::to_string(&hello)?.into()))
        .await
        .map_err(|e| AppError::Cloud(e.to_string()))?;

    let id = rand::random::<u64>();
    let header = RelayFrame::Request {
        id,
        method: method.to_string(),
        path: path.to_string(),
        headers: headers
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect(),
        content_length: body.len() as u64,
    };
    socket
        .send(Message::Text(serde_json::to_string(&header)?.into()))
        .await
        .map_err(|e| AppError::Cloud(e.to_string()))?;
    for (index, chunk) in body.chunks(CHUNK).enumerate() {
        let last = (index + 1) * CHUNK >= body.len();
        let frame = RelayFrame::RequestChunk {
            id,
            data: base64_encode(chunk),
            last,
        };
        socket
            .send(Message::Text(serde_json::to_string(&frame)?.into()))
            .await
            .map_err(|e| AppError::Cloud(e.to_string()))?;
    }
    if body.is_empty() {
        let frame = RelayFrame::RequestChunk {
            id,
            data: String::new(),
            last: true,
        };
        socket
            .send(Message::Text(serde_json::to_string(&frame)?.into()))
            .await
            .map_err(|e| AppError::Cloud(e.to_string()))?;
    }

    let mut response_status = 0u16;
    let mut response_body = Vec::new();
    let deadline = tokio::time::Instant::now() + Duration::from_secs(120);
    loop {
        let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
        if remaining.is_zero() {
            return Err(AppError::Cloud("bridge request timed out".into()));
        }
        let msg = tokio::time::timeout(remaining, socket.next())
            .await
            .map_err(|_| AppError::Cloud("bridge request timed out".into()))?;
        let Some(msg) = msg else {
            return Err(AppError::Cloud("bridge disconnected".into()));
        };
        let msg = msg.map_err(|e| AppError::Cloud(e.to_string()))?;
        let Message::Text(text) = msg else {
            continue;
        };
        let frame: RelayFrame = match serde_json::from_str(text.as_str()) {
            Ok(frame) => frame,
            Err(err) => {
                debug!("ignoring malformed bridge frame: {err}");
                continue;
            }
        };
        match frame {
            RelayFrame::Response {
                id: rid, status, ..
            } if rid == id => {
                response_status = status;
            }
            RelayFrame::ResponseChunk {
                id: rid,
                data,
                last,
            } if rid == id => {
                response_body.extend(base64_decode(&data)?);
                if last {
                    break;
                }
            }
            RelayFrame::Error {
                id: rid,
                status,
                message,
            } if rid == id => {
                return Err(AppError::Cloud(format!("{status}: {message}")));
            }
            RelayFrame::Ping { ts } => {
                let pong = RelayFrame::Pong { ts };
                let _ = socket
                    .send(Message::Text(serde_json::to_string(&pong)?.into()))
                    .await;
            }
            _ => {}
        }
    }
    let _ = socket.close(None).await;
    if !(200..300).contains(&response_status) {
        return Err(AppError::Network(format!(
            "bridge upstream returned {response_status}: {}",
            String::from_utf8_lossy(&response_body)
        )));
    }
    Ok(response_body)
}

pub fn base64_encode(data: &[u8]) -> String {
    use base64::Engine;
    base64::engine::general_purpose::STANDARD.encode(data)
}

pub fn base64_decode(data: &str) -> Result<Vec<u8>> {
    use base64::Engine;
    base64::engine::general_purpose::STANDARD
        .decode(data)
        .map_err(|e| AppError::Cloud(format!("invalid chunk: {e}")))
}

pub fn device_uri(host: &str, port: u16, printer: &str, token: &str) -> String {
    format!(
        "lanprint://{host}:{port}/{}?token={}",
        url_encode(printer),
        url_encode(token)
    )
}

pub fn cloud_device_uri(server: &str, share_code: &str, printer: &str, token: &str) -> String {
    format!(
        "lanprint-cloud://bridge?server={}&code={}&printer={}&token={}",
        url_encode(server),
        url_encode(share_code),
        url_encode(printer),
        url_encode(token)
    )
}

pub fn parse_cloud_device_uri(uri: &str) -> Option<(String, String, String, String)> {
    let query = uri.strip_prefix("lanprint-cloud://bridge?")?;
    let mut server = None;
    let mut code = None;
    let mut printer = None;
    let mut token = None;
    for (key, value) in url::form_urlencoded::parse(query.as_bytes()) {
        match key.as_ref() {
            "server" => server = Some(value.into_owned()),
            "code" => code = Some(value.into_owned()),
            "printer" => printer = Some(value.into_owned()),
            "token" => token = Some(value.into_owned()),
            _ => {}
        }
    }
    Some((server?, code?, printer?, token?))
}

pub fn parse_device_uri(uri: &str) -> Option<(String, u16, String, String)> {
    let rest = uri.strip_prefix("lanprint://")?;
    let (authority, tail) = rest.split_once('/')?;
    let (host, port) = match authority.rsplit_once(':') {
        Some((h, p)) => (h.to_string(), p.parse().ok()?),
        None => (authority.to_string(), lanprint_core::DEFAULT_API_PORT),
    };
    let (printer, token) = tail.split_once("?token=")?;
    let printer = url_decode(printer);
    let token = url_decode(token);
    Some((host, port, printer, token))
}

fn url_decode(value: &str) -> String {
    urlencoding::decode(value)
        .map(|v| v.into_owned())
        .unwrap_or_else(|_| value.to_string())
}

#[cfg(test)]
mod tests {
    use super::{cloud_device_uri, parse_cloud_device_uri, parse_device_uri};

    #[test]
    fn lan_uri_roundtrip() {
        let uri = super::device_uri("192.168.1.5", 52333, "HP LaserJet", "token+/=");
        let (host, port, printer, token) = parse_device_uri(&uri).unwrap();
        assert_eq!(host, "192.168.1.5");
        assert_eq!(port, 52333);
        assert_eq!(printer, "HP LaserJet");
        assert_eq!(token, "token+/=");
    }

    #[test]
    fn cloud_uri_roundtrip() {
        let uri = cloud_device_uri(
            "https://bridge.example.com:8443",
            "ABCD2345",
            "HP LaserJet",
            "token+/=",
        );
        let (server, code, printer, token) = parse_cloud_device_uri(&uri).unwrap();
        assert_eq!(server, "https://bridge.example.com:8443");
        assert_eq!(code, "ABCD2345");
        assert_eq!(printer, "HP LaserJet");
        assert_eq!(token, "token+/=");
    }
}
