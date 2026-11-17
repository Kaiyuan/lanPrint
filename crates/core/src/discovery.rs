//! mDNS advertisement and browsing. The implementation is runtime agnostic;
//! callers run it on a blocking thread or inside `spawn_blocking`.

use std::collections::HashMap;
use std::time::{Duration, Instant};

use mdns_sd::{ServiceDaemon, ServiceEvent, ServiceInfo};
use serde::{Deserialize, Serialize};
use tracing::{debug, warn};

use crate::model::DiscoveredDevice;
use crate::VERSION;

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct TxtRecords {
    pub id: String,
    pub name: String,
    pub os: String,
    pub version: String,
    pub port: u16,
    pub cloud: bool,
    pub requires_password: bool,
    pub shared_count: u32,
}

impl TxtRecords {
    pub fn to_properties(&self) -> HashMap<String, String> {
        HashMap::from([
            ("id".to_string(), self.id.clone()),
            ("name".to_string(), self.name.clone()),
            ("os".to_string(), self.os.clone()),
            ("version".to_string(), self.version.clone()),
            ("port".to_string(), self.port.to_string()),
            ("cloud".to_string(), self.cloud.to_string()),
            ("pw".to_string(), self.requires_password.to_string()),
            ("printers".to_string(), self.shared_count.to_string()),
        ])
    }

    pub fn from_properties(props: &HashMap<String, String>) -> Self {
        Self {
            id: props.get("id").cloned().unwrap_or_default(),
            name: props.get("name").cloned().unwrap_or_default(),
            os: props.get("os").cloned().unwrap_or_default(),
            version: props.get("version").cloned().unwrap_or_default(),
            port: props
                .get("port")
                .and_then(|p| p.parse().ok())
                .unwrap_or(crate::DEFAULT_API_PORT),
            cloud: props.get("cloud").map(|v| v == "true").unwrap_or(false),
            requires_password: props.get("pw").map(|v| v == "true").unwrap_or(true),
            shared_count: props
                .get("printers")
                .and_then(|v| v.parse().ok())
                .unwrap_or(0),
        }
    }
}

/// Advertise this host. Keep the returned daemon alive for as long as the
/// service should be visible.
pub fn advertise(
    device_id: &str,
    device_name: &str,
    port: u16,
    cloud: bool,
    requires_password: bool,
    shared_count: u32,
) -> Result<ServiceDaemon, Box<dyn std::error::Error + Send + Sync>> {
    let mdns = ServiceDaemon::new()?;
    let txt = TxtRecords {
        id: device_id.to_string(),
        name: device_name.to_string(),
        os: crate::protocol::os_name().to_string(),
        version: VERSION.to_string(),
        port,
        cloud,
        requires_password,
        shared_count,
    };
    let instance = sanitize_instance(device_name);
    let hostname = format!("{}.local.", sanitize_host(device_name));
    let service = ServiceInfo::new(
        crate::MDNS_SERVICE,
        &instance,
        &hostname,
        (),
        port,
        txt.to_properties(),
    )?
    .enable_addr_auto();
    mdns.register(service)?;
    Ok(mdns)
}

pub fn unregister(mdns: ServiceDaemon, device_name: &str) {
    let instance = sanitize_instance(device_name);
    let fullname = format!("{}.{}", instance, crate::MDNS_SERVICE);
    let _ = mdns.unregister(&fullname);
    let _ = mdns.shutdown();
}

fn sanitize_instance(name: &str) -> String {
    let cleaned: String = name
        .chars()
        .map(|c| {
            if c.is_alphanumeric() || c == '-' || c == '_' {
                c
            } else {
                '-'
            }
        })
        .collect();
    if cleaned.is_empty() {
        "lanPrint".into()
    } else {
        cleaned
    }
}

fn sanitize_host(name: &str) -> String {
    sanitize_instance(name).to_ascii_lowercase()
}

/// Browse the LAN for `_lanprint._tcp.local.` hosts. Stops after `timeout`.
pub fn browse(timeout: Duration) -> Vec<DiscoveredDevice> {
    let mut found = HashMap::<String, DiscoveredDevice>::new();
    let mdns = match ServiceDaemon::new() {
        Ok(m) => m,
        Err(err) => {
            warn!("mDNS unavailable: {err}");
            return Vec::new();
        }
    };
    let receiver = match mdns.browse(crate::MDNS_SERVICE) {
        Ok(r) => r,
        Err(err) => {
            warn!("mDNS browse failed: {err}");
            return Vec::new();
        }
    };
    let deadline = Instant::now() + timeout;
    while Instant::now() < deadline {
        let remaining = deadline.saturating_duration_since(Instant::now());
        match receiver.recv_timeout(remaining.min(Duration::from_millis(250))) {
            Ok(ServiceEvent::ServiceResolved(info)) => {
                let props = info
                    .get_properties()
                    .iter()
                    .map(|p| (p.key().to_string(), p.val_str().to_string()))
                    .collect::<HashMap<_, _>>();
                let txt = TxtRecords::from_properties(&props);
                let address = info
                    .get_addresses()
                    .iter()
                    .find(|ip| ip.is_ipv4())
                    .or_else(|| info.get_addresses().iter().next())
                    .map(|ip| ip.to_string())
                    .unwrap_or_default();
                if address.is_empty() {
                    continue;
                }
                let key = if txt.id.is_empty() {
                    format!("{address}:{}", info.get_port())
                } else {
                    txt.id.clone()
                };
                let name = if txt.name.is_empty() {
                    info.get_fullname()
                        .split('.')
                        .next()
                        .unwrap_or("lanPrint")
                        .to_string()
                } else {
                    txt.name.clone()
                };
                found.insert(
                    key,
                    DiscoveredDevice {
                        id: txt.id,
                        name,
                        address,
                        port: info.get_port(),
                        os: txt.os,
                        version: txt.version,
                        cloud: txt.cloud,
                        requires_password: txt.requires_password,
                        shared_count: txt.shared_count,
                    },
                );
            }
            Ok(_) => {}
            Err(mdns_sd::RecvTimeoutError::Timeout) => {}
            Err(mdns_sd::RecvTimeoutError::Disconnected) => break,
        }
    }
    let _ = mdns.stop_browse(crate::MDNS_SERVICE);
    let _ = mdns.shutdown();
    let mut list: Vec<_> = found.into_values().collect();
    list.sort_by(|a, b| a.name.to_lowercase().cmp(&b.name.to_lowercase()));
    debug!("mDNS browse found {} devices", list.len());
    list
}

/// Resolve a single host name to an IPv4 address using the OS resolver.
pub fn resolve_host(name: &str) -> Option<String> {
    use std::net::ToSocketAddrs;
    let addrs = (name, 0u16).to_socket_addrs().ok()?;
    for addr in addrs {
        if addr.is_ipv4() {
            return Some(addr.ip().to_string());
        }
    }
    None
}
