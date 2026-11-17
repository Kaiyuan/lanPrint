//! lanPrint core: shared data model, configuration store, authentication and
//! the wire protocol used between lanPrint peers and the cloud bridge.
//!
//! Everything in this crate is platform independent so that the desktop app,
//! the cloud bridge and tests all agree on the same definitions.

pub mod auth;
pub mod config;
pub mod discovery;
pub mod i18n;
pub mod model;
pub mod protocol;

pub use auth::{issue_token, verify_token, PasswordHash, TokenClaims};
pub use config::{AppConfig, ConfigStore};
pub use model::*;
pub use protocol::*;

/// Version reported by the API and mDNS TXT records.
pub const VERSION: &str = env!("CARGO_PKG_VERSION");

/// Default local API port used by every lanPrint desktop instance.
pub const DEFAULT_API_PORT: u16 = 52333;

/// Port used to detect an already running desktop instance on this machine.
pub const INSTANCE_PORT: u16 = 52335;

/// mDNS service type advertised by lanPrint hosts.
pub const MDNS_SERVICE: &str = "_lanprint._tcp.local.";

/// Maximum accepted print job size (128 MiB).
pub const MAX_JOB_BYTES: usize = 128 * 1024 * 1024;
