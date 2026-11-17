use std::collections::HashMap;
use std::sync::RwLock;
use std::time::Instant;

use lanprint_core::RelayFrame;
use tokio::sync::mpsc::UnboundedSender;

pub struct HostConn {
    pub tx: UnboundedSender<RelayFrame>,
    #[allow(dead_code)]
    pub device_id: String,
    #[allow(dead_code)]
    pub name: String,
    #[allow(dead_code)]
    pub api_port: u16,
}

pub struct Session {
    pub host_id: String,
    pub exp: i64,
}

pub struct Pending {
    pub client_id: String,
    pub tx: UnboundedSender<RelayFrame>,
    pub created: Instant,
}

#[derive(Default)]
pub struct Hub {
    pub hosts: RwLock<HashMap<String, HostConn>>,
    pub sessions: RwLock<HashMap<String, Session>>,
    pub pending: RwLock<HashMap<u64, Pending>>,
}

impl Hub {
    pub fn register_host(
        &self,
        device_id: &str,
        tx: UnboundedSender<RelayFrame>,
        name: String,
        api_port: u16,
    ) {
        self.hosts.write().unwrap().insert(
            device_id.to_string(),
            HostConn {
                tx,
                device_id: device_id.to_string(),
                name,
                api_port,
            },
        );
    }

    pub fn unregister_host(&self, device_id: &str) {
        self.hosts.write().unwrap().remove(device_id);
    }

    pub fn host_sender(&self, device_id: &str) -> Option<UnboundedSender<RelayFrame>> {
        self.hosts
            .read()
            .unwrap()
            .get(device_id)
            .map(|h| h.tx.clone())
    }

    pub fn host_online(&self, device_id: &str) -> bool {
        self.hosts.read().unwrap().contains_key(device_id)
    }

    pub fn add_session(&self, token: String, host_id: String, exp: i64) {
        self.sessions
            .write()
            .unwrap()
            .insert(token, Session { host_id, exp });
    }

    pub fn session_host(&self, token: &str) -> Option<String> {
        let mut sessions = self.sessions.write().unwrap();
        let now = chrono::Utc::now().timestamp();
        sessions.retain(|_, s| s.exp > now);
        sessions.get(token).map(|s| s.host_id.clone())
    }

    pub fn add_pending(&self, id: u64, client_id: &str, tx: UnboundedSender<RelayFrame>) {
        self.pending.write().unwrap().insert(
            id,
            Pending {
                client_id: client_id.to_string(),
                tx,
                created: Instant::now(),
            },
        );
    }

    pub fn finish_pending(&self, id: u64) {
        self.pending.write().unwrap().remove(&id);
    }

    pub fn drop_client(&self, client_id: &str) {
        self.pending
            .write()
            .unwrap()
            .retain(|_, p| p.client_id != client_id);
    }

    pub fn cleanup(&self) {
        let cutoff = Instant::now() - std::time::Duration::from_secs(180);
        self.pending
            .write()
            .unwrap()
            .retain(|_, p| p.created > cutoff);
        let now = chrono::Utc::now().timestamp();
        self.sessions.write().unwrap().retain(|_, s| s.exp > now);
    }
}
