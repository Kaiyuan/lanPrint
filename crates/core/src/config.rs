use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::{Arc, RwLock};

use rand::RngCore;
use serde::{Deserialize, Serialize};
use tracing::{info, warn};

use crate::auth::PasswordHash;
use crate::model::{
    ClientPrinter, CloudConfig, Language, PrinterPreferences, RemoteDevice, SharedPrinterConfig,
};

/// Persistent application configuration. This file is written atomically and
/// contains no plaintext access codes (only Argon2 hashes and signed tokens).
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct AppConfig {
    pub schema: u32,
    pub device_id: String,
    pub device_name: String,
    pub language: String,
    /// `system`, `light` or `dark`.
    pub appearance: String,
    /// Accent theme: `blue`, `purple`, `pink`, `red`, `orange`, `green` or `graphite`.
    pub accent: String,
    pub autostart: bool,
    pub api_port: u16,
    /// Argon2 hash of the host (connection) access code.
    pub host_password_hash: String,
    /// Base64 secret used to sign short lived session tokens.
    pub host_secret: String,
    /// Rotated whenever the host access code changes so old tokens die.
    pub token_epoch: u64,
    pub shared_printers: Vec<SharedPrinterConfig>,
    pub remote_devices: Vec<RemoteDevice>,
    pub client_printers: Vec<ClientPrinter>,
    pub cloud: CloudConfig,
    pub default_preferences: PrinterPreferences,
    pub log_level: String,
    pub first_run: bool,
}

impl Default for AppConfig {
    fn default() -> Self {
        let mut secret = [0u8; 32];
        rand::thread_rng().fill_bytes(&mut secret);
        let secret =
            base64::Engine::encode(&base64::engine::general_purpose::STANDARD_NO_PAD, secret);
        Self {
            schema: 2,
            device_id: uuid::Uuid::new_v4().to_string(),
            device_name: default_device_name(),
            language: sys_language(),
            appearance: "system".into(),
            accent: "blue".into(),
            autostart: true,
            api_port: crate::DEFAULT_API_PORT,
            host_password_hash: String::new(),
            host_secret: secret,
            token_epoch: 1,
            shared_printers: Vec::new(),
            remote_devices: Vec::new(),
            client_printers: Vec::new(),
            cloud: CloudConfig::default(),
            default_preferences: PrinterPreferences::default(),
            log_level: "info".into(),
            first_run: true,
        }
    }
}

impl AppConfig {
    pub fn language(&self) -> Language {
        Language::from_code(&self.language).unwrap_or(Language::ZhCn)
    }

    pub fn has_host_password(&self) -> bool {
        !self.host_password_hash.is_empty()
    }

    pub fn set_host_password(&mut self, password: &str) -> Result<(), crate::auth::AuthError> {
        if password.trim().len() < 4 {
            return Err(crate::auth::AuthError::PasswordRequired);
        }
        let hash = PasswordHash::create(password)?;
        self.host_password_hash = hash.into_string();
        // Invalidate every previously issued token.
        self.token_epoch = self.token_epoch.wrapping_add(1);
        let mut secret = [0u8; 32];
        rand::thread_rng().fill_bytes(&mut secret);
        self.host_secret =
            base64::Engine::encode(&base64::engine::general_purpose::STANDARD_NO_PAD, secret);
        self.first_run = false;
        Ok(())
    }

    pub fn verify_host_password(&self, password: &str) -> bool {
        if self.host_password_hash.is_empty() {
            return false;
        }
        PasswordHash::from_string(&self.host_password_hash)
            .map(|hash| hash.verify(password))
            .unwrap_or(false)
    }

    pub fn shared_printer(&self, name: &str) -> Option<&SharedPrinterConfig> {
        self.shared_printers.iter().find(|p| p.name == name)
    }

    pub fn shared_printer_mut(&mut self, name: &str) -> Option<&mut SharedPrinterConfig> {
        self.shared_printers.iter_mut().find(|p| p.name == name)
    }

    pub fn upsert_shared_printer(
        &mut self,
        name: &str,
        enabled: bool,
        password: Option<&str>,
        preferences: Option<PrinterPreferences>,
    ) -> Result<(), crate::auth::AuthError> {
        let password_hash = match password {
            Some(pw) if !pw.is_empty() => Some(PasswordHash::create(pw)?.into_string()),
            _ => None,
        };
        if let Some(existing) = self.shared_printer_mut(name) {
            existing.enabled = enabled;
            if let Some(hash) = password_hash {
                existing.password_hash = hash;
            }
            if password.map(|p| p.is_empty()).unwrap_or(false) {
                existing.password_hash.clear();
            }
            if let Some(prefs) = preferences {
                existing.preferences = prefs;
            }
            return Ok(());
        }
        let mut cfg = SharedPrinterConfig {
            name: name.to_string(),
            enabled,
            shared_at: chrono::Utc::now().timestamp(),
            ..Default::default()
        };
        if let Some(hash) = password_hash {
            cfg.password_hash = hash;
        }
        if let Some(prefs) = preferences {
            cfg.preferences = prefs;
        }
        self.shared_printers.push(cfg);
        Ok(())
    }

    pub fn set_printer_password(
        &mut self,
        name: &str,
        password: &str,
    ) -> Result<(), crate::auth::AuthError> {
        let hash = if password.is_empty() {
            String::new()
        } else {
            PasswordHash::create(password)?.into_string()
        };
        if let Some(p) = self.shared_printer_mut(name) {
            p.password_hash = hash;
            Ok(())
        } else {
            Err(crate::auth::AuthError::Internal(
                "printer not shared".into(),
            ))
        }
    }

    pub fn prune_tokens(&mut self) {
        let now = chrono::Utc::now().timestamp();
        for device in &mut self.remote_devices {
            if device.host_token_exp < now {
                device.host_token.clear();
            }
        }
        for printer in &mut self.client_printers {
            if printer.token_exp < now {
                printer.token.clear();
            }
        }
    }
}

fn default_device_name() -> String {
    hostname().unwrap_or_else(|| "lanPrint".to_string())
}

fn hostname() -> Option<String> {
    #[cfg(windows)]
    {
        std::env::var("COMPUTERNAME").ok()
    }
    #[cfg(not(windows))]
    {
        std::fs::read_to_string("/etc/hostname")
            .ok()
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
            .or_else(|| std::env::var("HOSTNAME").ok())
    }
}

fn sys_language() -> String {
    let code = std::env::var("LC_ALL")
        .or_else(|_| std::env::var("LC_MESSAGES"))
        .or_else(|_| std::env::var("LANG"))
        .unwrap_or_default();
    if !code.is_empty() {
        if let Some(lang) = Language::from_code(code.split('.').next().unwrap_or("")) {
            return lang.code().to_string();
        }
    }
    #[cfg(windows)]
    {
        let name = unsafe { windows_locale_name() };
        if let Some(lang) = Language::from_code(&name) {
            return lang.code().to_string();
        }
    }
    Language::ZhCn.code().to_string()
}

#[cfg(windows)]
unsafe fn windows_locale_name() -> String {
    #[link(name = "kernel32")]
    extern "system" {
        fn GetUserDefaultLocaleName(lp_locale_name: *mut u16, cch_locale_name: i32) -> i32;
    }
    let mut buf = [0u16; 85];
    let n = GetUserDefaultLocaleName(buf.as_mut_ptr(), buf.len() as i32);
    if n > 0 {
        String::from_utf16_lossy(&buf[..(n as usize - 1)])
    } else {
        String::new()
    }
}

/// Thread-safe wrapper around the configuration file.
pub struct ConfigStore {
    path: PathBuf,
    data: RwLock<AppConfig>,
}

impl ConfigStore {
    pub fn load_or_create(dir: Option<PathBuf>) -> std::io::Result<Arc<Self>> {
        let custom_dir = dir.is_some();
        let dir = dir.unwrap_or_else(default_config_dir);
        if !custom_dir {
            migrate_legacy_config(&dir);
        }
        fs::create_dir_all(&dir)?;
        let path = dir.join("config.toml");
        let data = if path.exists() {
            match fs::read_to_string(&path) {
                Ok(text) => match toml::from_str::<AppConfig>(&text) {
                    Ok(mut cfg) => {
                        cfg.normalize();
                        cfg
                    }
                    Err(err) => {
                        warn!("config parse failed, starting with defaults: {err}");
                        let backup = dir.join(format!(
                            "config.broken.{}.toml",
                            chrono::Utc::now().timestamp()
                        ));
                        let _ = fs::copy(&path, backup);
                        AppConfig::default()
                    }
                },
                Err(err) => {
                    warn!("config read failed: {err}");
                    AppConfig::default()
                }
            }
        } else {
            AppConfig::default()
        };
        let store = Self {
            path,
            data: RwLock::new(data),
        };
        store.save()?;
        info!("configuration loaded from {}", store.path.display());
        Ok(Arc::new(store))
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    pub fn config_dir(&self) -> PathBuf {
        self.path.parent().unwrap_or(Path::new(".")).to_path_buf()
    }

    pub fn read(&self) -> AppConfig {
        self.data.read().unwrap().clone()
    }

    pub fn edit<R>(&self, f: impl FnOnce(&mut AppConfig) -> R) -> R {
        let mut guard = self.data.write().unwrap();
        let result = f(&mut guard);
        if let Err(err) = self.write_locked(&guard) {
            warn!("failed to save config: {err}");
        }
        result
    }

    pub fn save(&self) -> std::io::Result<()> {
        let guard = self.data.read().unwrap();
        self.write_locked(&guard)
    }

    fn write_locked(&self, cfg: &AppConfig) -> std::io::Result<()> {
        let text = toml::to_string_pretty(cfg)
            .map_err(|e| std::io::Error::new(std::io::ErrorKind::Other, e))?;
        let tmp = self
            .path
            .with_extension(format!("toml.{}.tmp", std::process::id()));
        {
            let mut file = fs::File::create(&tmp)?;
            file.write_all(text.as_bytes())?;
            file.sync_all()?;
        }
        fs::rename(&tmp, &self.path)?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let _ = fs::set_permissions(&self.path, fs::Permissions::from_mode(0o600));
        }
        Ok(())
    }
}

impl AppConfig {
    pub fn normalize(&mut self) {
        if self.device_id.is_empty() {
            self.device_id = uuid::Uuid::new_v4().to_string();
        }
        if self.device_name.is_empty() {
            self.device_name = default_device_name();
        }
        if self.api_port == 0 {
            self.api_port = crate::DEFAULT_API_PORT;
        }
        if !matches!(self.appearance.as_str(), "system" | "light" | "dark") {
            self.appearance = "system".into();
        }
        if !matches!(
            self.accent.as_str(),
            "blue" | "purple" | "pink" | "red" | "orange" | "green" | "graphite"
        ) {
            self.accent = "blue".into();
        }
        if self.host_secret.is_empty() {
            let mut secret = [0u8; 32];
            rand::thread_rng().fill_bytes(&mut secret);
            self.host_secret =
                base64::Engine::encode(&base64::engine::general_purpose::STANDARD_NO_PAD, secret);
        }
        if self.schema < 2 {
            self.schema = 2;
        }
        self.prune_tokens();
    }
}

pub fn default_config_dir() -> PathBuf {
    dirs::document_dir()
        .or_else(dirs::config_dir)
        .unwrap_or_else(|| PathBuf::from("."))
        .join("lanPrint")
}

/// Older builds stored the configuration under the platform config directory
/// (`%APPDATA%\lanPrint` on Windows). Copy it to the new Documents location the
/// first time the new path is used, so upgrades keep their settings.
fn migrate_legacy_config(new_dir: &Path) {
    let new_path = new_dir.join("config.toml");
    if new_path.exists() {
        return;
    }
    let Some(legacy_dir) = dirs::config_dir().map(|dir| dir.join("lanPrint")) else {
        return;
    };
    if legacy_dir == new_dir {
        return;
    }
    let legacy_path = legacy_dir.join("config.toml");
    if !legacy_path.exists() {
        return;
    }
    if fs::create_dir_all(new_dir).is_ok() && fs::copy(&legacy_path, &new_path).is_ok() {
        info!(
            "migrated configuration from {} to {}",
            legacy_path.display(),
            new_path.display()
        );
    }
}

pub fn default_data_dir() -> PathBuf {
    dirs::data_local_dir()
        .unwrap_or_else(default_config_dir)
        .join("lanPrint")
}

pub fn default_download_dir() -> PathBuf {
    dirs::download_dir()
        .or_else(dirs::picture_dir)
        .unwrap_or_else(std::env::temp_dir)
}

/// Base64 helpers used by config and protocol code.
pub mod b64 {
    use base64::Engine;

    pub const ENGINE: base64::engine::GeneralPurpose =
        base64::engine::general_purpose::STANDARD_NO_PAD;

    pub fn encode(data: impl AsRef<[u8]>) -> String {
        ENGINE.encode(data)
    }

    pub fn decode(data: &str) -> Result<Vec<u8>, base64::DecodeError> {
        ENGINE.decode(data)
    }
}
