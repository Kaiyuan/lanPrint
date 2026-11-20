//! Platform specific printer, scanner and virtual queue integration.

use std::path::Path;

use lanprint_core::{LocalPrinter, PrinterPreferences, ScanRequest};

use crate::error::Result;

#[cfg(windows)]
mod windows;
#[cfg(windows)]
mod windows_pdf;
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
    id: u32,
    tcp_port: u16,
    uri: &str,
    prefs: &PrinterPreferences,
) -> Result<()> {
    #[cfg(windows)]
    {
        let _ = uri;
        imp::install_remote_printer(local_name, display_name, driver, id, tcp_port, prefs)
    }
    #[cfg(unix)]
    {
        imp::install_remote_printer(local_name, display_name, driver, id, tcp_port, uri, prefs)
    }
}

pub fn remove_remote_printer(local_name: &str) -> Result<()> {
    imp::remove_remote_printer(local_name)
}

/// Start the downloaded platform update package.
pub fn launch_update(path: &Path) -> Result<()> {
    imp::launch_update(path)
}

/// Move existing client queues to the local IPP endpoint. Windows batches them
/// into one elevated helper run so the user is asked for administrator rights
/// at most once.
pub fn upgrade_ipp_queues(printers: &[(String, u32, PrinterPreferences)]) -> Result<()> {
    #[cfg(unix)]
    {
        for (local_name, id, _) in printers {
            imp::upgrade_ipp_queue(local_name, *id)?;
        }
        Ok(())
    }
    #[cfg(windows)]
    {
        imp::upgrade_ipp_queues(printers)
    }
}

/// Whether an existing queue is still on the pre-IPP code path.
pub fn queue_needs_upgrade(local_name: &str, id: u32) -> bool {
    #[cfg(unix)]
    {
        imp::queue_needs_upgrade(local_name, id)
    }
    #[cfg(windows)]
    {
        let _ = id;
        imp::queue_needs_upgrade(local_name)
    }
}

/// Run the elevated printer helper instance (Windows only).
#[cfg(windows)]
pub fn run_printer_job(job: Option<&String>, out: Option<&String>) -> i32 {
    imp::run_printer_job(job, out)
}

/// Update the local queue defaults so the system print dialog starts from the
/// same settings as the lanPrint dialog.
pub fn apply_printer_preferences(local_name: &str, prefs: &PrinterPreferences) -> Result<()> {
    imp::apply_printer_preferences(local_name, prefs)
}

pub fn open_path(path: &Path) -> Result<()> {
    imp::open_path(path)
}

pub fn log_dir() -> std::path::PathBuf {
    imp::log_dir()
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

/// Build the colourful A4 test page used by the "Test print" action. It is a
/// PDF so the normal print pipeline (host-side rasterising, colour, paper and
/// duplex handling) is exercised end to end.
pub fn test_page(device_name: &str) -> Vec<u8> {
    crate::testpage::pdf(device_name)
}
