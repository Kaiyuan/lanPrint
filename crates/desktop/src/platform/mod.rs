//! Platform specific printer, scanner and virtual queue integration.

use std::path::Path;

use lanprint_core::{LocalPrinter, PrinterPreferences, ScanRequest};

use crate::error::Result;

#[cfg(windows)]
mod windows;
#[cfg(windows)]
use windows as imp;

#[cfg(unix)]
mod unix;
#[cfg(unix)]
use unix as imp;

#[derive(Debug, Clone)]
pub struct ScannerInfo {
    pub id: String,
    pub name: String,
    #[allow(dead_code)]
    pub adf: bool,
}

pub fn list_printers() -> Result<Vec<LocalPrinter>> {
    imp::list_printers()
}

#[allow(dead_code)]
pub fn detect_scanners() -> Result<Vec<ScannerInfo>> {
    imp::detect_scanners()
}

pub fn scan(request: &ScanRequest, scanner_id: &str) -> Result<Vec<u8>> {
    imp::scan(request, scanner_id)
}

pub fn print_document(
    printer: &str,
    data: &[u8],
    content_type: &str,
    filename: &str,
    prefs: &PrinterPreferences,
) -> Result<()> {
    imp::print_document(printer, data, content_type, filename, prefs)
}

/// Install a client-side virtual queue. On Windows `uri` and `prefs` are
/// ignored because the queue connects to a local TCP listener instead. On
/// Unix the CUPS backend consumes the `lanprint://` URI directly.
pub fn install_remote_printer(
    local_name: &str,
    display_name: &str,
    driver: &str,
    tcp_port: u16,
    uri: &str,
    prefs: &PrinterPreferences,
) -> Result<()> {
    #[cfg(windows)]
    {
        let _ = (uri, prefs);
        imp::install_remote_printer(local_name, display_name, driver, tcp_port)
    }
    #[cfg(unix)]
    {
        imp::install_remote_printer(local_name, display_name, driver, tcp_port, uri, prefs)
    }
}

pub fn remove_remote_printer(local_name: &str) -> Result<()> {
    imp::remove_remote_printer(local_name)
}

pub fn open_path(path: &Path) -> Result<()> {
    imp::open_path(path)
}

pub fn log_dir() -> std::path::PathBuf {
    imp::log_dir()
}

/// Open the operating system's printer preference dialog for a local queue.
pub fn open_printer_preferences(name: &str) -> Result<()> {
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        let status = std::process::Command::new("rundll32")
            .args(["printui.dll,PrintUIEntry", "/e", "/n", name])
            .creation_flags(0x0800_0000)
            .status()
            .map_err(|e| crate::error::AppError::Platform(e.to_string()))?;
        if !status.success() {
            return Err(crate::error::AppError::Platform(
                "could not open printer preferences".into(),
            ));
        }
        Ok(())
    }
    #[cfg(unix)]
    {
        // CUPS does not define a portable GUI. Prefer the desktop printer
        // configuration tool when present and fall back to the CUPS web UI.
        let candidates = ["system-config-printer", "system-config-printer.py"];
        for candidate in candidates {
            if std::process::Command::new(candidate)
                .arg("--show-printer-properties")
                .arg(name)
                .spawn()
                .is_ok()
            {
                return Ok(());
            }
        }
        let url = format!(
            "http://localhost:631/printers/{}",
            crate::client::url_encode(name)
        );
        open::that(url).map_err(|e| crate::error::AppError::Platform(e.to_string()))
    }
}

/// Copy text to the system clipboard using the platform helper utilities.
pub fn copy_text(text: &str) -> Result<()> {
    use std::io::Write;
    use std::process::{Command, Stdio};
    #[cfg(windows)]
    let mut cmd = {
        use std::os::windows::process::CommandExt;
        let mut cmd = Command::new("cmd");
        cmd.args(["/c", "clip"]).creation_flags(0x0800_0000);
        cmd
    };
    #[cfg(target_os = "macos")]
    let mut cmd = Command::new("pbcopy");
    #[cfg(all(unix, not(target_os = "macos")))]
    let mut cmd = {
        let mut cmd = Command::new("sh");
        cmd.args([
            "-c",
            "command -v wl-copy >/dev/null 2>&1 && wl-copy || xclip -selection clipboard",
        ]);
        cmd
    };
    let mut child = cmd
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|e| crate::error::AppError::Platform(format!("clipboard: {e}")))?;
    if let Some(mut stdin) = child.stdin.take() {
        stdin
            .write_all(text.as_bytes())
            .map_err(|e| crate::error::AppError::Platform(format!("clipboard: {e}")))?;
    }
    let status = child
        .wait()
        .map_err(|e| crate::error::AppError::Platform(format!("clipboard: {e}")))?;
    if !status.success() {
        return Err(crate::error::AppError::Platform(
            "no clipboard utility available (install xclip or wl-clipboard)".into(),
        ));
    }
    Ok(())
}

/// A helper used by both platforms to build a small test page. It is plain
/// text so that even the "Generic / Text Only" fallback driver can print it.
pub fn test_page(device_name: &str) -> Vec<u8> {
    format!(
        "lanPrint test page\n\
         ------------------------------\n\
         Host: {device_name}\n\
         If you can read this page, the shared printer is working.\n\
         \n\
         lanPrint - cross platform printer sharing\n\
         https://github.com/Kaiyuan/lanPrint\n"
    )
    .into_bytes()
}
