//! Access to the application's Tokio runtime from UI callbacks.
//!
//! Slint callbacks run on the UI thread, outside any Tokio runtime context, so
//! `tokio::spawn` cannot be used directly there. The runtime handle is stored
//! once at startup and shared by the UI helper functions.

use std::future::Future;
use std::sync::OnceLock;

use tokio::runtime::Handle;
use tracing::error;

static HANDLE: OnceLock<Handle> = OnceLock::new();

pub fn install(handle: Handle) {
    let _ = HANDLE.set(handle);
}

pub fn spawn<F>(future: F)
where
    F: Future + Send + 'static,
    F::Output: Send + 'static,
{
    if let Some(handle) = HANDLE.get() {
        handle.spawn(future);
    } else {
        error!("async runtime is not installed; dropping background task");
    }
}
