use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum DuplexMode {
    Simplex,
    LongEdge,
    ShortEdge,
}

impl Default for DuplexMode {
    fn default() -> Self {
        Self::Simplex
    }
}

impl DuplexMode {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Simplex => "simplex",
            Self::LongEdge => "duplex_long",
            Self::ShortEdge => "duplex_short",
        }
    }
}

/// Print preferences synchronised from the host. The client stores these and
/// applies them when creating or updating its local virtual queue.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(default)]
pub struct PrinterPreferences {
    /// ISO paper name such as `iso_a4_210x297mm`, or a friendly `A4`.
    pub paper: String,
    pub color: bool,
    pub duplex: DuplexMode,
    /// Printer resolution in DPI, 0 means "printer default".
    pub resolution: u32,
    /// Optional media source / input tray.
    pub media_source: String,
    /// Extra driver options as `key=value` pairs.
    pub options: Vec<(String, String)>,
    /// Impose two pages per side, folded like a book. Requires duplex.
    pub booklet: bool,
    /// Number of copies; every value below one means "one copy".
    pub copies: u32,
}

impl Default for PrinterPreferences {
    fn default() -> Self {
        Self {
            paper: "A4".into(),
            color: true,
            duplex: DuplexMode::Simplex,
            resolution: 0,
            media_source: String::new(),
            options: Vec::new(),
            booklet: false,
            copies: 1,
        }
    }
}

impl PrinterPreferences {
    /// Copies that should actually be printed.
    pub fn copies(&self) -> u32 {
        self.copies.max(1)
    }
}

/// Capabilities discovered on the host and exposed to clients.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Default)]
#[serde(default)]
pub struct PrinterCapabilities {
    pub color: bool,
    pub duplex: bool,
    pub max_paper: String,
    pub make_model: String,
    pub supports_scan: bool,
    pub scanner_id: String,
    pub resolutions: Vec<u32>,
    /// Known supported document formats for the pass-through pipeline.
    pub formats: Vec<String>,
    /// Host-side option names that can be applied to a job.
    pub options: Vec<PrinterOption>,
    /// Paper sizes accepted by the driver, e.g. `A4`, `A3`, `Letter`.
    pub papers: Vec<String>,
    /// Input trays / media sources as reported by the driver.
    pub sources: Vec<String>,
    /// Duplex orientations the driver supports: `long_edge`, `short_edge`.
    pub duplex_modes: Vec<String>,
    /// The host can impose booklets for this printer.
    pub booklet: bool,
    /// Maximum copies the driver accepts; zero means unknown.
    pub max_copies: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct PrinterOption {
    pub name: String,
    pub label: String,
    pub values: Vec<String>,
    pub default: String,
}

/// A physical printer installed on this machine.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct LocalPrinter {
    pub name: String,
    pub driver: String,
    pub port: String,
    pub is_default: bool,
    /// `idle`, `printing`, `offline`, `error`, `unknown`.
    pub status: String,
    pub capabilities: PrinterCapabilities,
    pub preferences: PrinterPreferences,
    /// Whether this printer can be offered to remote clients.
    pub available: bool,
}

impl Default for LocalPrinter {
    fn default() -> Self {
        Self {
            name: String::new(),
            driver: String::new(),
            port: String::new(),
            is_default: false,
            status: "unknown".into(),
            capabilities: PrinterCapabilities::default(),
            preferences: PrinterPreferences::default(),
            available: true,
        }
    }
}

/// Per-printer sharing configuration stored on the host.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct SharedPrinterConfig {
    pub name: String,
    pub enabled: bool,
    /// Argon2 hash for an optional printer-specific access code.
    pub password_hash: String,
    pub preferences: PrinterPreferences,
    /// Optional friendly name shown to clients.
    pub display_name: String,
    pub shared_at: i64,
}

impl Default for SharedPrinterConfig {
    fn default() -> Self {
        Self {
            name: String::new(),
            enabled: false,
            password_hash: String::new(),
            preferences: PrinterPreferences::default(),
            display_name: String::new(),
            shared_at: 0,
        }
    }
}

impl SharedPrinterConfig {
    pub fn has_password(&self) -> bool {
        !self.password_hash.is_empty()
    }
}

/// Public view of a shared printer. Never contains the password hash.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct SharedPrinter {
    pub name: String,
    pub display_name: String,
    pub driver: String,
    pub location: String,
    pub online: bool,
    pub enabled: bool,
    pub has_password: bool,
    pub supports_scan: bool,
    pub capabilities: PrinterCapabilities,
    pub preferences: PrinterPreferences,
}

/// A host that this client has connected to.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct RemoteDevice {
    pub id: String,
    pub name: String,
    /// IP address or DNS name.
    pub address: String,
    pub port: u16,
    pub os: String,
    /// Whether the device was added through the cloud bridge.
    pub cloud: bool,
    pub share_code: String,
    /// Cached host session token. Short lived; renewed with the password.
    pub host_token: String,
    pub host_token_exp: i64,
    /// Optional password cached in memory only by the UI; never persisted.
    #[serde(skip)]
    pub password: String,
    pub online: bool,
    pub last_seen: i64,
}

impl Default for RemoteDevice {
    fn default() -> Self {
        Self {
            id: String::new(),
            name: String::new(),
            address: String::new(),
            port: crate::DEFAULT_API_PORT,
            os: String::new(),
            cloud: false,
            share_code: String::new(),
            host_token: String::new(),
            host_token_exp: 0,
            password: String::new(),
            online: false,
            last_seen: 0,
        }
    }
}

/// A virtual printer added on the client.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct ClientPrinter {
    pub id: u32,
    /// Local queue name, e.g. `lanPrint-HP_LaserJet`.
    pub local_name: String,
    pub remote_name: String,
    pub display_name: String,
    pub device_id: String,
    pub device_name: String,
    /// Signed printer-scoped token; renew with the access code when expired.
    pub token: String,
    pub token_exp: i64,
    pub supports_scan: bool,
    pub online: bool,
    pub installed: bool,
    pub preferences: PrinterPreferences,
    /// Capabilities advertised by the host, kept so the client can show which
    /// features (duplex, booklet, trays, ...) the remote printer offers.
    pub capabilities: PrinterCapabilities,
    pub last_error: String,
}

impl Default for ClientPrinter {
    fn default() -> Self {
        Self {
            id: 0,
            local_name: String::new(),
            remote_name: String::new(),
            display_name: String::new(),
            device_id: String::new(),
            device_name: String::new(),
            token: String::new(),
            token_exp: 0,
            supports_scan: false,
            online: false,
            installed: false,
            preferences: PrinterPreferences::default(),
            capabilities: PrinterCapabilities::default(),
            last_error: String::new(),
        }
    }
}

/// Cloud bridge settings. The host registers itself with the bridge using the
/// server access code and receives a share code that clients can enter.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct CloudConfig {
    pub enabled: bool,
    /// `https://bridge.example.com:8443` or `http://192.168.1.10:8080`.
    pub server_url: String,
    /// Shared secret configured on the bridge (`LANPRINT_BRIDGE_AUTH_CODE`).
    pub server_code: String,
    /// Code returned by the bridge. Clients only need this code.
    pub share_code: String,
    /// Token issued by the bridge for the host WebSocket.
    pub host_token: String,
    pub host_token_exp: i64,
    /// Keep the relay connection open.
    pub connected: bool,
    pub last_error: String,
    pub last_connected_at: i64,
}

impl Default for CloudConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            server_url: String::new(),
            server_code: String::new(),
            share_code: String::new(),
            host_token: String::new(),
            host_token_exp: 0,
            connected: false,
            last_error: String::new(),
            last_connected_at: 0,
        }
    }
}

/// A client-side connection to a cloud host.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct CloudTarget {
    pub device_id: String,
    pub share_code: String,
    pub server_url: String,
    pub address_hint: String,
    pub port_hint: u16,
    pub online: bool,
}

impl Default for CloudTarget {
    fn default() -> Self {
        Self {
            device_id: String::new(),
            share_code: String::new(),
            server_url: String::new(),
            address_hint: String::new(),
            port_hint: 0,
            online: false,
        }
    }
}

/// A discovered lanPrint host (mDNS or manual entry).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct DiscoveredDevice {
    pub id: String,
    pub name: String,
    pub address: String,
    pub port: u16,
    pub os: String,
    pub version: String,
    pub cloud: bool,
    pub requires_password: bool,
    pub shared_count: u32,
}

impl Default for DiscoveredDevice {
    fn default() -> Self {
        Self {
            id: String::new(),
            name: String::new(),
            address: String::new(),
            port: crate::DEFAULT_API_PORT,
            os: String::new(),
            version: String::new(),
            cloud: false,
            requires_password: true,
            shared_count: 0,
        }
    }
}

/// Job record used by the UI and by `/api/v1/jobs`.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct JobRecord {
    pub id: String,
    pub printer: String,
    pub source: String,
    pub filename: String,
    pub bytes: u64,
    pub status: String,
    pub message: String,
    pub created_at: i64,
    pub finished_at: i64,
}

/// Scan request accepted by a host.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct ScanRequest {
    pub scanner_id: String,
    /// 75..1200
    pub resolution: u32,
    pub color: bool,
    /// `png`, `jpeg` or `pdf`
    pub format: String,
    /// `flatbed` or `adf`
    pub source: String,
    /// `A4`, `Letter`, ...
    pub paper: String,
}

impl Default for ScanRequest {
    fn default() -> Self {
        Self {
            scanner_id: String::new(),
            resolution: 300,
            color: true,
            format: "png".into(),
            source: "flatbed".into(),
            paper: "A4".into(),
        }
    }
}

/// Scan capabilities returned to the client.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Default)]
pub struct ScanCapabilities {
    pub available: bool,
    pub scanner_id: String,
    pub scanner_name: String,
    pub resolutions: Vec<u32>,
    pub color: bool,
    pub adf: bool,
}

/// Language identifiers supported by the desktop app and the web site.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Language {
    ZhCn,
    ZhTw,
    En,
    Fr,
    De,
    It,
    Es,
    Ja,
    Ko,
}

impl Language {
    pub const ALL: [Language; 9] = [
        Language::ZhCn,
        Language::ZhTw,
        Language::En,
        Language::Fr,
        Language::De,
        Language::It,
        Language::Es,
        Language::Ja,
        Language::Ko,
    ];

    pub fn code(self) -> &'static str {
        match self {
            Self::ZhCn => "zh-CN",
            Self::ZhTw => "zh-TW",
            Self::En => "en-US",
            Self::Fr => "fr-FR",
            Self::De => "de-DE",
            Self::It => "it-IT",
            Self::Es => "es-ES",
            Self::Ja => "ja-JP",
            Self::Ko => "ko-KR",
        }
    }

    pub fn native_name(self) -> &'static str {
        match self {
            Self::ZhCn => "简体中文",
            Self::ZhTw => "繁體中文",
            Self::En => "English",
            Self::Fr => "Français",
            Self::De => "Deutsch",
            Self::It => "Italiano",
            Self::Es => "Español",
            Self::Ja => "日本語",
            Self::Ko => "한국어",
        }
    }

    pub fn from_code(code: &str) -> Option<Self> {
        let lower = code.replace('_', "-").to_ascii_lowercase();
        let lang = lower.split('-').next().unwrap_or("");
        match lower.as_str() {
            "zh-cn" | "zh-hans" | "zh-sg" => Some(Self::ZhCn),
            "zh-tw" | "zh-hk" | "zh-mo" | "zh-hant" => Some(Self::ZhTw),
            "en" | "en-us" | "en-gb" => Some(Self::En),
            "fr" | "fr-fr" | "fr-ca" => Some(Self::Fr),
            "de" | "de-de" | "de-at" => Some(Self::De),
            "it" | "it-it" => Some(Self::It),
            "es" | "es-es" | "es-mx" => Some(Self::Es),
            "ja" | "ja-jp" => Some(Self::Ja),
            "ko" | "ko-kr" => Some(Self::Ko),
            _ => match lang {
                "zh" => Some(Self::ZhCn),
                "en" => Some(Self::En),
                "fr" => Some(Self::Fr),
                "de" => Some(Self::De),
                "it" => Some(Self::It),
                "es" => Some(Self::Es),
                "ja" => Some(Self::Ja),
                "ko" => Some(Self::Ko),
                _ => None,
            },
        }
    }
}
