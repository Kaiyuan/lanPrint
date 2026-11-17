use serde::{Deserialize, Serialize};

use crate::model::{
    JobRecord, PrinterCapabilities, PrinterPreferences, ScanCapabilities, ScanRequest,
    SharedPrinter,
};

pub const API_PREFIX: &str = "/api/v1";

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct InfoResponse {
    pub device_id: String,
    pub device_name: String,
    pub version: String,
    pub os: String,
    pub api_port: u16,
    pub requires_password: bool,
    pub shared_count: u32,
    pub supports_scan: bool,
    pub cloud_enabled: bool,
    pub cloud_share_code: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AuthRequest {
    pub password: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AuthResponse {
    pub token: String,
    pub expires_at: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PrinterTokenRequest {
    pub password: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PrinterTokenResponse {
    pub token: String,
    pub expires_at: i64,
    pub local_password_required: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PrintersResponse {
    pub printers: Vec<SharedPrinter>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StatusResponse {
    pub online: bool,
    pub status: String,
    pub message: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PrintResult {
    pub job_id: String,
    pub status: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct JobsResponse {
    pub jobs: Vec<JobRecord>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ScanResponseMeta {
    pub job_id: String,
    pub format: String,
    pub width: u32,
    pub height: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ErrorResponse {
    pub error: String,
    pub message: String,
    #[serde(default)]
    pub code: String,
}

impl ErrorResponse {
    pub fn new(error: impl Into<String>, message: impl Into<String>) -> Self {
        Self {
            error: error.into(),
            message: message.into(),
            code: String::new(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct PrinterUpdate {
    pub display_name: Option<String>,
    pub preferences: Option<PrinterPreferences>,
    pub capabilities: Option<PrinterCapabilities>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct ScanOptions {
    #[serde(flatten)]
    pub request: ScanRequest,
    pub capabilities: Option<ScanCapabilities>,
}

/// Cloud bridge: host registration request. The bridge authenticates the
/// request with the server access code and returns a stable share code.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CloudRegisterRequest {
    pub device_id: String,
    pub device_name: String,
    pub version: String,
    #[serde(default)]
    pub api_port: u16,
    #[serde(default)]
    pub old_share_code: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CloudRegisterResponse {
    pub share_code: String,
    pub host_token: String,
    pub expires_at: i64,
    pub server_version: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CloudConnectRequest {
    pub share_code: String,
    #[serde(default)]
    pub device_id: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CloudConnectResponse {
    pub session_token: String,
    pub device_id: String,
    pub device_name: String,
    pub server_url: String,
    pub address_hint: String,
    pub port_hint: u16,
    pub expires_at: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CloudError {
    pub error: String,
    pub message: String,
}

/// Frames multiplexed over the bridge WebSocket. Request/response bodies are
/// chunked and base64 encoded so that a single WebSocket can carry large print
/// jobs and scans without interfering with signalling frames.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "t", rename_all = "snake_case")]
pub enum RelayFrame {
    Hello {
        role: String,
        device_id: String,
        name: String,
    },
    Request {
        id: u64,
        method: String,
        path: String,
        headers: Vec<(String, String)>,
        content_length: u64,
    },
    RequestChunk {
        id: u64,
        data: String,
        last: bool,
    },
    Response {
        id: u64,
        status: u16,
        headers: Vec<(String, String)>,
        content_length: u64,
    },
    ResponseChunk {
        id: u64,
        data: String,
        last: bool,
    },
    Error {
        id: u64,
        status: u16,
        message: String,
    },
    Ping {
        ts: i64,
    },
    Pong {
        ts: i64,
    },
    Bye {
        reason: String,
    },
}

pub fn os_name() -> &'static str {
    std::env::consts::OS
}

/// Split a `host:port` string, accepting bare IPv6 addresses by requiring
/// bracketed notation when a port is present.
pub fn parse_address(input: &str, default_port: u16) -> Option<(String, u16)> {
    let input = input.trim().trim_end_matches('/');
    if input.is_empty() {
        return None;
    }
    let without_scheme = input
        .strip_prefix("http://")
        .or_else(|| input.strip_prefix("https://"))
        .unwrap_or(input);
    if let Some(rest) = without_scheme.strip_prefix('[') {
        let (host, tail) = rest.split_once(']')?;
        let port = tail
            .strip_prefix(':')
            .and_then(|p| p.parse().ok())
            .unwrap_or(default_port);
        return Some((host.to_string(), port));
    }
    if let Some((host, port)) = without_scheme.rsplit_once(':') {
        if let Ok(port) = port.parse::<u16>() {
            if !host.contains(':') {
                return Some((host.to_string(), port));
            }
        }
    }
    Some((without_scheme.to_string(), default_port))
}

#[cfg(test)]
mod tests {
    use super::parse_address;

    #[test]
    fn parses_addresses() {
        assert_eq!(
            parse_address("192.168.1.2", 52333),
            Some(("192.168.1.2".into(), 52333))
        );
        assert_eq!(
            parse_address("host:9000", 52333),
            Some(("host".into(), 9000))
        );
        assert_eq!(
            parse_address("https://bridge.example.com:8443/", 52333),
            Some(("bridge.example.com".into(), 8443))
        );
        assert_eq!(
            parse_address("[::1]:1234", 52333),
            Some(("::1".into(), 1234))
        );
    }
}
