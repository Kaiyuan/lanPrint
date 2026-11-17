use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::RwLock;

use lanprint_core::CloudRegisterRequest;
use serde::{Deserialize, Serialize};
use tracing::warn;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HostRecord {
    pub device_id: String,
    pub device_name: String,
    pub version: String,
    pub share_code: String,
    pub host_token: String,
    pub api_port: u16,
    pub created_at: i64,
    pub last_seen: i64,
    pub public_ip: String,
}

#[derive(Debug, Default, Serialize, Deserialize)]
pub struct Persisted {
    #[serde(default)]
    pub auth_fingerprint: String,
    pub hosts: HashMap<String, HostRecord>,
}

pub struct Store {
    path: PathBuf,
    data: RwLock<Persisted>,
}

impl Store {
    pub fn load(path: PathBuf) -> std::io::Result<Self> {
        let data = match std::fs::read_to_string(&path) {
            Ok(text) => serde_json::from_str(&text).unwrap_or_else(|err| {
                warn!("bridge data file is invalid: {err}");
                Persisted::default()
            }),
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => Persisted::default(),
            Err(err) => return Err(err),
        };
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        Ok(Self {
            path,
            data: RwLock::new(data),
        })
    }

    pub fn upsert(&self, record: HostRecord) {
        {
            let mut data = self.data.write().unwrap();
            data.hosts.insert(record.device_id.clone(), record);
        }
        self.save();
    }

    pub fn by_share_code(&self, code: &str) -> Option<HostRecord> {
        self.data
            .read()
            .unwrap()
            .hosts
            .values()
            .find(|h| h.share_code.eq_ignore_ascii_case(code))
            .cloned()
    }

    pub fn by_token(&self, token: &str) -> Option<HostRecord> {
        self.data
            .read()
            .unwrap()
            .hosts
            .values()
            .find(|h| h.host_token == token)
            .cloned()
    }

    pub fn by_device(&self, device_id: &str) -> Option<HostRecord> {
        self.data.read().unwrap().hosts.get(device_id).cloned()
    }

    pub fn touch(&self, device_id: &str, ip: &str) {
        {
            let mut data = self.data.write().unwrap();
            if let Some(host) = data.hosts.get_mut(device_id) {
                host.last_seen = chrono::Utc::now().timestamp();
                if !ip.is_empty() {
                    host.public_ip = ip.to_string();
                }
            }
        }
        self.save();
    }

    pub fn stats(&self) -> (usize, usize) {
        let data = self.data.read().unwrap();
        (
            data.hosts.len(),
            data.hosts
                .values()
                .filter(|h| h.last_seen > chrono::Utc::now().timestamp() - 120)
                .count(),
        )
    }

    /// Rotate host tokens if the server access code changed since the last run.
    pub fn sync_auth_code(&self, code: &str) {
        use sha2::{Digest, Sha256};
        let hash = Sha256::digest(code.as_bytes());
        let fingerprint = format!("{hash:x}");
        let changed = {
            let data = self.data.read().unwrap();
            data.auth_fingerprint != fingerprint
        };
        if !changed {
            return;
        }
        {
            let mut data = self.data.write().unwrap();
            data.auth_fingerprint = fingerprint;
            for host in data.hosts.values_mut() {
                host.host_token.clear();
            }
        }
        warn!("server access code changed; all host tokens were invalidated");
        self.save();
    }

    pub fn ensure_code(&self, req: &CloudRegisterRequest) -> String {
        if let Some(existing) = self.by_device(&req.device_id) {
            if !existing.share_code.is_empty() {
                return existing.share_code;
            }
        }
        if !req.old_share_code.is_empty() && self.by_share_code(&req.old_share_code).is_none() {
            return req.old_share_code.clone();
        }
        loop {
            let code = generate_share_code();
            if self.by_share_code(&code).is_none() {
                return code;
            }
        }
    }

    fn save(&self) {
        let text = {
            let data = self.data.read().unwrap();
            serde_json::to_string_pretty(&*data).unwrap_or_default()
        };
        let tmp = self.path.with_extension("json.tmp");
        if let Err(err) = std::fs::write(&tmp, text) {
            warn!("cannot write bridge store: {err}");
            return;
        }
        if let Err(err) = std::fs::rename(&tmp, &self.path) {
            warn!("cannot replace bridge store: {err}");
        }
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let _ = std::fs::set_permissions(&self.path, std::fs::Permissions::from_mode(0o600));
        }
    }
}

const CODE_ALPHABET: &[u8] = b"23456789ABCDEFGHJKLMNPQRSTUVWXYZ";

pub fn generate_share_code() -> String {
    use rand::Rng;
    let mut rng = rand::thread_rng();
    (0..8)
        .map(|_| CODE_ALPHABET[rng.gen_range(0..CODE_ALPHABET.len())] as char)
        .collect()
}

pub fn random_token() -> String {
    use base64::Engine;
    let mut bytes = [0u8; 32];
    rand::Rng::fill(&mut rand::thread_rng(), &mut bytes);
    base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(bytes)
}
