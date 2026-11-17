//! Virtual printer forwarding.
//!
//! On Windows a client queue is backed by a standard TCP/IP printer port. The
//! spooler connects to `127.0.0.1:<port>` and writes the rendered job; we read
//! it and forward it to the host over the normal lanPrint API.

use std::sync::Arc;

use lanprint_core::ClientPrinter;
use tracing::{info, warn};

use crate::app::App;
use crate::error::Result;

#[cfg(windows)]
pub struct Forwarder {
    handle: tokio::task::JoinHandle<()>,
}

#[cfg(windows)]
impl Forwarder {
    pub async fn start(app: Arc<App>, client: ClientPrinter, port: u16) -> Result<Self> {
        let listener = tokio::net::TcpListener::bind(("127.0.0.1", port))
            .await
            .map_err(|e| {
                crate::error::AppError::Network(format!("cannot listen on 127.0.0.1:{port}: {e}"))
            })?;
        info!(
            "virtual printer {} listening on 127.0.0.1:{port}",
            client.local_name
        );
        let handle = tokio::spawn(async move {
            loop {
                let Ok((mut socket, _)) = listener.accept().await else {
                    break;
                };
                let app = app.clone();
                let client = client.clone();
                tokio::spawn(async move {
                    use tokio::io::AsyncReadExt;
                    let mut data = Vec::new();
                    let read = tokio::time::timeout(
                        std::time::Duration::from_secs(120),
                        socket.read_to_end(&mut data),
                    )
                    .await;
                    match read {
                        Ok(Ok(_)) if !data.is_empty() => {
                            let (content_type, filename) = sniff(&data);
                            if let Err(err) = app
                                .send_print(
                                    &client.device_id,
                                    &client.remote_name,
                                    &client.token,
                                    data,
                                    content_type,
                                    filename,
                                )
                                .await
                            {
                                warn!("forwarding job for {} failed: {err}", client.local_name);
                            }
                        }
                        Ok(Ok(_)) => {}
                        Ok(Err(err)) => warn!("spool read error: {err}"),
                        Err(_) => warn!("spool read timed out for {}", client.local_name),
                    }
                });
            }
        });
        Ok(Self { handle })
    }

    pub fn stop(self) {
        self.handle.abort();
    }
}

#[cfg(windows)]
fn sniff(data: &[u8]) -> (&'static str, &'static str) {
    if data.starts_with(b"%PDF") {
        ("application/pdf", "spool.pdf")
    } else if data.starts_with(b"%!PS") {
        ("application/postscript", "spool.ps")
    } else if data.starts_with(b"PK\x03\x04") {
        (
            "application/vnd.openxmlformats-officedocument.wordprocessingml.document",
            "spool.docx",
        )
    } else {
        ("application/octet-stream", "spool.raw")
    }
}

#[cfg(not(windows))]
pub struct Forwarder;

#[cfg(not(windows))]
impl Forwarder {
    pub async fn start(_app: Arc<App>, _client: ClientPrinter, _port: u16) -> Result<Self> {
        Ok(Self)
    }

    pub fn stop(self) {}
}
