//! CUPS backend entry point.
//!
//! Installed as `/usr/lib/cups/backend/lanprint`, this mode is invoked by CUPS
//! with the device URI in `DEVICE_URI`. The job data is forwarded to the host
//! using the normal authenticated API.

use std::io::Read;

use lanprint_core::{DuplexMode, PrinterPreferences};
use tracing::{error, info};

use crate::client::{self, HostLink};

/// Translate the CUPS job options into the preferences the host understands.
fn preferences_from_options(options: &str) -> PrinterPreferences {
    let mut prefs = PrinterPreferences {
        // Empty paper means "use the host default".
        paper: String::new(),
        ..Default::default()
    };
    for token in options.split_whitespace() {
        let (key, value) = token.split_once('=').unwrap_or((token, ""));
        match key {
            "sides" => {
                prefs.duplex = match value {
                    "two-sided-long-edge" => DuplexMode::LongEdge,
                    "two-sided-short-edge" => DuplexMode::ShortEdge,
                    _ => DuplexMode::Simplex,
                };
            }
            "media" => prefs.paper = value.to_ascii_uppercase(),
            "ColorModel" => prefs.color = !value.to_ascii_lowercase().contains("gray"),
            "Resolution" | "printer-resolution" => {
                let dpi: String = value.chars().take_while(|c| c.is_ascii_digit()).collect();
                if let Ok(dpi) = dpi.parse() {
                    prefs.resolution = dpi;
                }
            }
            "InputSlot" | "media-source" => prefs.media_source = value.to_string(),
            "booklet" | "Booklet" => prefs.booklet = value.eq_ignore_ascii_case("true"),
            "copies" => prefs.copies = value.parse().unwrap_or(1),
            _ => {}
        }
    }
    prefs
}

pub fn run() -> ! {
    let args: Vec<String> = std::env::args().collect();
    let uri = std::env::var("DEVICE_URI").unwrap_or_default();
    let (link, printer, token) =
        if let Some((server, code, printer, token)) = client::parse_cloud_device_uri(&uri) {
            (HostLink::cloud(&server, &code), printer, token)
        } else if let Some((host, port, printer, token)) = client::parse_device_uri(&uri) {
            (HostLink::lan(&host, port, false), printer, token)
        } else {
            eprintln!("ERROR: invalid lanPrint device URI: {uri}");
            std::process::exit(1);
        };
    let title = args
        .get(3)
        .cloned()
        .unwrap_or_else(|| "lanPrint job".into());
    let path = args.get(6).cloned().unwrap_or_default();
    let mut data = Vec::new();
    let read_result = if !path.is_empty() && path != "-" {
        std::fs::read(&path).map(|bytes| data = bytes)
    } else {
        std::io::stdin().read_to_end(&mut data).map(|_| ())
    };
    if let Err(err) = read_result {
        eprintln!("ERROR: cannot read spool file: {err}");
        std::process::exit(1);
    }
    if data.is_empty() {
        eprintln!("ERROR: empty print job");
        std::process::exit(1);
    }
    let content_type = if data.starts_with(b"%PDF") {
        "application/pdf"
    } else if data.starts_with(b"%!PS") {
        "application/postscript"
    } else {
        "application/octet-stream"
    };
    let runtime = match tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
    {
        Ok(runtime) => runtime,
        Err(err) => {
            eprintln!("ERROR: cannot start runtime: {err}");
            std::process::exit(1);
        }
    };
    let printer_inner = printer.clone();
    let prefs = preferences_from_options(args.get(5).map(String::as_str).unwrap_or(""));
    let result = runtime.block_on(async move {
        let http = reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(600))
            .build()?;
        link.print(
            &http,
            &token,
            &printer_inner,
            data,
            content_type,
            &title,
            &prefs,
        )
        .await
    });
    match result {
        Ok(_) => {
            info!("CUPS job forwarded for {printer}");
            std::process::exit(0);
        }
        Err(err) => {
            error!("CUPS job failed: {err}");
            eprintln!("ERROR: {err}");
            std::process::exit(1);
        }
    }
}
