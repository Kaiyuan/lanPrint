use std::collections::{HashMap, VecDeque};
use std::sync::mpsc::Sender;
use std::sync::{Arc, RwLock};

use lanprint_core::auth::{
    authorize_host, authorize_printer, decode_secret, issue_token, PasswordHash, TokenClaims,
    HOST_TOKEN_TTL, PRINTER_TOKEN_TTL,
};
use lanprint_core::{
    discovery, AppConfig, ClientPrinter, ConfigStore, DiscoveredDevice, DuplexMode, InfoResponse,
    JobRecord, LocalPrinter, PrinterPreferences, RemoteDevice, ScanRequest, SharedPrinter,
    SharedPrinterConfig,
};
use mdns_sd::ServiceDaemon;
use tokio::sync::Mutex;
use tracing::{debug, warn};

use crate::client::{self, HostLink};
use crate::error::{AppError, Result};
use crate::forwarder::Forwarder;
use crate::platform;

#[derive(Debug, Clone)]
pub enum AppEvent {
    Refresh,
    Toast(String),
    ShowWindow,
    Quit,
}

#[derive(Debug, Clone)]
pub struct AvailableUpdate {
    pub version: String,
    pub page_url: String,
    pub assets: Vec<(String, String)>,
}

#[derive(Default)]
pub struct Runtime {
    pub local_printers: Vec<LocalPrinter>,
    pub remote_printers: HashMap<String, Vec<SharedPrinter>>,
    pub jobs: VecDeque<JobRecord>,
    pub cloud_status: String,
    pub busy: bool,
}

pub struct App {
    pub config: Arc<ConfigStore>,
    pub http: reqwest::Client,
    state: RwLock<Runtime>,
    events: Sender<AppEvent>,
    pub forwarders: Mutex<HashMap<u32, Forwarder>>,
    mdns: Mutex<Option<ServiceDaemon>>,
    cloud_task: Mutex<Option<tokio::task::JoinHandle<()>>>,
    relay_tx: Mutex<Option<tokio::sync::mpsc::UnboundedSender<lanprint_core::RelayFrame>>>,
    activity: crate::activity::ActivityLog,
    update: std::sync::Mutex<Option<AvailableUpdate>>,
}

impl App {
    pub fn new(config: Arc<ConfigStore>, events: Sender<AppEvent>) -> Result<Arc<Self>> {
        let http = reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(20))
            .connect_timeout(std::time::Duration::from_secs(5))
            .user_agent(format!("lanPrint/{}", lanprint_core::VERSION))
            .build()
            .map_err(AppError::from)?;
        {
            let cfg = config.read();
            crate::client::set_identity(&cfg.device_id, &cfg.device_name);
        }
        Ok(Arc::new(Self {
            activity: crate::activity::ActivityLog::load(&config.config_dir()),
            update: std::sync::Mutex::new(None),
            config,
            http,
            state: RwLock::new(Runtime::default()),
            events,
            forwarders: Mutex::new(HashMap::new()),
            mdns: Mutex::new(None),
            cloud_task: Mutex::new(None),
            relay_tx: Mutex::new(None),
        }))
    }

    pub fn emit(&self, event: AppEvent) {
        let _ = self.events.send(event);
    }

    pub fn activity(&self) -> Vec<crate::activity::ActivityEntry> {
        self.activity.entries()
    }

    pub fn record_activity(&self, entry: crate::activity::ActivityEntry) {
        self.activity.record(entry);
        self.emit(AppEvent::Refresh);
    }

    pub fn available_update(&self) -> Option<AvailableUpdate> {
        self.update.lock().ok().and_then(|update| update.clone())
    }

    /// Client printer by its numeric id, used by the IPP endpoint.
    pub fn client_printer(&self, id: u32) -> Option<ClientPrinter> {
        self.config
            .read()
            .client_printers
            .iter()
            .find(|printer| printer.id == id)
            .cloned()
    }

    /// Ensure existing client queues use the IPP endpoint. Runs silently on
    /// Windows; on macOS/Linux it may request administrator rights, so the UI
    /// calls it only after an explicit user action.
    pub async fn upgrade_client_queues(self: &Arc<Self>, force: bool) {
        if !force && !cfg!(windows) && std::env::args().any(|arg| arg == "--tray") {
            return;
        }
        let printers = self.config.read().client_printers.clone();
        for printer in printers {
            if upgraded_queues()
                .lock()
                .map(|done| done.contains(&printer.id))
                .unwrap_or(false)
            {
                continue;
            }
            let name = printer.local_name.clone();
            let id = printer.id;
            match tokio::task::spawn_blocking(move || platform::upgrade_ipp_queue(&name, id)).await
            {
                Ok(Ok(())) => {
                    if let Ok(mut done) = upgraded_queues().lock() {
                        done.insert(id);
                    }
                    debug!("queue {} uses IPP", printer.local_name);
                }
                Ok(Err(err)) => debug!("IPP upgrade for {} failed: {err}", printer.local_name),
                Err(err) => debug!("IPP upgrade task for {} failed: {err}", printer.local_name),
            }
        }
    }

    pub fn set_auto_update(self: &Arc<Self>, enabled: bool) {
        self.config.edit(|cfg| cfg.auto_update = enabled);
        if enabled {
            let app = self.clone();
            crate::runtime::spawn(async move {
                if let Err(err) = app.check_updates(false).await {
                    debug!("update check failed: {err}");
                }
            });
        }
    }

    /// Ask GitHub for the latest release. `force` bypasses the automatic
    /// update preference (used by the manual check button).
    pub async fn check_updates(self: &Arc<Self>, force: bool) -> Result<()> {
        if !force && !self.config.read().auto_update {
            return Ok(());
        }
        let response = self
            .http
            .get("https://api.github.com/repos/Kaiyuan/lanPrint/releases/latest")
            .header(reqwest::header::ACCEPT, "application/vnd.github+json")
            .send()
            .await
            .map_err(|e| AppError::Network(format!("update check: {e}")))?;
        if !response.status().is_success() {
            return Err(AppError::Network(format!(
                "update check returned {}",
                response.status()
            )));
        }
        let value: serde_json::Value = response
            .json()
            .await
            .map_err(|e| AppError::Network(format!("update response: {e}")))?;
        let version = value
            .get("tag_name")
            .and_then(|tag| tag.as_str())
            .unwrap_or_default()
            .trim_start_matches('v')
            .trim()
            .to_string();
        let page_url = value
            .get("html_url")
            .and_then(|url| url.as_str())
            .unwrap_or("https://github.com/Kaiyuan/lanPrint/releases")
            .to_string();
        let assets: Vec<(String, String)> = value
            .get("assets")
            .and_then(|assets| assets.as_array())
            .map(|assets| {
                assets
                    .iter()
                    .filter_map(|asset| {
                        let name = asset.get("name")?.as_str()?.to_string();
                        let url = asset.get("browser_download_url")?.as_str()?.to_string();
                        Some((name, url))
                    })
                    .collect()
            })
            .unwrap_or_default();
        let available = if version_is_newer(&version, lanprint_core::VERSION) {
            Some(AvailableUpdate {
                version,
                page_url,
                assets,
            })
        } else {
            None
        };
        if let Ok(mut update) = self.update.lock() {
            *update = available;
        }
        self.emit(AppEvent::Refresh);
        Ok(())
    }

    /// Download the release asset and hand it to the platform installer.
    pub async fn install_update(self: &Arc<Self>) -> Result<()> {
        let update = self
            .available_update()
            .ok_or_else(|| AppError::Network("no update available".into()))?;
        let Some((name, url)) = update_asset(&update.version, &update.assets) else {
            // Fall back to the release page when no asset matches this build.
            open::that(&update.page_url)
                .map_err(|e| AppError::Platform(format!("cannot open release page: {e}")))?;
            return Ok(());
        };
        let dir = std::env::temp_dir().join("lanPrint-updates");
        std::fs::create_dir_all(&dir)
            .map_err(|e| AppError::Platform(format!("update folder: {e}")))?;
        let path = dir.join(&name);
        let client = reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(900))
            .build()
            .map_err(AppError::from)?;
        let response = client
            .get(&url)
            .send()
            .await
            .map_err(|e| AppError::Network(format!("download update: {e}")))?;
        if !response.status().is_success() {
            return Err(AppError::Network(format!(
                "download update returned {}",
                response.status()
            )));
        }
        let data = response
            .bytes()
            .await
            .map_err(|e| AppError::Network(format!("download update: {e}")))?;
        std::fs::write(&path, &data)
            .map_err(|e| AppError::Platform(format!("save update: {e}")))?;
        platform::launch_update(&path)?;
        #[cfg(windows)]
        {
            // The Inno Setup installer closes the running app before replacing
            // files; give it a moment to start and then exit cleanly.
            tokio::time::sleep(std::time::Duration::from_secs(1)).await;
            self.emit(AppEvent::Quit);
        }
        Ok(())
    }

    pub fn snapshot(&self) -> RuntimeSnapshot {
        let state = self.state.read().unwrap();
        RuntimeSnapshot {
            local_printers: state.local_printers.clone(),
            remote_printers: state.remote_printers.clone(),
            jobs: state.jobs.iter().cloned().collect(),
            cloud_status: state.cloud_status.clone(),
            busy: state.busy,
        }
    }

    pub fn set_busy(&self, busy: bool) {
        self.state.write().unwrap().busy = busy;
        self.emit(AppEvent::Refresh);
    }

    /// Start background services: restore virtual queues, advertise mDNS and
    /// begin the periodic status refresh loop.
    pub async fn initialize(self: &Arc<Self>) {
        self.refresh_local().await;
        self.restore_forwarders().await;
        self.refresh_mdns().await;
        self.refresh_statuses().await;
        let devices = self.config.read().remote_devices.clone();
        for device in devices {
            let _ = self.refresh_device(&device.id).await;
        }
        self.start_cloud_host().await;

        let ipp = self.clone();
        tokio::spawn(async move {
            if let Err(err) = crate::ipp::serve(ipp).await {
                warn!("IPP endpoint unavailable: {err}");
            }
        });

        if cfg!(windows) || !std::env::args().any(|arg| arg == "--tray") {
            let upgrader = self.clone();
            tokio::spawn(async move {
                tokio::time::sleep(std::time::Duration::from_secs(1)).await;
                upgrader.upgrade_client_queues(false).await;
            });
        }

        let updater = self.clone();
        tokio::spawn(async move {
            tokio::time::sleep(std::time::Duration::from_secs(5)).await;
            loop {
                if updater.config.read().auto_update {
                    if let Err(err) = updater.check_updates(false).await {
                        debug!("update check failed: {err}");
                    }
                }
                tokio::time::sleep(std::time::Duration::from_secs(6 * 3600)).await;
            }
        });

        let app = self.clone();
        tokio::spawn(async move {
            let mut tick = 0u64;
            loop {
                tokio::time::sleep(std::time::Duration::from_secs(10)).await;
                app.refresh_statuses().await;
                tick += 1;
                if tick % 6 == 0 {
                    app.refresh_local().await;
                }
                if tick % 12 == 0 {
                    app.refresh_mdns().await;
                }
            }
        });
    }

    pub async fn refresh_local(self: &Arc<Self>) {
        let result = tokio::task::spawn_blocking(platform::list_printers).await;
        match result {
            Ok(Ok(mut printers)) => {
                let cfg = self.config.read();
                for p in &mut printers {
                    if let Some(shared) = cfg.shared_printer(&p.name) {
                        p.preferences = shared.preferences.clone();
                    }
                }
                self.state.write().unwrap().local_printers = printers;
                self.emit(AppEvent::Refresh);
            }
            Ok(Err(err)) => warn!("printer enumeration failed: {err}"),
            Err(err) => warn!("printer enumeration task failed: {err}"),
        }
    }

    pub fn shared_printers(&self) -> Vec<SharedPrinter> {
        let cfg = self.config.read();
        self.shared_printers_for(&cfg)
    }

    fn shared_printers_for(&self, cfg: &AppConfig) -> Vec<SharedPrinter> {
        let local = self.state.read().unwrap().local_printers.clone();
        let mut out = Vec::new();
        for shared in cfg.shared_printers.iter().filter(|p| p.enabled) {
            let Some(printer) = local.iter().find(|p| p.name == shared.name) else {
                continue;
            };
            out.push(SharedPrinter {
                name: shared.name.clone(),
                display_name: if shared.display_name.is_empty() {
                    shared.name.clone()
                } else {
                    shared.display_name.clone()
                },
                driver: printer.driver.clone(),
                location: String::new(),
                online: printer.status != "offline",
                enabled: shared.enabled,
                has_password: shared.has_password(),
                supports_scan: printer.capabilities.supports_scan,
                capabilities: printer.capabilities.clone(),
                preferences: shared.preferences.clone(),
            });
        }
        out
    }

    pub fn info(&self) -> InfoResponse {
        let cfg = self.config.read();
        let printers = self.shared_printers_for(&cfg);
        InfoResponse {
            device_id: cfg.device_id.clone(),
            device_name: cfg.device_name.clone(),
            version: lanprint_core::VERSION.to_string(),
            os: std::env::consts::OS.to_string(),
            api_port: cfg.api_port,
            requires_password: cfg.has_host_password(),
            shared_count: printers.len() as u32,
            supports_scan: printers.iter().any(|p| p.supports_scan),
            cloud_enabled: cfg.cloud.enabled,
            cloud_share_code: cfg.cloud.share_code.clone(),
        }
    }

    // ---- authentication -------------------------------------------------

    pub fn verify_host_password(&self, password: &str) -> bool {
        self.config.read().verify_host_password(password)
    }

    pub fn issue_host_token(&self) -> Result<(String, i64)> {
        let cfg = self.config.read();
        if !cfg.has_host_password() {
            return Err(AppError::Auth("host access code is not configured".into()));
        }
        let secret = decode_secret(&cfg.host_secret);
        let claims = TokenClaims::host(&cfg.device_id, HOST_TOKEN_TTL);
        Ok((issue_token(&secret, &claims), claims.exp))
    }

    fn identity(&self) -> (String, String) {
        let cfg = self.config.read();
        (cfg.device_id.clone(), cfg.device_name.clone())
    }

    pub fn issue_printer_token(&self, printer: &str, client_name: &str) -> Result<(String, i64)> {
        let cfg = self.config.read();
        let secret = decode_secret(&cfg.host_secret);
        let mut claims = TokenClaims::printer(&cfg.device_id, printer, PRINTER_TOKEN_TTL);
        claims.dname = client_name.to_string();
        Ok((issue_token(&secret, &claims), claims.exp))
    }

    pub fn authorize_host_token(&self, token: &str) -> Result<TokenClaims> {
        let cfg = self.config.read();
        let secret = decode_secret(&cfg.host_secret);
        authorize_host(&secret, token, &cfg.device_id).map_err(AppError::from)
    }

    pub fn authorize_printer_token(&self, token: &str, printer: &str) -> Result<TokenClaims> {
        let cfg = self.config.read();
        let secret = decode_secret(&cfg.host_secret);
        authorize_printer(&secret, token, &cfg.device_id, printer).map_err(AppError::from)
    }

    pub fn verify_printer_password(&self, printer: &str, password: &str) -> bool {
        let cfg = self.config.read();
        let Some(shared) = cfg.shared_printer(printer) else {
            return false;
        };
        if shared.password_hash.is_empty() {
            return true;
        }
        PasswordHash::from_string(&shared.password_hash)
            .map(|h| h.verify(password))
            .unwrap_or(false)
    }

    // ---- host settings --------------------------------------------------

    pub async fn set_host_password(self: &Arc<Self>, password: String) -> Result<()> {
        if password.trim().len() < 4 {
            return Err(AppError::Auth(
                "access code must be at least 4 characters".into(),
            ));
        }
        self.config.edit(|cfg| cfg.set_host_password(&password))?;
        self.refresh_mdns().await;
        self.emit(AppEvent::Toast("password_saved".into()));
        self.emit(AppEvent::Refresh);
        Ok(())
    }

    pub async fn share_printer(
        self: &Arc<Self>,
        name: String,
        enabled: bool,
        password: Option<String>,
        preferences: Option<PrinterPreferences>,
    ) -> Result<()> {
        let prefs = preferences.or_else(|| {
            self.state
                .read()
                .unwrap()
                .local_printers
                .iter()
                .find(|p| p.name == name)
                .map(|p| p.preferences.clone())
        });
        self.config
            .edit(|cfg| cfg.upsert_shared_printer(&name, enabled, password.as_deref(), prefs))?;
        self.refresh_mdns().await;
        self.emit(AppEvent::Refresh);
        Ok(())
    }

    pub async fn set_printer_password(
        self: &Arc<Self>,
        name: String,
        password: String,
    ) -> Result<()> {
        self.config
            .edit(|cfg| cfg.set_printer_password(&name, &password))?;
        self.emit(AppEvent::Refresh);
        Ok(())
    }

    pub fn shared_config(&self, name: &str) -> Option<SharedPrinterConfig> {
        self.config.read().shared_printer(name).cloned()
    }

    /// Capabilities of a local printer as last enumerated by the spooler.
    pub fn printer_capabilities(&self, name: &str) -> lanprint_core::PrinterCapabilities {
        self.state
            .read()
            .unwrap()
            .local_printers
            .iter()
            .find(|p| p.name == name)
            .map(|p| p.capabilities.clone())
            .unwrap_or_default()
    }

    // ---- mDNS -----------------------------------------------------------

    pub async fn refresh_mdns(self: &Arc<Self>) {
        let cfg = self.config.read();
        let enabled: Vec<_> = cfg.shared_printers.iter().filter(|p| p.enabled).collect();
        let should_advertise = cfg.has_host_password() && !enabled.is_empty();
        let device_id = cfg.device_id.clone();
        let device_name = cfg.device_name.clone();
        let port = cfg.api_port;
        let cloud = cfg.cloud.enabled;
        let requires_password = cfg.has_host_password();
        let count = enabled.len() as u32;
        drop(cfg);

        let mut guard = self.mdns.lock().await;
        if let Some(daemon) = guard.take() {
            let name = self.config.read().device_name.clone();
            let _ = tokio::task::spawn_blocking(move || {
                discovery::unregister(daemon, &name);
            })
            .await;
        }
        if !should_advertise {
            return;
        }
        let result = tokio::task::spawn_blocking(move || {
            discovery::advertise(
                &device_id,
                &device_name,
                port,
                cloud,
                requires_password,
                count,
            )
        })
        .await;
        match result {
            Ok(Ok(daemon)) => {
                *guard = Some(daemon);
                debug!("mDNS advertisement active");
            }
            Ok(Err(err)) => warn!("mDNS advertise failed: {err}"),
            Err(err) => warn!("mDNS advertise task failed: {err}"),
        }
    }

    pub async fn scan_lan(self: &Arc<Self>) -> Vec<DiscoveredDevice> {
        self.set_busy(true);
        let result =
            tokio::task::spawn_blocking(|| discovery::browse(std::time::Duration::from_secs(5)))
                .await
                .unwrap_or_default();
        let own_id = self.config.read().device_id.clone();
        let mut list: Vec<_> = result.into_iter().filter(|d| d.id != own_id).collect();
        list.sort_by(|a, b| a.name.to_lowercase().cmp(&b.name.to_lowercase()));
        self.set_busy(false);
        list
    }

    // ---- client: devices ------------------------------------------------

    pub async fn add_device(
        self: &Arc<Self>,
        name_hint: String,
        address: String,
        password: String,
        cloud: Option<(String, String)>,
    ) -> Result<RemoteDevice> {
        self.set_busy(true);
        let result = self
            .add_device_inner(name_hint, address, password, cloud)
            .await;
        self.set_busy(false);
        result
    }

    async fn add_device_inner(
        self: &Arc<Self>,
        name_hint: String,
        address: String,
        password: String,
        cloud: Option<(String, String)>,
    ) -> Result<RemoteDevice> {
        let (link, address, port, cloud_flag, share_code) = if let Some((server, code)) = cloud {
            let link = HostLink::Cloud {
                server: server.clone(),
                share_code: code.clone(),
            };
            (link, server, 0, true, code)
        } else {
            let (host, port) =
                lanprint_core::parse_address(&address, lanprint_core::DEFAULT_API_PORT)
                    .ok_or_else(|| AppError::Network("invalid address".into()))?;
            (
                HostLink::lan(&host, port, false),
                host,
                port,
                false,
                String::new(),
            )
        };

        let info = link.info(&self.http).await?;
        let (client_id, client_name) = self.identity();
        let auth = link
            .auth(&self.http, &password, &client_id, &client_name)
            .await
            .map_err(|err| {
                if err.to_string().contains("401") {
                    AppError::Auth("wrong_password".into())
                } else {
                    err
                }
            })?;

        let device = RemoteDevice {
            id: info.device_id.clone(),
            name: if name_hint.trim().is_empty() {
                info.device_name.clone()
            } else {
                name_hint.trim().to_string()
            },
            address,
            port,
            os: info.os.clone(),
            cloud: cloud_flag,
            share_code,
            host_token: auth.token,
            host_token_exp: auth.expires_at,
            password,
            online: true,
            last_seen: chrono::Utc::now().timestamp(),
        };
        self.config.edit(|cfg| {
            if let Some(existing) = cfg.remote_devices.iter_mut().find(|d| d.id == device.id) {
                *existing = device.clone();
            } else {
                cfg.remote_devices.push(device.clone());
            }
        });
        self.refresh_device(&device.id).await?;
        self.emit(AppEvent::Refresh);
        Ok(device)
    }

    pub async fn refresh_device(self: &Arc<Self>, device_id: &str) -> Result<()> {
        let device = self
            .config
            .read()
            .remote_devices
            .iter()
            .find(|d| d.id == device_id)
            .cloned()
            .ok_or_else(|| AppError::Network("device not found".into()))?;
        let link = device_link(&device);
        let info = tokio::time::timeout(std::time::Duration::from_secs(8), link.info(&self.http))
            .await
            .map_err(|_| AppError::Network("device timed out".into()))??;
        let mut printers = Vec::new();
        let mut token = device.host_token.clone();
        if token.is_empty() || device.host_token_exp < chrono::Utc::now().timestamp() {
            if !device.password.is_empty() {
                let (client_id, client_name) = self.identity();
                if let Ok(auth) = link
                    .auth(&self.http, &device.password, &client_id, &client_name)
                    .await
                {
                    token = auth.token.clone();
                    self.config.edit(|cfg| {
                        if let Some(d) = cfg.remote_devices.iter_mut().find(|d| d.id == device_id) {
                            d.host_token = auth.token;
                            d.host_token_exp = auth.expires_at;
                        }
                    });
                }
            }
        }
        if !token.is_empty() {
            if let Ok(list) = link.printers(&self.http, &token).await {
                printers = list;
            }
        }
        self.config.edit(|cfg| {
            if let Some(d) = cfg.remote_devices.iter_mut().find(|d| d.id == device_id) {
                d.online = true;
                d.last_seen = chrono::Utc::now().timestamp();
                d.name = if d.name.is_empty() {
                    info.device_name.clone()
                } else {
                    d.name.clone()
                };
                d.os = info.os.clone();
            }
            for cp in &mut cfg.client_printers {
                if cp.device_id == device_id {
                    cp.online = true;
                    if let Some(shared) = printers
                        .iter()
                        .find(|p| p.name == cp.remote_name && p.enabled)
                    {
                        cp.capabilities = shared.capabilities.clone();
                        cp.supports_scan = shared.supports_scan;
                    }
                }
            }
        });
        self.state
            .write()
            .unwrap()
            .remote_printers
            .insert(device_id.to_string(), printers);
        self.emit(AppEvent::Refresh);
        Ok(())
    }

    pub async fn remove_device(self: &Arc<Self>, device_id: String) -> Result<()> {
        let to_remove: Vec<ClientPrinter> = self
            .config
            .read()
            .client_printers
            .iter()
            .filter(|p| p.device_id == device_id)
            .cloned()
            .collect();
        for printer in to_remove {
            let _ = self.remove_printer(printer.local_name.clone()).await;
        }
        self.config.edit(|cfg| {
            cfg.remote_devices.retain(|d| d.id != device_id);
        });
        self.emit(AppEvent::Refresh);
        Ok(())
    }

    pub async fn refresh_statuses(self: &Arc<Self>) {
        let devices = self.config.read().remote_devices.clone();
        for device in devices {
            if self.refresh_device(&device.id).await.is_err() {
                self.config.edit(|cfg| {
                    if let Some(d) = cfg.remote_devices.iter_mut().find(|d| d.id == device.id) {
                        d.online = false;
                    }
                    for cp in &mut cfg.client_printers {
                        if cp.device_id == device.id {
                            cp.online = false;
                        }
                    }
                });
            }
        }
        self.emit(AppEvent::Refresh);
    }

    // ---- client: printers ----------------------------------------------

    pub async fn add_remote_printer(
        self: &Arc<Self>,
        device_id: String,
        printer_name: String,
        password: String,
    ) -> Result<ClientPrinter> {
        self.set_busy(true);
        let result = self
            .add_remote_printer_inner(device_id, printer_name, password)
            .await;
        self.set_busy(false);
        result
    }

    async fn add_remote_printer_inner(
        self: &Arc<Self>,
        device_id: String,
        printer_name: String,
        password: String,
    ) -> Result<ClientPrinter> {
        let device = self
            .config
            .read()
            .remote_devices
            .iter()
            .find(|d| d.id == device_id)
            .cloned()
            .ok_or_else(|| AppError::Network("device not found".into()))?;
        let link = device_link(&device);
        let host_token = device.host_token.clone();
        if host_token.is_empty() {
            return Err(AppError::Auth("token_expired".into()));
        }
        let printers = link.printers(&self.http, &host_token).await?;
        let shared = printers
            .iter()
            .find(|p| p.name == printer_name)
            .ok_or_else(|| AppError::Network("printer is not shared".into()))?;
        if shared.has_password && password.is_empty() {
            return Err(AppError::Auth("printer password required".into()));
        }
        let (client_id, client_name) = self.identity();
        let printer_token = link
            .printer_token(
                &self.http,
                &host_token,
                &printer_name,
                &password,
                &client_id,
                &client_name,
            )
            .await?;

        let cfg = self.config.read().clone();
        if let Some(existing) = cfg
            .client_printers
            .iter()
            .find(|p| p.device_id == device_id && p.remote_name == printer_name)
        {
            return Ok(existing.clone());
        }
        let id = cfg.client_printers.iter().map(|p| p.id).max().unwrap_or(0) + 1;
        let local_name = unique_local_name(&cfg, &device.name, &shared.display_name, id);
        let display = shared.display_name.clone();
        let prefs = shared.preferences.clone();
        let capabilities = shared.capabilities.clone();
        let supports_scan = shared.supports_scan;
        // Native driver output (PCL/PostScript) is passed through to a Windows
        // host unchanged. Other hosts print PDF through CUPS, so only reuse the
        // host driver when the host itself runs Windows.
        let queue_driver = if device.os.eq_ignore_ascii_case("windows") {
            shared.driver.clone()
        } else {
            String::new()
        };
        let tcp_port = 19_100u16.saturating_add(id as u16 % 800);

        let uri = if device.cloud {
            client::cloud_device_uri(
                &device.address,
                &device.share_code,
                &printer_name,
                &printer_token.token,
            )
        } else {
            client::device_uri(
                &device.address,
                device.port,
                &printer_name,
                &printer_token.token,
            )
        };

        let mut cp = ClientPrinter {
            id,
            local_name: local_name.clone(),
            remote_name: printer_name.clone(),
            display_name: display.clone(),
            device_id: device_id.clone(),
            device_name: device.name.clone(),
            token: printer_token.token.clone(),
            token_exp: printer_token.expires_at,
            supports_scan,
            online: true,
            installed: false,
            preferences: prefs.clone(),
            capabilities: capabilities.clone(),
            last_error: String::new(),
        };
        // Register the printer before installing the queue: the IPP endpoint
        // must answer capability queries while CUPS or Windows creates it.
        self.config.edit(|cfg| cfg.client_printers.push(cp.clone()));

        #[cfg(windows)]
        {
            let app = self.clone();
            let forwarder = match Forwarder::start(app, cp.clone(), tcp_port).await {
                Ok(forwarder) => forwarder,
                Err(err) => {
                    self.config
                        .edit(|cfg| cfg.client_printers.retain(|printer| printer.id != id));
                    return Err(err);
                }
            };
            self.forwarders.lock().await.insert(id, forwarder);
        }

        if let Err(err) = platform::install_remote_printer(
            &local_name,
            &display,
            &queue_driver,
            id,
            tcp_port,
            &uri,
            &prefs,
        ) {
            #[cfg(windows)]
            {
                if let Some(handle) = self.forwarders.lock().await.remove(&id) {
                    handle.stop();
                }
            }
            self.config
                .edit(|cfg| cfg.client_printers.retain(|printer| printer.id != id));
            return Err(err);
        }

        cp.installed = true;
        self.config.edit(|cfg| {
            if let Some(printer) = cfg.client_printers.iter_mut().find(|p| p.id == id) {
                printer.installed = true;
            }
        });
        self.emit(AppEvent::Refresh);
        Ok(cp)
    }

    /// Remove every virtual printer that belongs to a host. The host itself is
    /// kept in the device list so it can be expanded again later.
    pub async fn disconnect_all_printers(self: &Arc<Self>, device_id: &str) -> Result<()> {
        let locals: Vec<String> = self
            .config
            .read()
            .client_printers
            .iter()
            .filter(|p| p.device_id == device_id)
            .map(|p| p.local_name.clone())
            .collect();
        for local in locals {
            if let Err(err) = self.remove_printer(local).await {
                warn!("could not disconnect printer: {err}");
            }
        }
        Ok(())
    }

    pub async fn remove_printer(self: &Arc<Self>, local_name: String) -> Result<()> {
        let cp = self
            .config
            .read()
            .client_printers
            .iter()
            .find(|p| p.local_name == local_name)
            .cloned()
            .ok_or_else(|| AppError::Network("printer not found".into()))?;
        #[cfg(windows)]
        if let Some(handle) = self.forwarders.lock().await.remove(&cp.id) {
            handle.stop();
        }
        if let Err(err) = platform::remove_remote_printer(&cp.local_name) {
            warn!("could not remove local queue {}: {err}", cp.local_name);
        }
        self.config.edit(|cfg| {
            cfg.client_printers
                .retain(|p| p.local_name != cp.local_name);
        });
        self.emit(AppEvent::Refresh);
        Ok(())
    }

    /// Current job settings for a client printer, used by the spool forwarder
    /// so changes in the UI apply to the next job without restarting it.
    pub fn client_preferences(&self, local_name: &str) -> Option<PrinterPreferences> {
        self.config
            .read()
            .client_printers
            .iter()
            .find(|p| p.local_name == local_name)
            .map(|p| p.preferences.clone())
    }

    pub fn set_client_preferences(
        &self,
        local_name: &str,
        prefs: PrinterPreferences,
    ) -> Result<()> {
        let mut found = false;
        self.config.edit(|cfg| {
            if let Some(cp) = cfg
                .client_printers
                .iter_mut()
                .find(|p| p.local_name == local_name)
            {
                cp.preferences = prefs.clone();
                found = true;
            }
        });
        if !found {
            return Err(AppError::Network("printer not found".into()));
        }
        if let Err(err) = platform::apply_printer_preferences(local_name, &prefs) {
            warn!("could not update queue defaults for {local_name}: {err}");
        }
        self.emit(AppEvent::Refresh);
        Ok(())
    }

    pub async fn test_print(self: &Arc<Self>, local_name: String) -> Result<()> {
        let cp = self
            .config
            .read()
            .client_printers
            .iter()
            .find(|p| p.local_name == local_name)
            .cloned()
            .ok_or_else(|| AppError::Network("printer not found".into()))?;
        let device = self
            .config
            .read()
            .remote_devices
            .iter()
            .find(|d| d.id == cp.device_id)
            .cloned()
            .ok_or_else(|| AppError::Network("device not found".into()))?;
        let link = device_link(&device);
        let data = platform::test_page(&device.name);
        let mut prefs = cp.preferences.clone();
        // The colour test page is a single sheet: always print it one-sided,
        // without booklet imposition, and only once.
        prefs.duplex = DuplexMode::Simplex;
        prefs.booklet = false;
        prefs.copies = 1;
        link.print(
            &self.http,
            &cp.token,
            &cp.remote_name,
            data,
            "application/pdf",
            "lanprint-test.pdf",
            &prefs,
        )
        .await?;
        self.emit(AppEvent::Toast("test_print_sent".into()));
        Ok(())
    }

    pub async fn scan(
        self: &Arc<Self>,
        local_name: String,
        request: ScanRequest,
    ) -> Result<std::path::PathBuf> {
        let cp = self
            .config
            .read()
            .client_printers
            .iter()
            .find(|p| p.local_name == local_name)
            .cloned()
            .ok_or_else(|| AppError::Network("printer not found".into()))?;
        let device = self
            .config
            .read()
            .remote_devices
            .iter()
            .find(|d| d.id == cp.device_id)
            .cloned()
            .ok_or_else(|| AppError::Network("device not found".into()))?;
        let link = device_link(&device);
        let data = link
            .scan(&self.http, &cp.token, &cp.remote_name, &request)
            .await?;
        let dir = lanprint_core::config::default_download_dir().join("lanPrint-scans");
        std::fs::create_dir_all(&dir)?;
        let stamp = chrono::Local::now().format("%Y%m%d-%H%M%S");
        let ext = if request.format == "pdf" {
            "pdf"
        } else if request.format == "jpeg" {
            "jpg"
        } else {
            "png"
        };
        let path = dir.join(format!("scan-{stamp}.{ext}"));
        std::fs::write(&path, data)?;
        self.emit(AppEvent::Toast(format!("scan_saved:{}", path.display())));
        self.emit(AppEvent::Refresh);
        Ok(path)
    }

    pub async fn send_print(
        self: &Arc<Self>,
        device_id: &str,
        printer: &str,
        token: &str,
        data: Vec<u8>,
        content_type: &str,
        filename: &str,
        prefs: &lanprint_core::PrinterPreferences,
    ) -> Result<()> {
        let device = self
            .config
            .read()
            .remote_devices
            .iter()
            .find(|d| d.id == device_id)
            .cloned()
            .ok_or_else(|| AppError::Network("device not found".into()))?;
        let link = device_link(&device);
        link.print(
            &self.http,
            token,
            printer,
            data,
            content_type,
            filename,
            prefs,
        )
        .await?;
        Ok(())
    }

    pub async fn restore_forwarders(self: &Arc<Self>) {
        #[cfg(windows)]
        {
            let printers = self.config.read().client_printers.clone();
            let mut guard = self.forwarders.lock().await;
            for cp in printers {
                let port = 19_100u16.saturating_add(cp.id as u16 % 800);
                match Forwarder::start(self.clone(), cp.clone(), port).await {
                    Ok(forwarder) => {
                        guard.insert(cp.id, forwarder);
                    }
                    Err(err) => warn!("could not restore forwarder for {}: {err}", cp.local_name),
                }
            }
        }
    }

    pub fn record_job(&self, job: JobRecord) {
        let mut state = self.state.write().unwrap();
        state.jobs.push_front(job);
        state.jobs.truncate(100);
    }

    /// Verify a host without saving it. Used by the "test connection" button.
    pub async fn test_connection(
        &self,
        address: &str,
        password: &str,
        cloud: Option<(String, String)>,
    ) -> Result<(InfoResponse, lanprint_core::AuthResponse)> {
        let link = if let Some((server, code)) = cloud {
            HostLink::cloud(&server, &code)
        } else {
            let (host, port) =
                lanprint_core::parse_address(address, lanprint_core::DEFAULT_API_PORT)
                    .ok_or_else(|| AppError::Network("invalid address".into()))?;
            HostLink::lan(&host, port, false)
        };
        let info = link.info(&self.http).await?;
        let (client_id, client_name) = self.identity();
        let auth = link
            .auth(&self.http, password, &client_id, &client_name)
            .await?;
        Ok((info, auth))
    }

    pub async fn set_cloud_config(
        self: &Arc<Self>,
        enabled: bool,
        server_url: String,
        server_code: String,
    ) -> Result<()> {
        let (changed_server, changed_code) = {
            let cfg = self.config.read();
            (
                cfg.cloud.server_url.trim_end_matches('/')
                    != server_url.trim().trim_end_matches('/'),
                cfg.cloud.server_code != server_code,
            )
        };
        self.config.edit(|cfg| {
            cfg.cloud.enabled = enabled;
            cfg.cloud.server_url = server_url.trim().trim_end_matches('/').to_string();
            cfg.cloud.server_code = server_code;
            if changed_server || changed_code {
                cfg.cloud.host_token.clear();
                cfg.cloud.host_token_exp = 0;
                cfg.cloud.share_code.clear();
            }
            if !enabled {
                cfg.cloud.connected = false;
                cfg.cloud.share_code.clear();
                cfg.cloud.host_token.clear();
            }
        });
        self.start_cloud_host().await;
        self.emit(AppEvent::Refresh);
        Ok(())
    }

    async fn start_cloud_host(self: &Arc<Self>) {
        let cfg = self.config.read().cloud.clone();
        let mut slot = self.cloud_task.lock().await;
        if let Some(task) = slot.take() {
            task.abort();
        }
        if !cfg.enabled || cfg.server_url.is_empty() {
            self.state.write().unwrap().cloud_status = String::new();
            return;
        }
        let app = self.clone();
        *slot = Some(tokio::spawn(async move {
            crate::cloud::run_cloud_host(app).await;
        }));
    }

    pub fn set_cloud_status(&self, status: impl Into<String>) {
        self.state.write().unwrap().cloud_status = status.into();
        self.emit(AppEvent::Refresh);
    }

    pub async fn set_relay_sender(
        &self,
        tx: Option<tokio::sync::mpsc::UnboundedSender<lanprint_core::RelayFrame>>,
    ) {
        *self.relay_tx.lock().await = tx;
    }

    pub async fn relay_sender(
        &self,
    ) -> Option<tokio::sync::mpsc::UnboundedSender<lanprint_core::RelayFrame>> {
        self.relay_tx.lock().await.clone()
    }

    pub fn open_logs(&self) {
        let dir = platform::log_dir();
        let _ = std::fs::create_dir_all(&dir);
        let _ = platform::open_path(&dir);
    }

    pub async fn shutdown(self: &Arc<Self>) {
        if let Some(daemon) = self.mdns.lock().await.take() {
            let name = self.config.read().device_name.clone();
            let _ = tokio::task::spawn_blocking(move || discovery::unregister(daemon, &name)).await;
        }
        let mut guard = self.forwarders.lock().await;
        for (_, forwarder) in guard.drain() {
            forwarder.stop();
        }
    }
}

pub struct RuntimeSnapshot {
    pub local_printers: Vec<LocalPrinter>,
    pub remote_printers: HashMap<String, Vec<SharedPrinter>>,
    pub jobs: Vec<JobRecord>,
    pub cloud_status: String,
    pub busy: bool,
}

pub fn device_link(device: &RemoteDevice) -> HostLink {
    if device.cloud && !device.share_code.is_empty() {
        HostLink::cloud(&device.address, &device.share_code)
    } else {
        HostLink::lan(&device.address, device.port, false)
    }
}

fn version_parts(version: &str) -> Vec<u64> {
    version
        .trim_start_matches('v')
        .split('.')
        .map(|part| {
            part.chars()
                .take_while(|ch| ch.is_ascii_digit())
                .collect::<String>()
                .parse()
                .unwrap_or(0)
        })
        .collect()
}

fn version_is_newer(latest: &str, current: &str) -> bool {
    let latest = version_parts(latest);
    let current = version_parts(current);
    for index in 0..latest.len().max(current.len()) {
        let left = latest.get(index).copied().unwrap_or(0);
        let right = current.get(index).copied().unwrap_or(0);
        if left != right {
            return left > right;
        }
    }
    false
}

fn update_asset(version: &str, assets: &[(String, String)]) -> Option<(String, String)> {
    #[cfg(windows)]
    let mut fallback: Option<(String, String)> = None;
    for (name, url) in assets {
        #[cfg(windows)]
        {
            if name.ends_with("-setup.exe") {
                if name == &format!("lanPrint-{version}-setup.exe") {
                    return Some((name.clone(), url.clone()));
                }
                fallback.get_or_insert_with(|| (name.clone(), url.clone()));
            }
        }
        #[cfg(target_os = "macos")]
        {
            let arch = if cfg!(target_arch = "aarch64") {
                "Mac_aarch64"
            } else {
                "Mac_x86_64"
            };
            if name.ends_with(".tar.gz") && name.contains(arch) {
                return Some((name.clone(), url.clone()));
            }
        }
        #[cfg(all(unix, not(target_os = "macos")))]
        {
            let arch = if cfg!(target_arch = "aarch64") {
                "Linux_aarch64"
            } else {
                "Linux_x86_64"
            };
            if name.ends_with(".tar.gz") && name.contains(arch) {
                return Some((name.clone(), url.clone()));
            }
        }
    }
    #[cfg(windows)]
    {
        fallback
    }
    #[cfg(not(windows))]
    {
        None
    }
}

fn upgraded_queues() -> &'static std::sync::Mutex<std::collections::HashSet<u32>> {
    static DONE: std::sync::OnceLock<std::sync::Mutex<std::collections::HashSet<u32>>> =
        std::sync::OnceLock::new();
    DONE.get_or_init(|| std::sync::Mutex::new(std::collections::HashSet::new()))
}

fn unique_local_name(cfg: &AppConfig, device: &str, display: &str, id: u32) -> String {
    let clean_part = |value: &str| -> String {
        let clean: String = value
            .chars()
            .map(|c| {
                if c.is_alphanumeric() || c == '-' || c == '_' {
                    c
                } else {
                    '-'
                }
            })
            .collect();
        clean.trim_matches('-').to_string()
    };
    let device = clean_part(device);
    let display = clean_part(display);
    let base = match (device.is_empty(), display.is_empty()) {
        (false, false) => format!("{device}-{display}"),
        (false, true) => device,
        (true, false) => display,
        (true, true) => format!("lanPrint-{id}"),
    };
    let mut name = base.clone();
    let mut n = 1;
    while cfg.client_printers.iter().any(|p| p.local_name == name) {
        n += 1;
        name = format!("{base}-{n}");
    }
    if name.len() > 60 {
        name = format!("lanPrint-{id}");
    }
    name
}

#[cfg(test)]
mod naming_tests {
    use super::{unique_local_name, version_is_newer};
    use lanprint_core::{AppConfig, ClientPrinter};

    #[test]
    fn queues_are_named_after_device_and_printer() {
        let mut cfg = AppConfig::default();
        let name = unique_local_name(&cfg, "Design-PC", "HP LaserJet Pro", 1);
        assert_eq!(name, "Design-PC-HP-LaserJet-Pro");

        let printer = ClientPrinter {
            local_name: name.clone(),
            ..Default::default()
        };
        cfg.client_printers.push(printer);
        assert_eq!(
            unique_local_name(&cfg, "Design-PC", "HP LaserJet Pro", 2),
            "Design-PC-HP-LaserJet-Pro-2"
        );
    }

    #[test]
    fn falls_back_when_names_are_empty() {
        let cfg = AppConfig::default();
        assert_eq!(unique_local_name(&cfg, "", "", 7), "lanPrint-7");
        assert_eq!(unique_local_name(&cfg, "Design-PC", "", 7), "Design-PC");
    }

    #[test]
    fn compares_versions() {
        assert!(version_is_newer("2.0.2", "2.0.1"));
        assert!(version_is_newer("v2.1.0", "2.0.9"));
        assert!(version_is_newer("2.0.10", "2.0.9"));
        assert!(!version_is_newer("2.0.1", "2.0.1"));
        assert!(!version_is_newer("1.9.9", "2.0.0"));
    }
}
