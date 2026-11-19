use std::collections::{HashMap, VecDeque};
use std::sync::mpsc::Sender;
use std::sync::{Arc, RwLock};

use lanprint_core::auth::{
    authorize_host, authorize_printer, decode_secret, issue_token, PasswordHash, TokenClaims,
    HOST_TOKEN_TTL, PRINTER_TOKEN_TTL,
};
use lanprint_core::{
    discovery, AppConfig, ClientPrinter, ConfigStore, DiscoveredDevice, InfoResponse, JobRecord,
    LocalPrinter, PrinterPreferences, RemoteDevice, ScanRequest, SharedPrinter,
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
}

impl App {
    pub fn new(config: Arc<ConfigStore>, events: Sender<AppEvent>) -> Result<Arc<Self>> {
        let http = reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(20))
            .connect_timeout(std::time::Duration::from_secs(5))
            .user_agent(format!("lanPrint/{}", lanprint_core::VERSION))
            .build()
            .map_err(AppError::from)?;
        Ok(Arc::new(Self {
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

    pub fn issue_printer_token(&self, printer: &str) -> Result<(String, i64)> {
        let cfg = self.config.read();
        let secret = decode_secret(&cfg.host_secret);
        let claims = TokenClaims::printer(&cfg.device_id, printer, PRINTER_TOKEN_TTL);
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
        let auth = link.auth(&self.http, &password).await.map_err(|err| {
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
                if let Ok(auth) = link.auth(&self.http, &device.password).await {
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
        let printer_token = link
            .printer_token(&self.http, &host_token, &printer_name, &password)
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
        let local_name = unique_local_name(&cfg, &shared.display_name, id);
        let display = shared.display_name.clone();
        let prefs = shared.preferences.clone();
        let capabilities = shared.capabilities.clone();
        let supports_scan = shared.supports_scan;
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

        #[cfg(windows)]
        {
            let app = self.clone();
            let client_printer = ClientPrinter {
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
            let forwarder = Forwarder::start(app, client_printer, tcp_port).await?;
            self.forwarders.lock().await.insert(id, forwarder);
        }

        if let Err(err) = platform::install_remote_printer(
            &local_name,
            &display,
            &shared.driver,
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
            return Err(err);
        }

        let cp = ClientPrinter {
            id,
            local_name: local_name.clone(),
            remote_name: printer_name,
            display_name: display,
            device_id: device_id.clone(),
            device_name: device.name,
            token: printer_token.token,
            token_exp: printer_token.expires_at,
            supports_scan,
            online: true,
            installed: true,
            preferences: prefs,
            capabilities,
            last_error: String::new(),
        };
        self.config.edit(|cfg| cfg.client_printers.push(cp.clone()));
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
        link.print(
            &self.http,
            &cp.token,
            &cp.remote_name,
            data,
            "text/plain",
            "lanprint-test.txt",
            &cp.preferences,
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
        let auth = link.auth(&self.http, password).await?;
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

fn unique_local_name(cfg: &AppConfig, display: &str, id: u32) -> String {
    let clean: String = display
        .chars()
        .map(|c| {
            if c.is_alphanumeric() || c == '-' || c == '_' {
                c
            } else {
                '-'
            }
        })
        .collect();
    let base = format!("lanPrint-{}", clean.trim_matches('-'));
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
