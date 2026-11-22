//! Best-effort injection of host capabilities into a `Microsoft IPP Class
//! Driver` queue.
//!
//! Windows generates a GPD plus `ipp_print_capabilities.xml` for every IPP
//! queue. The class driver only puts the *default* media size, input bin,
//! duplex mode and resolution into those files, even though lanPrint's IPP
//! endpoint advertises the host printer's complete set. Classic applications
//! (Word included) read the compiled GPD through `DeviceCapabilities`, modern
//! ones read the XML print capabilities, so both are patched here.
//!
//! The spooler is restarted to let it rebuild its binary GPD cache (`.BUD`)
//! from the patched GPD; the cache file itself is removed when possible. The
//! patched GPD is verified through `DeviceCapabilities` and rolled back when it
//! cannot be compiled, so a failed patch never leaves the queue broken.
//!
//! Set `LANPRINT_CAPS_TRACE` to a file path to get diagnostics from the
//! elevated helper, which runs before normal logging is initialised.

use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::OnceLock;

#[cfg(windows)]
use std::os::windows::process::CommandExt;

use lanprint_core::PrinterCapabilities;
use serde::{Deserialize, Serialize};
use tracing::{debug, warn};

use crate::error::{AppError, Result};
use crate::media;

const CREATE_NO_WINDOW: u32 = 0x0800_0000;
/// Bumped whenever the injected layout changes so existing queues are patched
/// again (once) after an update.
const CAPS_VERSION: u32 = 1;
const MARKER: &str = "lanprint-caps.txt";
const BACKUP_SUFFIX: &str = ".lanprint-original";
const IPP_NAMESPACE: &str =
    "http://schemas.microsoft.com/windows/2018/04/printing/printschemakeywords/Ipp";

/// Host capabilities that are worth exposing through the Windows queue.
///
/// Resolution options are left to the class driver; it already offers the
/// queue's default resolution, which is the value the queue was created with.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct QueueCaps {
    pub papers: Vec<String>,
    pub sources: Vec<String>,
    pub duplex: bool,
    pub duplex_short_edge: bool,
}

impl QueueCaps {
    pub fn from_capabilities(caps: &PrinterCapabilities) -> Self {
        let sources = caps
            .sources
            .iter()
            .filter(|source| !is_auto_source(source))
            .cloned()
            .collect();
        QueueCaps {
            papers: caps.papers.clone(),
            sources,
            duplex: caps.duplex || !caps.duplex_modes.is_empty(),
            duplex_short_edge: caps
                .duplex_modes
                .iter()
                .any(|mode| mode.eq_ignore_ascii_case("short_edge")),
        }
    }

    /// Stable fingerprint of the capabilities that actually get injected.
    pub fn signature(&self) -> String {
        let mut papers: Vec<&str> = Vec::new();
        for paper in &self.papers {
            if let Some(size) = media::lookup(paper) {
                if !papers.contains(&size.keyword) {
                    papers.push(size.keyword);
                }
            }
        }
        format!(
            "v{CAPS_VERSION}|{}|{}|{}|{}",
            papers.join(","),
            self.sources.join(","),
            self.duplex,
            self.duplex_short_edge
        )
    }
}

/// Signature of the capabilities that were last injected into the queue, or
/// `None` when the queue has never been patched.
pub fn installed_signature(local_name: &str) -> Option<String> {
    let dir = v4_directory(local_name)?;
    let raw = std::fs::read_to_string(dir.join(MARKER)).ok()?;
    let value = raw.trim().to_string();
    (!value.is_empty()).then_some(value)
}

/// Whether the queue is backed by the class driver's per-queue files and can
/// therefore carry injected capabilities.
pub fn can_inject(local_name: &str) -> bool {
    v4_directory(local_name).is_some()
}

/// Patch the queue's generated files so the print dialog offers the host
/// printer's media sizes, trays and duplex modes.
pub fn apply(local_name: &str, caps: &QueueCaps, default_paper: &str) -> Result<()> {
    trace(&format!(
        "apply {local_name}: papers={:?} sources={:?} duplex={} short={}",
        caps.papers, caps.sources, caps.duplex, caps.duplex_short_edge
    ));
    let signature = caps.signature();
    let result = apply_inner(local_name, caps, default_paper);
    if let Some(dir) = v4_directory(local_name) {
        let _ = std::fs::write(dir.join(MARKER), &signature);
    }
    match &result {
        Ok(()) => trace("apply: ok"),
        Err(err) => trace(&format!("apply: {err}")),
    }
    result
}

/// Writes diagnostics to the file named by `LANPRINT_CAPS_TRACE`, if set. The
/// elevated helper runs before logging is initialised, so this is the only way
/// to see capability-injection problems on a user's machine.
fn trace(message: &str) {
    let Some(path) = std::env::var_os("LANPRINT_CAPS_TRACE") else {
        return;
    };
    use std::io::Write;
    if let Ok(mut file) = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
    {
        let _ = writeln!(file, "{message}");
    }
}

fn apply_inner(local_name: &str, caps: &QueueCaps, default_paper: &str) -> Result<()> {
    let Some(dir) = v4_directory(local_name) else {
        trace("no V4 driver directory");
        debug!("no V4 driver directory for {local_name}");
        return Ok(());
    };
    trace(&format!("v4 dir {}", dir.display()));
    let Some(gpd_path) = gpd_file(&dir) else {
        trace("no generated GPD");
        debug!("no generated GPD for {local_name}");
        return Ok(());
    };
    trace(&format!("gpd {}", gpd_path.display()));
    let caps_path = dir.join("ipp_print_capabilities.xml");
    let original_gpd = match std::fs::read_to_string(&gpd_path) {
        Ok(text) => text,
        Err(err) => {
            trace(&format!("read gpd failed: {err}"));
            debug!("cannot read {}: {err}", gpd_path.display());
            return Ok(());
        }
    };
    let Some(patched_gpd) = patch_gpd(&original_gpd, caps, default_paper) else {
        trace("patch_gpd returned none");
        debug!("GPD of {local_name} has no recognised paper feature");
        return Ok(());
    };
    trace("gpd patched");
    let original_caps = std::fs::read_to_string(&caps_path).ok();
    let fallback = original_default_paper(&original_gpd);
    let patched_caps = original_caps
        .as_deref()
        .and_then(|text| patch_caps(text, caps, fallback));

    if patched_gpd == original_gpd && patched_caps.as_deref() == original_caps.as_deref() {
        trace("already up to date");
        debug!("capabilities of {local_name} are already up to date");
        return Ok(());
    }

    backup(&gpd_path);
    if original_caps.is_some() {
        backup(&caps_path);
    }

    let expected = mapped_papers(caps).len();
    let restore = || {
        warn!("restoring the generated files of {local_name}");
        let _ = write_and_reload(
            &gpd_path,
            &original_gpd,
            &caps_path,
            original_caps.as_deref(),
        );
    };
    match write_and_reload(
        &gpd_path,
        &patched_gpd,
        &caps_path,
        patched_caps.as_deref(),
    ) {
        Ok(()) => match verify(local_name, expected) {
            Ok(()) => {
                trace("verify ok");
                debug!("injected {expected} media sizes into {local_name}");
                Ok(())
            }
            Err(err) => {
                trace(&format!("verify failed: {err}"));
                warn!("capability injection into {local_name} failed: {err}");
                restore();
                Err(err)
            }
        },
        Err(err) => {
            trace(&format!("write failed: {err}"));
            warn!("could not inject capabilities into {local_name}: {err}");
            restore();
            Err(err)
        }
    }
}

/// Replace the generated files with the spooler stopped. The stale binary GPD
/// cache is removed as well when it is not locked, so the driver rebuilds it
/// from the patched GPD on next use.
fn write_and_reload(
    gpd_path: &Path,
    gpd: &str,
    caps_path: &Path,
    caps: Option<&str>,
) -> Result<()> {
    trace("stopping spooler");
    stop_spooler()?;
    trace("spooler stopped");
    let written = (|| -> Result<()> {
        std::fs::write(gpd_path, gpd)
            .map_err(|err| AppError::Platform(format!("cannot patch {}: {err}", gpd_path.display())))?;
        if let Some(text) = caps {
            std::fs::write(caps_path, text).map_err(|err| {
                AppError::Platform(format!("cannot patch {}: {err}", caps_path.display()))
            })?;
        }
        if let Some(bud) = bud_file(gpd_path) {
            match std::fs::remove_file(&bud) {
                Ok(()) => debug!("removed stale {}", bud.display()),
                Err(err) if err.kind() == std::io::ErrorKind::NotFound => {}
                // The driver rebuilds its cache from the newer GPD when the
                // spooler restarts, so a locked cache file is not fatal.
                Err(err) => {
                    trace(&format!("could not remove {}: {err}", bud.display()));
                    debug!("could not remove {}: {err}", bud.display());
                }
            }
        }
        Ok(())
    })();
    let restarted = start_spooler();
    trace(&format!("written={written:?} restarted={restarted:?}"));
    written?;
    restarted
}

fn verify(local_name: &str, expected: usize) -> Result<()> {
    if expected == 0 {
        return Ok(());
    }
    let mut actual = 0;
    for _ in 0..8 {
        actual = paper_count(local_name);
        trace(&format!("paper_count={actual}"));
        if actual > 0 {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(400));
    }
    if actual == 0 {
        return Err(AppError::Platform(format!(
            "Windows could not rebuild the queue for {local_name}"
        )));
    }
    if (actual as usize) < expected {
        warn!("queue {local_name} exposes {actual} of {expected} media sizes");
    }
    Ok(())
}

fn backup(path: &Path) {
    let mut name = path.file_name().unwrap_or_default().to_os_string();
    name.push(BACKUP_SUFFIX);
    let target = path.with_file_name(name);
    if !target.exists() {
        let _ = std::fs::copy(path, &target);
    }
}

fn stop_spooler() -> Result<()> {
    run_net(&["stop", "spooler", "/y"], true)
}

fn start_spooler() -> Result<()> {
    run_net(&["start", "spooler"], true)
}

fn run_net(args: &[&str], tolerate_already: bool) -> Result<()> {
    let output = Command::new("net")
        .args(args)
        .creation_flags(CREATE_NO_WINDOW)
        .output()
        .map_err(|err| AppError::Platform(format!("cannot run net.exe: {err}")))?;
    if output.status.success() {
        return Ok(());
    }
    // `net` exits with 2 when a service is already stopped/started.
    if tolerate_already && output.status.code() == Some(2) {
        return Ok(());
    }
    let mut message = String::from_utf8_lossy(&output.stderr).trim().to_string();
    if message.is_empty() {
        message = String::from_utf8_lossy(&output.stdout).trim().to_string();
    }
    Err(AppError::Platform(format!("net {} failed: {message}", args.join(" "))))
}

/// Number of paper options as seen by classic applications.
fn paper_count(local_name: &str) -> i32 {
    use winapi::um::wingdi::DeviceCapabilitiesW;
    const DC_PAPERS: u16 = 2;
    let printer: Vec<u16> = local_name
        .encode_utf16()
        .chain(std::iter::once(0))
        .collect();
    unsafe {
        DeviceCapabilitiesW(
            printer.as_ptr(),
            std::ptr::null(),
            DC_PAPERS,
            std::ptr::null_mut(),
            std::ptr::null(),
        )
    }
}

fn v4_directory(local_name: &str) -> Option<PathBuf> {
    use winapi::um::winnt::KEY_READ;
    use winapi::um::winreg::{RegCloseKey, RegOpenKeyExW, RegQueryValueExW, HKEY_LOCAL_MACHINE};
    let subkey = format!("SYSTEM\\CurrentControlSet\\Control\\Print\\Printers\\{local_name}");
    let subkey: Vec<u16> = subkey.encode_utf16().chain(std::iter::once(0)).collect();
    let value_name: Vec<u16> = "PrintQueueV4DriverDirectory"
        .encode_utf16()
        .chain(std::iter::once(0))
        .collect();
    unsafe {
        let mut key = std::ptr::null_mut();
        if RegOpenKeyExW(HKEY_LOCAL_MACHINE, subkey.as_ptr(), 0, KEY_READ, &mut key) != 0 {
            return None;
        }
        let mut kind = 0u32;
        let mut size = 0u32;
        let mut status = RegQueryValueExW(
            key,
            value_name.as_ptr(),
            std::ptr::null_mut(),
            &mut kind,
            std::ptr::null_mut(),
            &mut size,
        );
        if status != 0 || size < 2 {
            RegCloseKey(key);
            return None;
        }
        let mut buffer = vec![0u8; size as usize + std::mem::size_of::<u16>()];
        status = RegQueryValueExW(
            key,
            value_name.as_ptr(),
            std::ptr::null_mut(),
            &mut kind,
            buffer.as_mut_ptr(),
            &mut size,
        );
        RegCloseKey(key);
        if status != 0 {
            return None;
        }
        let mut units = Vec::with_capacity(size as usize / 2);
        let mut index = 0;
        while index + 1 < buffer.len() {
            let unit = u16::from_le_bytes([buffer[index], buffer[index + 1]]);
            if unit == 0 {
                break;
            }
            units.push(unit);
            index += 2;
        }
        let name = String::from_utf16_lossy(&units);
        if name.trim().is_empty() {
            return None;
        }
        Some(spool_dir().join("V4Dirs").join(name.trim()))
    }
}

fn spool_dir() -> PathBuf {
    static DIR: OnceLock<PathBuf> = OnceLock::new();
    DIR.get_or_init(|| {
        std::env::var_os("WINDIR")
            .map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from("C:\\Windows"))
            .join("System32")
            .join("spool")
    })
    .clone()
}

fn gpd_file(dir: &Path) -> Option<PathBuf> {
    let mut candidates: Vec<PathBuf> = std::fs::read_dir(dir)
        .ok()?
        .filter_map(|entry| entry.ok().map(|entry| entry.path()))
        .filter(|path| {
            path.extension()
                .and_then(|ext| ext.to_str())
                .is_some_and(|ext| ext.eq_ignore_ascii_case("gpd"))
        })
        .collect();
    candidates.sort();
    candidates.into_iter().next()
}

fn bud_file(gpd: &Path) -> Option<PathBuf> {
    let mut path = gpd.to_path_buf();
    path.set_extension("BUD");
    path.exists().then_some(path)
}

fn is_auto_source(name: &str) -> bool {
    let value = name.trim().to_lowercase();
    value.starts_with("auto")
        || value.contains("自动")
        || value.contains("自動")
        || matches!(
            value.as_str(),
            "automatisch" | "automatique" | "automatico" | "automatycznie"
        )
}

fn mapped_papers(caps: &QueueCaps) -> Vec<&'static media::MediaSize> {
    let mut papers: Vec<&'static media::MediaSize> = Vec::new();
    for paper in &caps.papers {
        if let Some(size) = media::lookup(paper) {
            if !papers.iter().any(|known| known.keyword == size.keyword) {
                papers.push(size);
            }
        }
    }
    papers
}

/// Paper size the generated GPD currently defaults to, if it is a standard one.
fn original_default_paper(text: &str) -> Option<&'static media::MediaSize> {
    let start = text.find("*Feature: PaperSize")?;
    let block = &text[start..];
    let end = block.find("\n}")?;
    let value = block[..end]
        .lines()
        .find_map(|line| line.trim().strip_prefix("*DefaultOption:"))?
        .trim();
    media::lookup(value)
}

fn master_units(text: &str) -> f64 {
    const DEFAULT: f64 = 18000.0;
    let Some(index) = text.find("*MasterUnits:") else {
        return DEFAULT;
    };
    let rest = &text[index..];
    let Some(open) = rest.find("PAIR(") else {
        return DEFAULT;
    };
    let rest = &rest[open + 5..];
    let Some(comma) = rest.find(',') else {
        return DEFAULT;
    };
    rest[..comma]
        .trim()
        .parse::<f64>()
        .ok()
        .filter(|value| *value > 0.0)
        .map(|value| value / 25.4)
        .unwrap_or(DEFAULT)
}

fn multiline(parts: &[String], nl: &str) -> String {
    parts.join(nl)
}

fn patch_gpd(text: &str, caps: &QueueCaps, default_paper: &str) -> Option<String> {
    let mut papers = mapped_papers(caps);
    if papers.is_empty() {
        return None;
    }
    // The print ticket provider binds the queue's saved DEVMODE to the GPD.
    // Keep the paper the queue was originally created with, even if the host no
    // longer reports it, so a capability refresh cannot invalidate that
    // snapshot.
    if let Some(size) = original_default_paper(text) {
        if !papers.iter().any(|paper| paper.keyword == size.keyword) {
            papers.push(size);
        }
    }
    let nl = if text.contains("\r\n") { "\r\n" } else { "\n" };
    let units = master_units(text);
    let default = media::lookup(default_paper)
        .filter(|size| papers.iter().any(|paper| paper.keyword == size.keyword))
        .unwrap_or(papers[0])
        .gpd;

    let mut options: Vec<String> = Vec::new();
    for paper in &papers {
        options.push(
            [
                format!("*Option: {}", paper.gpd),
                "{".to_string(),
                "*rcNameID: =RCID_DMPAPER_SYSTEM_NAME".to_string(),
                format!("*PrintSchemaKeywordMap: \"{}\"", paper.keyword),
                "*PrintableOrigin: PAIR(0, 0)".to_string(),
                format!(
                    "*PrintableArea: PAIR({}, {})",
                    (paper.width_mm * units).round() as i64,
                    (paper.height_mm * units).round() as i64
                ),
                "}".to_string(),
            ]
            .join(nl),
        );
    }
    let paper_block = [
        "*Feature: PaperSize".to_string(),
        "{".to_string(),
        "*rcNameID: =PAPER_SIZE_DISPLAY".to_string(),
        "*PrintSchemaKeywordMap: \"PageMediaSize\"".to_string(),
        format!("*DefaultOption: {default}"),
        multiline(&options, nl),
        "}".to_string(),
    ]
    .join(nl);

    let mut out = replace_feature(text, "PaperSize", &paper_block)?;

    if caps.duplex {
        let mut duplex_options = vec![
            option_block(
                nl,
                "VERTICAL",
                "=FLIP_ON_LONG_EDGE_DISPLAY",
                "TwoSidedLongEdge",
            ),
            option_block(nl, "NONE", "=NONE_DISPLAY", "OneSided"),
        ];
        if caps.duplex_short_edge {
            duplex_options.insert(
                1,
                option_block(
                    nl,
                    "HORIZONTAL",
                    "=FLIP_ON_SHORT_EDGE_DISPLAY",
                    "TwoSidedShortEdge",
                ),
            );
        }
        let duplex_block = [
            "*Feature: Duplex".to_string(),
            "{".to_string(),
            "*rcNameID: =TWO_SIDED_PRINTING_DISPLAY".to_string(),
            "*PrintSchemaKeywordMap: \"JobDuplexAllDocumentsContiguously\"".to_string(),
            "*DefaultOption: NONE".to_string(),
            duplex_options.join(nl),
            "}".to_string(),
        ]
        .join(nl);
        out = insert_feature(&out, "Duplex", &duplex_block, nl);
    }

    if !caps.sources.is_empty() {
        let mut source_options: Vec<String> = vec![option_block(
            nl,
            "FORMSOURCE",
            "=AUTO_DISPLAY",
            "AutoSelect",
        )];
        for (index, label) in caps.sources.iter().enumerate() {
            source_options.push(
                [
                    format!("*Option: Tray{}", index + 1),
                    "{".to_string(),
                    format!("*Name: \"{}\"", label.replace('"', "'")),
                    format!("*PrintSchemaNamespace: \"{IPP_NAMESPACE}\""),
                    "}".to_string(),
                ]
                .join(nl),
            );
        }
        let bin_block = [
            "*Feature: InputBin".to_string(),
            "{".to_string(),
            "*rcNameID: =PAPER_SOURCE_DISPLAY".to_string(),
            "*PrintSchemaKeywordMap: \"JobInputBin\"".to_string(),
            "*DefaultOption: FORMSOURCE".to_string(),
            source_options.join(nl),
            "}".to_string(),
        ]
        .join(nl);
        out = insert_feature(&out, "InputBin", &bin_block, nl);
    }

    Some(out)
}

fn option_block(nl: &str, name: &str, rc: &str, keyword: &str) -> String {
    [
        format!("*Option: {name}"),
        "{".to_string(),
        format!("*rcNameID: {rc}"),
        format!("*PrintSchemaKeywordMap: \"{keyword}\""),
        "}".to_string(),
    ]
    .join(nl)
}

fn patch_caps(
    text: &str,
    caps: &QueueCaps,
    fallback: Option<&'static media::MediaSize>,
) -> Option<String> {
    let mut papers = mapped_papers(caps);
    if let Some(size) = fallback {
        if !papers.iter().any(|paper| paper.keyword == size.keyword) {
            papers.push(size);
        }
    }
    if papers.is_empty() {
        return None;
    }
    let media_options: String = papers
        .iter()
        .map(|paper| {
            format!(
                concat!(
                    "<psf:Option name=\"psk:{keyword}\" constrained=\"psk:None\">",
                    "<psf:ScoredProperty name=\"psk:MediaSizeWidth\">",
                    "<psf:Value xsi:type=\"xsd:integer\">{width}</psf:Value>",
                    "</psf:ScoredProperty>",
                    "<psf:ScoredProperty name=\"psk:MediaSizeHeight\">",
                    "<psf:Value xsi:type=\"xsd:integer\">{height}</psf:Value>",
                    "</psf:ScoredProperty>",
                    "<psf:Property name=\"psk:DisplayName\">",
                    "<psf:Value xsi:type=\"xsd:string\">{name}</psf:Value>",
                    "</psf:Property>",
                    "</psf:Option>"
                ),
                keyword = paper.keyword,
                width = (paper.width_mm * 1000.0).round() as i64,
                height = (paper.height_mm * 1000.0).round() as i64,
                name = escape_xml(paper.name),
            )
        })
        .collect();
    let media_block = format!(
        concat!(
            "<psf:Feature name=\"psk:PageMediaSize\">",
            "<psf:Property name=\"psf:SelectionType\">",
            "<psf:Value xsi:type=\"xsd:QName\">psk:PickOne</psf:Value>",
            "</psf:Property>",
            "<psf:Property name=\"psk:DisplayName\">",
            "<psf:Value xsi:type=\"xsd:string\">Paper Size</psf:Value>",
            "</psf:Property>",
            "{options}</psf:Feature>"
        ),
        options = media_options
    );
    let mut out = replace_xml_feature(text, "PageMediaSize", &media_block)?;

    if caps.duplex {
        let mut options = String::from(
            concat!(
                "<psf:Option name=\"psk:OneSided\" constrained=\"psk:None\">",
                "<psf:Property name=\"psk:DisplayName\">",
                "<psf:Value xsi:type=\"xsd:string\">One-Sided</psf:Value>",
                "</psf:Property></psf:Option>",
                "<psf:Option name=\"psk:TwoSidedLongEdge\" constrained=\"psk:None\">",
                "<psf:Property name=\"psk:DisplayName\">",
                "<psf:Value xsi:type=\"xsd:string\">Long Edge</psf:Value>",
                "</psf:Property></psf:Option>"
            ),
        );
        if caps.duplex_short_edge {
            options.push_str(concat!(
                "<psf:Option name=\"psk:TwoSidedShortEdge\" constrained=\"psk:None\">",
                "<psf:Property name=\"psk:DisplayName\">",
                "<psf:Value xsi:type=\"xsd:string\">Short Edge</psf:Value>",
                "</psf:Property></psf:Option>"
            ));
        }
        let block = format!(
            concat!(
                "<psf:Feature name=\"psk:JobDuplexAllDocumentsContiguously\">",
                "<psf:Property name=\"psf:SelectionType\">",
                "<psf:Value xsi:type=\"xsd:QName\">psk:PickOne</psf:Value>",
                "</psf:Property>",
                "<psf:Property name=\"psk:DisplayName\">",
                "<psf:Value xsi:type=\"xsd:string\">Duplex</psf:Value>",
                "</psf:Property>{options}</psf:Feature>"
            ),
            options = options
        );
        out = insert_xml_feature(&out, "JobDuplexAllDocumentsContiguously", &block);
    }

    if !caps.sources.is_empty() {
        let mut options = String::from(concat!(
            "<psf:Option name=\"psk:AutoSelect\" constrained=\"psk:None\">",
            "<psf:Property name=\"psk:DisplayName\">",
            "<psf:Value xsi:type=\"xsd:string\">Auto Select</psf:Value>",
            "</psf:Property></psf:Option>"
        ));
        for (index, label) in caps.sources.iter().enumerate() {
            options.push_str(&format!(
                concat!(
                    "<psf:Option name=\"ns0001:Tray{index}\" constrained=\"psk:None\">",
                    "<psf:Property name=\"psk:DisplayName\">",
                    "<psf:Value xsi:type=\"xsd:string\">{label}</psf:Value>",
                    "</psf:Property></psf:Option>"
                ),
                index = index + 1,
                label = escape_xml(label)
            ));
        }
        let block = format!(
            concat!(
                "<psf:Feature name=\"psk:JobInputBin\">",
                "<psf:Property name=\"psf:SelectionType\">",
                "<psf:Value xsi:type=\"xsd:QName\">psk:PickOne</psf:Value>",
                "</psf:Property>",
                "<psf:Property name=\"psk:DisplayName\">",
                "<psf:Value xsi:type=\"xsd:string\">Paper Source</psf:Value>",
                "</psf:Property>{options}</psf:Feature>"
            ),
            options = options
        );
        out = insert_xml_feature(&out, "JobInputBin", &block);
    }

    Some(out)
}

fn escape_xml(value: &str) -> String {
    value
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}

fn feature_span(text: &str, feature: &str) -> Option<(usize, usize)> {
    let needle = format!("*Feature: {feature}");
    let start = text.find(&needle)?;
    if start > 0 && !text[..start].ends_with('\n') {
        return None;
    }
    let open = start + text[start..].find('{')?;
    let mut depth = 0usize;
    let bytes = text.as_bytes();
    let mut index = open;
    while index < bytes.len() {
        match bytes[index] {
            b'{' => depth += 1,
            b'}' => {
                depth = depth.saturating_sub(1);
                if depth == 0 {
                    return Some((start, index + 1));
                }
            }
            _ => {}
        }
        index += 1;
    }
    None
}

fn replace_feature(text: &str, feature: &str, block: &str) -> Option<String> {
    let (start, end) = feature_span(text, feature)?;
    let mut out = String::with_capacity(text.len() + block.len());
    out.push_str(&text[..start]);
    out.push_str(block);
    out.push_str(&text[end..]);
    Some(out)
}

fn insert_feature(text: &str, feature: &str, block: &str, nl: &str) -> String {
    if let Some(updated) = replace_feature(text, feature, block) {
        return updated;
    }
    let anchor = text.find("\n*Command:");
    match anchor {
        Some(position) => {
            let mut out = String::with_capacity(text.len() + block.len() + nl.len());
            out.push_str(&text[..position + 1]);
            out.push_str(block);
            out.push_str(nl);
            out.push_str(&text[position + 1..]);
            out
        }
        None => format!("{text}{nl}{block}{nl}"),
    }
}

fn xml_feature_span(text: &str, feature: &str) -> Option<(usize, usize)> {
    let needle = format!("<psf:Feature name=\"psk:{feature}\">");
    let start = text.find(&needle)?;
    let close = "</psf:Feature>";
    let end = start + text[start..].find(close)? + close.len();
    Some((start, end))
}

fn replace_xml_feature(text: &str, feature: &str, block: &str) -> Option<String> {
    let (start, end) = xml_feature_span(text, feature)?;
    let mut out = String::with_capacity(text.len() + block.len());
    out.push_str(&text[..start]);
    out.push_str(block);
    out.push_str(&text[end..]);
    Some(out)
}

fn insert_xml_feature(text: &str, feature: &str, block: &str) -> String {
    if let Some(updated) = replace_xml_feature(text, feature, block) {
        return updated;
    }
    const ROOT_END: &str = "</psf:PrintCapabilities>";
    match text.rfind(ROOT_END) {
        Some(position) => {
            let mut out = String::with_capacity(text.len() + block.len());
            out.push_str(&text[..position]);
            out.push_str(block);
            out.push_str(&text[position..]);
            out
        }
        None => text.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const GPD: &str = concat!(
        "*MasterUnits: PAIR(457200, 457200)\r\n",
        "*Feature: PaperSize\r\n",
        "{\r\n",
        "*rcNameID: =PAPER_SIZE_DISPLAY\r\n",
        "*DefaultOption: A3\r\n",
        "*Option: A3\r\n",
        "{\r\n",
        "*rcNameID: =RCID_DMPAPER_SYSTEM_NAME\r\n",
        "*PrintSchemaKeywordMap: \"ISOA3\"\r\n",
        "*PrintableArea: PAIR(5346000, 7560000)\r\n",
        "}\r\n",
        "}\r\n",
        "*Feature: InputBin\r\n",
        "{\r\n",
        "*DefaultOption: Tray1\r\n",
        "*Option: FORMSOURCE\r\n",
        "{\r\n",
        "*rcNameID: =AUTO_DISPLAY\r\n",
        "*PrintSchemaKeywordMap: \"AutoSelect\"\r\n",
        "}\r\n",
        "}\r\n",
        "*Command: CmdCR { *Cmd : \"\" }\r\n",
    );

    fn caps() -> QueueCaps {
        QueueCaps {
            papers: vec![
                "A3".into(),
                "A4".into(),
                "Letter".into(),
                "Automatic".into(),
            ],
            sources: vec!["Tray 1".into(), "Tray 2".into()],
            duplex: true,
            duplex_short_edge: true,
        }
    }

    #[test]
    fn patches_media_duplex_and_bins() {
        let text = patch_gpd(GPD, &caps(), "A4").unwrap();
        assert!(text.contains("*Option: A4"));
        assert!(text.contains("*DefaultOption: A4"));
        assert!(text.contains("*PrintableArea: PAIR(5346000, 7560000)"));
        assert!(text.contains("*Feature: Duplex"));
        assert!(text.contains("*Option: HORIZONTAL"));
        assert!(text.contains("*Feature: InputBin"));
        assert!(text.contains("*Option: Tray2"));
        assert!(text.ends_with("*Command: CmdCR { *Cmd : \"\" }\r\n"));
        // Unknown sizes are skipped, not injected twice.
        assert_eq!(text.matches("*Option: A3").count(), 1);
    }

    #[test]
    fn patch_is_idempotent() {
        let once = patch_gpd(GPD, &caps(), "A4").unwrap();
        let twice = patch_gpd(&once, &caps(), "A4").unwrap();
        assert_eq!(once, twice);
    }

    #[test]
    fn keeps_the_queues_original_default_paper() {
        let caps = QueueCaps {
            papers: vec!["A4".into()],
            ..Default::default()
        };
        let text = patch_gpd(GPD, &caps, "A4").unwrap();
        // A3 is the queue's original default and must remain available for the
        // saved DEVMODE snapshot, even though the host no longer reports it.
        assert!(text.contains("*Option: A3"));
        assert!(text.contains("*Option: A4"));
    }

    #[test]
    fn skips_auto_sources() {
        let caps = QueueCaps::from_capabilities(&PrinterCapabilities {
            sources: vec!["自动".into(), "Tray 1".into()],
            ..Default::default()
        });
        assert_eq!(caps.sources, vec!["Tray 1"]);
    }

    #[test]
    fn builds_capability_xml() {
        let xml = concat!(
            "<psf:PrintCapabilities>",
            "<psf:Feature name=\"psk:PageMediaSize\">",
            "<psf:Option name=\"psk:ISOA3\"/>",
            "</psf:Feature>",
            "<psf:Feature name=\"psk:JobDuplexAllDocumentsContiguously\">",
            "<psf:Option name=\"psk:OneSided\"/>",
            "</psf:Feature>",
            "</psf:PrintCapabilities>"
        );
        let patched = patch_caps(xml, &caps(), None).unwrap();
        assert!(patched.contains("psk:ISOA4"));
        assert!(patched.contains("psk:TwoSidedShortEdge"));
        assert!(patched.contains("ns0001:Tray2"));
        assert!(patched.ends_with("</psf:PrintCapabilities>"));
    }
}
