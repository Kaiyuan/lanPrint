//! Host activity log: who connected and which jobs were printed.
//!
//! The log is written to the configuration directory so the host can review
//! activity after a restart. Entries are capped and stored newest first.

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use serde::{Deserialize, Serialize};
use tracing::warn;

const MAX_ENTRIES: usize = 500;
/// Repeated connection reports from the same device within this window (for
/// example "test connection" followed by "save", or routine device refreshes)
/// are collapsed into one entry.
const CONNECT_DEDUPE_SECS: i64 = 30 * 60;

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ActivityKind {
    Connect,
    Print,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ActivityEntry {
    /// Unix seconds.
    pub time: i64,
    pub kind: ActivityKind,
    pub device: String,
    pub device_id: String,
    /// `lan` or `cloud`.
    pub channel: String,
    /// Client IP for LAN connections; empty for cloud relays.
    pub address: String,
    pub printer: String,
    pub filename: String,
    pub bytes: u64,
    /// `sent` or `failed`.
    pub status: String,
    pub message: String,
}

pub struct ActivityLog {
    path: PathBuf,
    entries: Mutex<Vec<ActivityEntry>>,
}

impl ActivityLog {
    pub fn load(dir: &Path) -> Self {
        let path = dir.join("activity.json");
        let mut entries: Vec<ActivityEntry> = fs::read(&path)
            .ok()
            .and_then(|data| serde_json::from_slice(&data).ok())
            .unwrap_or_default();
        entries.truncate(MAX_ENTRIES);
        Self {
            path,
            entries: Mutex::new(entries),
        }
    }

    pub fn entries(&self) -> Vec<ActivityEntry> {
        self.entries
            .lock()
            .map(|entries| entries.clone())
            .unwrap_or_default()
    }

    pub fn record(&self, entry: ActivityEntry) {
        let mut entries = match self.entries.lock() {
            Ok(entries) => entries,
            Err(err) => {
                warn!("activity log is poisoned: {err}");
                return;
            }
        };
        if entry.kind == ActivityKind::Connect {
            let duplicate = entries.iter().any(|last| {
                last.kind == ActivityKind::Connect
                    && last.channel == entry.channel
                    && !entry.device_id.is_empty()
                    && last.device_id == entry.device_id
                    && (entry.time - last.time).abs() < CONNECT_DEDUPE_SECS
            });
            if duplicate {
                return;
            }
        }
        entries.insert(0, entry);
        entries.truncate(MAX_ENTRIES);
        self.persist(&entries);
    }

    fn persist(&self, entries: &[ActivityEntry]) {
        let Ok(text) = serde_json::to_vec_pretty(entries) else {
            return;
        };
        let tmp = self.path.with_extension("json.tmp");
        if let Err(err) = fs::write(&tmp, text).and_then(|_| fs::rename(&tmp, &self.path)) {
            warn!("could not save activity log: {err}");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(kind: ActivityKind, device: &str, time: i64) -> ActivityEntry {
        ActivityEntry {
            time,
            kind,
            device: device.into(),
            device_id: device.into(),
            channel: "lan".into(),
            address: "192.168.1.10".into(),
            printer: "Printer".into(),
            filename: "doc.pdf".into(),
            bytes: 1024,
            status: "sent".into(),
            message: String::new(),
        }
    }

    #[test]
    fn keeps_newest_first_and_dedupes_connections() {
        let dir = std::env::temp_dir().join(format!("lanprint-activity-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        let log = ActivityLog::load(&dir);
        log.record(entry(ActivityKind::Connect, "PC-A", 100));
        log.record(entry(ActivityKind::Connect, "PC-A", 120));
        log.record(entry(ActivityKind::Print, "PC-A", 130));
        let entries = log.entries();
        assert_eq!(entries.len(), 2);
        assert_eq!(entries[0].kind, ActivityKind::Print);
        assert_eq!(entries[1].kind, ActivityKind::Connect);
        let _ = fs::remove_dir_all(&dir);
    }
}
