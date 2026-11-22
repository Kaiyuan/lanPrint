//! Minimal IPP Everywhere server for client queues.
//!
//! The server advertises the host printer's real capabilities (media, sides,
//! colour, trays, resolution) so CUPS and the Windows IPP class driver build a
//! queue that matches the shared printer. CUPS exposes every media size and
//! tray; Windows' class driver shows duplex, colour, copies, resolution and the
//! default paper size, but keeps its own short media list. Print data is
//! forwarded to the host with the same PDF pipeline used by the in-app test
//! page.

use std::collections::HashMap;
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::Arc;

use axum::body::Bytes;
use axum::extract::{DefaultBodyLimit, Path, State};
use axum::http::{header, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::post;
use axum::Router;
use lanprint_core::{
    ClientPrinter, DuplexMode, PrinterCapabilities, PrinterPreferences, MAX_JOB_BYTES,
};
use tokio::net::TcpListener;
use tracing::{debug, warn};

use crate::app::App;

/// Loopback port used by the client-side IPP endpoint.
pub const IPP_PORT: u16 = 52336;

#[allow(dead_code)]
pub fn http_uri(id: u32) -> String {
    format!("http://127.0.0.1:{IPP_PORT}/ipp/{id}")
}

#[allow(dead_code)]
pub fn cups_uri(id: u32) -> String {
    format!("ipp://127.0.0.1:{IPP_PORT}/ipp/{id}")
}

/// Whether the local IPP endpoint is listening.
#[allow(dead_code)]
pub fn available() -> bool {
    std::net::TcpStream::connect_timeout(
        &std::net::SocketAddr::from(([127, 0, 0, 1], IPP_PORT)),
        std::time::Duration::from_millis(300),
    )
    .is_ok()
}

const GROUP_OPERATION: u8 = 0x01;
const GROUP_JOB: u8 = 0x02;
const GROUP_END: u8 = 0x03;
const GROUP_PRINTER: u8 = 0x04;

const TAG_INTEGER: u8 = 0x21;
const TAG_BOOLEAN: u8 = 0x22;
const TAG_ENUM: u8 = 0x23;
const TAG_TEXT: u8 = 0x41;
const TAG_NAME: u8 = 0x42;
const TAG_KEYWORD: u8 = 0x44;
const TAG_URI: u8 = 0x45;
const TAG_CHARSET: u8 = 0x47;
const TAG_LANGUAGE: u8 = 0x48;
const TAG_MIME: u8 = 0x49;
const TAG_RANGE: u8 = 0x33;
const TAG_RESOLUTION: u8 = 0x32;

const OP_PRINT_JOB: u16 = 0x0002;
const OP_VALIDATE_JOB: u16 = 0x0004;
const OP_CREATE_JOB: u16 = 0x0005;
const OP_SEND_DOCUMENT: u16 = 0x0006;
const OP_CANCEL_JOB: u16 = 0x0008;
const OP_GET_JOB_ATTRIBUTES: u16 = 0x0009;
const OP_GET_JOBS: u16 = 0x000A;
const OP_GET_PRINTER_ATTRIBUTES: u16 = 0x000B;
const OP_IDENTIFY_PRINTER: u16 = 0x003C;

const STATUS_OK: u16 = 0x0000;
const STATUS_BAD_REQUEST: u16 = 0x0400;
const STATUS_NOT_FOUND: u16 = 0x0406;
const STATUS_NOT_SUPPORTED: u16 = 0x0501;
const STATUS_INTERNAL: u16 = 0x0500;

#[derive(Debug, Clone)]
struct Attribute {
    name: String,
    tag: u8,
    value: Vec<u8>,
}

#[derive(Debug)]
struct IppRequest {
    version: (u8, u8),
    operation: u16,
    request_id: u32,
    attributes: Vec<Attribute>,
    data: Vec<u8>,
}

impl IppRequest {
    fn parse(body: &[u8]) -> Option<Self> {
        if body.len() < 8 {
            return None;
        }
        let version = (body[0], body[1]);
        let operation = u16::from_be_bytes([body[2], body[3]]);
        let request_id = u32::from_be_bytes([body[4], body[5], body[6], body[7]]);
        let mut attributes = Vec::new();
        let mut index = 8;
        let mut data = Vec::new();
        while index < body.len() {
            let tag = body[index];
            index += 1;
            if tag == GROUP_END {
                data.extend_from_slice(&body[index..]);
                break;
            }
            if tag <= 0x05 {
                continue;
            }
            if index + 2 > body.len() {
                return None;
            }
            let name_len = u16::from_be_bytes([body[index], body[index + 1]]) as usize;
            index += 2;
            if index + name_len + 2 > body.len() {
                return None;
            }
            let name = String::from_utf8_lossy(&body[index..index + name_len]).to_string();
            index += name_len;
            let value_len = u16::from_be_bytes([body[index], body[index + 1]]) as usize;
            index += 2;
            if index + value_len > body.len() {
                return None;
            }
            attributes.push(Attribute {
                name,
                tag,
                value: body[index..index + value_len].to_vec(),
            });
            index += value_len;
        }
        Some(Self {
            version,
            operation,
            request_id,
            attributes,
            data,
        })
    }

    fn string(&self, name: &str) -> Option<String> {
        self.attributes
            .iter()
            .find(|attribute| attribute.name.eq_ignore_ascii_case(name))
            .map(|attribute| String::from_utf8_lossy(&attribute.value).to_string())
    }

    fn integer(&self, name: &str) -> Option<i32> {
        self.attributes
            .iter()
            .find(|attribute| attribute.name.eq_ignore_ascii_case(name))
            .filter(|attribute| attribute.value.len() == 4)
            .map(|attribute| {
                i32::from_be_bytes([
                    attribute.value[0],
                    attribute.value[1],
                    attribute.value[2],
                    attribute.value[3],
                ])
            })
    }

    fn resolution(&self, name: &str) -> Option<u32> {
        self.attributes
            .iter()
            .find(|attribute| attribute.name.eq_ignore_ascii_case(name))
            .filter(|attribute| attribute.value.len() >= 8)
            .map(|attribute| {
                u32::from_be_bytes([
                    attribute.value[0],
                    attribute.value[1],
                    attribute.value[2],
                    attribute.value[3],
                ])
            })
    }
}

fn text(name: &str, value: &str) -> Attribute {
    Attribute {
        name: name.into(),
        tag: TAG_TEXT,
        value: value.as_bytes().to_vec(),
    }
}

fn name_attr(name: &str, value: &str) -> Attribute {
    Attribute {
        name: name.into(),
        tag: TAG_NAME,
        value: value.as_bytes().to_vec(),
    }
}

fn keyword(name: &str, value: &str) -> Attribute {
    Attribute {
        name: name.into(),
        tag: TAG_KEYWORD,
        value: value.as_bytes().to_vec(),
    }
}

fn uri(name: &str, value: &str) -> Attribute {
    Attribute {
        name: name.into(),
        tag: TAG_URI,
        value: value.as_bytes().to_vec(),
    }
}

fn charset(name: &str, value: &str) -> Attribute {
    Attribute {
        name: name.into(),
        tag: TAG_CHARSET,
        value: value.as_bytes().to_vec(),
    }
}

fn language(name: &str, value: &str) -> Attribute {
    Attribute {
        name: name.into(),
        tag: TAG_LANGUAGE,
        value: value.as_bytes().to_vec(),
    }
}

fn mime(name: &str, value: &str) -> Attribute {
    Attribute {
        name: name.into(),
        tag: TAG_MIME,
        value: value.as_bytes().to_vec(),
    }
}

fn integer(name: &str, value: i32) -> Attribute {
    Attribute {
        name: name.into(),
        tag: TAG_INTEGER,
        value: value.to_be_bytes().to_vec(),
    }
}

fn boolean(name: &str, value: bool) -> Attribute {
    Attribute {
        name: name.into(),
        tag: TAG_BOOLEAN,
        value: vec![u8::from(value)],
    }
}

fn enum_attr(name: &str, value: i32) -> Attribute {
    Attribute {
        name: name.into(),
        tag: TAG_ENUM,
        value: value.to_be_bytes().to_vec(),
    }
}

fn range(name: &str, low: i32, high: i32) -> Attribute {
    let mut value = Vec::with_capacity(8);
    value.extend_from_slice(&low.to_be_bytes());
    value.extend_from_slice(&high.to_be_bytes());
    Attribute {
        name: name.into(),
        tag: TAG_RANGE,
        value,
    }
}

fn resolution(name: &str, dpi: u32) -> Attribute {
    let mut value = Vec::with_capacity(9);
    value.extend_from_slice(&dpi.to_be_bytes());
    value.extend_from_slice(&dpi.to_be_bytes());
    value.push(3); // dots per inch
    Attribute {
        name: name.into(),
        tag: TAG_RESOLUTION,
        value,
    }
}

fn operation_attributes() -> Vec<Attribute> {
    vec![
        charset("attributes-charset", "utf-8"),
        language("attributes-natural-language", "en"),
    ]
}

fn response(
    request: &IppRequest,
    status: u16,
    groups: Vec<(u8, Vec<Attribute>)>,
    message: &str,
) -> Vec<u8> {
    let mut out = Vec::new();
    out.push(if request.version.0 == 2 { 2 } else { 1 });
    out.push(if request.version.0 == 2 { 0 } else { 1 });
    out.extend_from_slice(&status.to_be_bytes());
    out.extend_from_slice(&request.request_id.to_be_bytes());

    let mut operation = operation_attributes();
    if !message.is_empty() {
        operation.push(Attribute {
            name: "status-message".into(),
            tag: TAG_TEXT,
            value: message.as_bytes().to_vec(),
        });
    }
    write_group(&mut out, GROUP_OPERATION, &operation);
    for (group, attributes) in groups {
        write_group(&mut out, group, &attributes);
    }
    out.push(GROUP_END);
    out
}

fn write_group(out: &mut Vec<u8>, group: u8, attributes: &[Attribute]) {
    if attributes.is_empty() {
        return;
    }
    out.push(group);
    for attribute in attributes {
        out.push(attribute.tag);
        let name = attribute.name.as_bytes();
        out.extend_from_slice(&(name.len() as u16).to_be_bytes());
        out.extend_from_slice(name);
        out.extend_from_slice(&(attribute.value.len() as u16).to_be_bytes());
        out.extend_from_slice(&attribute.value);
    }
}

#[derive(Clone)]
struct IppState {
    app: Arc<App>,
    next_job: Arc<AtomicU32>,
    pending: Arc<tokio::sync::Mutex<HashMap<u32, PendingJob>>>,
}

struct PendingJob {
    printer_id: u32,
    prefs: PrinterPreferences,
    filename: String,
}

pub async fn serve(app: Arc<App>) -> std::io::Result<()> {
    let listener = TcpListener::bind(("127.0.0.1", IPP_PORT)).await?;
    let state = IppState {
        app,
        next_job: Arc::new(AtomicU32::new(1)),
        pending: Arc::new(tokio::sync::Mutex::new(HashMap::new())),
    };
    let router = Router::new()
        .route("/ipp/{id}", post(handle))
        .layer(DefaultBodyLimit::max(MAX_JOB_BYTES + 256 * 1024))
        .with_state(state);
    axum::serve(listener, router).await
}

async fn handle(State(state): State<IppState>, Path(id): Path<u32>, body: Bytes) -> Response {
    let Some(request) = IppRequest::parse(&body) else {
        let dummy = IppRequest {
            version: (1, 1),
            operation: 0,
            request_id: 0,
            attributes: Vec::new(),
            data: Vec::new(),
        };
        return ipp_response(response(
            &dummy,
            STATUS_BAD_REQUEST,
            Vec::new(),
            "bad request",
        ));
    };
    let Some(client) = state.app.client_printer(id) else {
        return ipp_response(response(
            &request,
            STATUS_NOT_FOUND,
            Vec::new(),
            "printer not found",
        ));
    };
    match request.operation {
        OP_GET_PRINTER_ATTRIBUTES => {
            let attrs = printer_attributes(&client);
            ipp_response(response(
                &request,
                STATUS_OK,
                vec![(GROUP_PRINTER, attrs)],
                "",
            ))
        }
        OP_VALIDATE_JOB => ipp_response(response(&request, STATUS_OK, Vec::new(), "")),
        OP_PRINT_JOB => {
            if pdf_payload(&request.data).is_none() {
                return ipp_response(response(
                    &request,
                    0x040A,
                    Vec::new(),
                    "only PDF documents are supported",
                ));
            }
            let prefs = prefs_from_request(&request, &client);
            let filename = job_name(&request);
            let job_id = state.next_job.fetch_add(1, Ordering::Relaxed);
            match forward(&state.app, &client, &request.data, &filename, &prefs).await {
                Ok(()) => ipp_response(response(
                    &request,
                    STATUS_OK,
                    vec![(GROUP_JOB, job_attributes(&request, 9, job_id))],
                    "",
                )),
                Err(err) => {
                    warn!("IPP print job failed: {err}");
                    ipp_response(response(
                        &request,
                        STATUS_INTERNAL,
                        Vec::new(),
                        &err.to_string(),
                    ))
                }
            }
        }
        OP_CREATE_JOB => {
            let job_id = state.next_job.fetch_add(1, Ordering::Relaxed);
            let prefs = prefs_from_request(&request, &client);
            let filename = job_name(&request);
            state.pending.lock().await.insert(
                job_id,
                PendingJob {
                    printer_id: id,
                    prefs,
                    filename,
                },
            );
            ipp_response(response(
                &request,
                STATUS_OK,
                vec![(GROUP_JOB, job_attributes(&request, 3, job_id))],
                "",
            ))
        }
        OP_SEND_DOCUMENT => {
            if pdf_payload(&request.data).is_none() {
                return ipp_response(response(
                    &request,
                    0x040A,
                    Vec::new(),
                    "only PDF documents are supported",
                ));
            }
            let job_id = request.integer("job-id").unwrap_or(0) as u32;
            let pending = state.pending.lock().await.remove(&job_id);
            let Some(pending) = pending else {
                return ipp_response(response(
                    &request,
                    STATUS_BAD_REQUEST,
                    Vec::new(),
                    "unknown job",
                ));
            };
            if pending.printer_id != id {
                return ipp_response(response(
                    &request,
                    STATUS_BAD_REQUEST,
                    Vec::new(),
                    "wrong printer",
                ));
            }
            match forward(
                &state.app,
                &client,
                &request.data,
                &pending.filename,
                &pending.prefs,
            )
            .await
            {
                Ok(()) => ipp_response(response(
                    &request,
                    STATUS_OK,
                    vec![(GROUP_JOB, job_attributes(&request, 9, job_id))],
                    "",
                )),
                Err(err) => {
                    warn!("IPP send-document failed: {err}");
                    ipp_response(response(
                        &request,
                        STATUS_INTERNAL,
                        Vec::new(),
                        &err.to_string(),
                    ))
                }
            }
        }
        OP_CANCEL_JOB => {
            let job_id = request.integer("job-id").unwrap_or(0) as u32;
            state.pending.lock().await.remove(&job_id);
            ipp_response(response(&request, STATUS_OK, Vec::new(), ""))
        }
        OP_GET_JOB_ATTRIBUTES => {
            let job_id = request.integer("job-id").unwrap_or(0) as u32;
            ipp_response(response(
                &request,
                STATUS_OK,
                vec![(GROUP_JOB, job_attributes(&request, 9, job_id))],
                "",
            ))
        }
        OP_GET_JOBS => ipp_response(response(&request, STATUS_OK, Vec::new(), "")),
        OP_IDENTIFY_PRINTER => ipp_response(response(&request, STATUS_OK, Vec::new(), "")),
        _ => ipp_response(response(
            &request,
            STATUS_NOT_SUPPORTED,
            Vec::new(),
            "operation not supported",
        )),
    }
}

fn ipp_response(body: Vec<u8>) -> Response {
    (
        StatusCode::OK,
        [(header::CONTENT_TYPE, "application/ipp")],
        body,
    )
        .into_response()
}

fn pdf_payload(data: &[u8]) -> Option<&[u8]> {
    if data.starts_with(b"%PDF") {
        return Some(data);
    }
    data.windows(4)
        .take(1024)
        .position(|window| window == b"%PDF")
        .map(|offset| &data[offset..])
}

async fn forward(
    app: &Arc<App>,
    client: &ClientPrinter,
    data: &[u8],
    filename: &str,
    prefs: &PrinterPreferences,
) -> crate::error::Result<()> {
    let Some(payload) = pdf_payload(data) else {
        return Err(crate::error::AppError::Print(
            "IPP document is not PDF".into(),
        ));
    };
    if payload.is_empty() {
        return Err(crate::error::AppError::Print("empty IPP document".into()));
    }
    debug!(
        "IPP job for {} via {}: {} bytes",
        client.local_name,
        client.remote_name,
        payload.len()
    );
    app.send_print(
        &client.device_id,
        &client.remote_name,
        &client.token,
        payload.to_vec(),
        "application/pdf",
        if filename.is_empty() {
            "lanprint-job.pdf"
        } else {
            filename
        },
        prefs,
    )
    .await
}

fn job_name(request: &IppRequest) -> String {
    request
        .string("job-name")
        .filter(|name| !name.is_empty())
        .unwrap_or_else(|| "lanPrint job".into())
}

fn job_attributes(request: &IppRequest, state: i32, job_id: u32) -> Vec<Attribute> {
    let id = if job_id == 0 {
        request.integer("job-id").unwrap_or(0).max(1) as u32
    } else {
        job_id
    };
    vec![
        uri("job-uri", &format!("ipp://127.0.0.1:{IPP_PORT}/jobs/{id}")),
        integer("job-id", id as i32),
        enum_attr("job-state", state),
        keyword(
            "job-state-reasons",
            if state == 9 {
                "job-completed-successfully"
            } else {
                "none"
            },
        ),
    ]
}

fn prefs_from_request(request: &IppRequest, client: &ClientPrinter) -> PrinterPreferences {
    let mut prefs = client.preferences.clone();
    if let Some(sides) = request.string("sides") {
        prefs.duplex = match sides.as_str() {
            "two-sided-long-edge" => DuplexMode::LongEdge,
            "two-sided-short-edge" => DuplexMode::ShortEdge,
            _ => DuplexMode::Simplex,
        };
    }
    if let Some(mode) = request.string("print-color-mode") {
        prefs.color = !mode.eq_ignore_ascii_case("monochrome");
    }
    if let Some(media) = request.string("media") {
        prefs.paper = media_to_friendly(&media, &client.capabilities);
    }
    if let Some(source) = request.string("media-source") {
        prefs.media_source = source_to_friendly(&source, &client.capabilities);
    }
    if let Some(copies) = request.integer("copies") {
        prefs.copies = copies.max(1) as u32;
    }
    if let Some(dpi) = request.resolution("printer-resolution") {
        prefs.resolution = dpi;
    }
    prefs
}

fn media_to_friendly(media: &str, caps: &PrinterCapabilities) -> String {
    let media = media.trim();
    if let Some(paper) = caps
        .papers
        .iter()
        .find(|paper| pwg_media(paper).eq_ignore_ascii_case(media))
    {
        return paper.clone();
    }
    if let Some(size) = crate::media::MEDIA.iter().find(|size| {
        let prefix = size
            .pwg
            .split('_')
            .take(2)
            .collect::<Vec<_>>()
            .join("_");
        media.to_ascii_lowercase().starts_with(&prefix)
    }) {
        return size.name.to_string();
    }
    media.to_string()
}

fn source_to_friendly(source: &str, caps: &PrinterCapabilities) -> String {
    let normalized = normalize_source(source);
    caps.sources
        .iter()
        .find(|candidate| {
            let value = normalize_source(candidate);
            value == normalized || value.contains(&normalized) || normalized.contains(&value)
        })
        .cloned()
        .unwrap_or_else(|| source.to_string())
}

fn normalize_source(value: &str) -> String {
    value
        .to_ascii_lowercase()
        .replace([' ', '_'], "-")
        .trim_matches('-')
        .to_string()
}

fn pwg_media(paper: &str) -> String {
    crate::media::pwg_media(paper)
}

/// Paper dimensions in hundredths of a millimetre, as required by
/// `media-size-supported`.
fn media_dimensions(paper: &str) -> Option<(i32, i32)> {
    crate::media::dimensions(paper)
}

/// Encode a `media-size-supported` collection (IPP collection syntax).
fn media_size(paper: &str) -> Vec<Attribute> {
    let Some((x, y)) = media_dimensions(paper) else {
        return Vec::new();
    };
    let mut attrs = vec![collection_start("media-size-supported")];
    attrs.extend(collection_member_name("x-dimension"));
    attrs.push(collection_integer(x));
    attrs.extend(collection_member_name("y-dimension"));
    attrs.push(collection_integer(y));
    attrs.push(collection_end());
    attrs
}

/// Encode a `media-col-database` / `media-col-default` entry. CUPS and macOS
/// build their page-size and tray lists from these collections; Windows' IPP
/// class driver only exposes the default media size in its print dialog.
fn media_col(name: &str, paper: &str) -> Vec<Attribute> {
    let Some((x, y)) = media_dimensions(paper) else {
        return Vec::new();
    };
    let mut attrs = vec![collection_start(name)];
    attrs.extend(collection_member_name("media-size"));
    attrs.push(collection_start(""));
    attrs.extend(collection_member_name("x-dimension"));
    attrs.push(collection_integer(x));
    attrs.extend(collection_member_name("y-dimension"));
    attrs.push(collection_integer(y));
    attrs.push(collection_end());
    attrs.push(collection_end());
    attrs
}

fn collection_start(name: &str) -> Attribute {
    Attribute {
        name: name.into(),
        tag: 0x34,
        value: Vec::new(),
    }
}

fn collection_member_name(name: &str) -> Vec<Attribute> {
    vec![Attribute {
        name: String::new(),
        tag: 0x4A,
        value: name.as_bytes().to_vec(),
    }]
}

fn collection_integer(value: i32) -> Attribute {
    Attribute {
        name: String::new(),
        tag: TAG_INTEGER,
        value: value.to_be_bytes().to_vec(),
    }
}

fn collection_end() -> Attribute {
    Attribute {
        name: String::new(),
        tag: 0x37,
        value: Vec::new(),
    }
}

fn printer_attributes(client: &ClientPrinter) -> Vec<Attribute> {
    let caps = &client.capabilities;
    let mut duplex_modes: Vec<&str> = Vec::new();
    for mode in &caps.duplex_modes {
        if mode.eq_ignore_ascii_case("short_edge") {
            duplex_modes.push("two-sided-short-edge");
        } else if mode.eq_ignore_ascii_case("long_edge") {
            duplex_modes.push("two-sided-long-edge");
        }
    }
    if duplex_modes.is_empty() && caps.duplex {
        duplex_modes.push("two-sided-long-edge");
        duplex_modes.push("two-sided-short-edge");
    }
    let preference = client.preferences.duplex.clone();
    let default_sides = match preference {
        DuplexMode::Simplex => "one-sided",
        mode => {
            let preferred = if mode == DuplexMode::ShortEdge {
                "two-sided-short-edge"
            } else {
                "two-sided-long-edge"
            };
            let fallback = if preferred == "two-sided-short-edge" {
                "two-sided-long-edge"
            } else {
                "two-sided-short-edge"
            };
            if duplex_modes.contains(&preferred) {
                preferred
            } else if duplex_modes.contains(&fallback) {
                fallback
            } else {
                "one-sided"
            }
        }
    };
    let mut attrs = vec![
        uri("printer-uri-supported", &cups_uri(client.id)),
        keyword("uri-security-supported", "none"),
        keyword("uri-authentication-supported", "none"),
        name_attr("printer-name", &client.local_name),
        uri(
            "printer-uuid",
            &format!("urn:uuid:00000000-0000-4000-8000-{:012x}", client.id),
        ),
        text("printer-info", &client.display_name),
        text(
            "printer-make-and-model",
            if caps.make_model.is_empty() {
                &client.display_name
            } else {
                &caps.make_model
            },
        ),
        enum_attr("printer-state", 3),
        keyword("printer-state-reasons", "none"),
        boolean("printer-is-accepting-jobs", true),
        keyword("ipp-versions-supported", "1.1"),
        keyword("ipp-versions-supported", "2.0"),
        charset("charset-configured", "utf-8"),
        charset("charset-supported", "utf-8"),
        language("natural-language-configured", "en"),
        language("generated-natural-language-supported", "en"),
        mime("document-format-default", "application/pdf"),
        mime("document-format-supported", "application/pdf"),
        mime("document-format-preferred", "application/pdf"),
        text(
            "printer-device-id",
            &format!(
                "MFG:lanPrint;MDL:{};CMD:PDF;",
                if caps.make_model.is_empty() {
                    &client.display_name
                } else {
                    &caps.make_model
                }
            ),
        ),
        enum_attr("print-quality-default", 4),
        enum_attr("print-quality-supported", 4),
        enum_attr("orientation-requested-default", 3),
        enum_attr("orientation-requested-supported", 3),
        enum_attr("orientation-requested-supported", 4),
        enum_attr("orientation-requested-supported", 5),
        enum_attr("orientation-requested-supported", 6),
        boolean("color-supported", caps.color),
        keyword(
            "print-color-mode-default",
            if client.preferences.color {
                "color"
            } else {
                "monochrome"
            },
        ),
        keyword("sides-default", default_sides),
        keyword(
            "media-default",
            &pwg_media(if client.preferences.paper.is_empty() {
                "A4"
            } else {
                &client.preferences.paper
            }),
        ),
        integer("copies-default", 1),
        keyword("job-creation-attributes-supported", "copies"),
        keyword("job-creation-attributes-supported", "media"),
        keyword("job-creation-attributes-supported", "sides"),
        keyword("job-creation-attributes-supported", "print-color-mode"),
        keyword("job-creation-attributes-supported", "media-source"),
        keyword("job-creation-attributes-supported", "printer-resolution"),
        keyword("job-creation-attributes-supported", "job-name"),
        keyword("job-creation-attributes-supported", "print-quality"),
        keyword("job-creation-attributes-supported", "orientation-requested"),
        enum_attr("operations-supported", OP_PRINT_JOB as i32),
        enum_attr("operations-supported", OP_VALIDATE_JOB as i32),
        enum_attr("operations-supported", OP_CREATE_JOB as i32),
        enum_attr("operations-supported", OP_SEND_DOCUMENT as i32),
        enum_attr("operations-supported", OP_CANCEL_JOB as i32),
        enum_attr("operations-supported", OP_GET_JOB_ATTRIBUTES as i32),
        enum_attr("operations-supported", OP_GET_JOBS as i32),
        enum_attr("operations-supported", OP_GET_PRINTER_ATTRIBUTES as i32),
        enum_attr("operations-supported", OP_IDENTIFY_PRINTER as i32),
    ];
    if caps.color {
        attrs.push(keyword("print-color-mode-supported", "color"));
        attrs.push(keyword("print-color-mode-supported", "monochrome"));
    } else {
        attrs.push(keyword("print-color-mode-supported", "monochrome"));
    }
    attrs.push(keyword("sides-supported", "one-sided"));
    for side in &duplex_modes {
        attrs.push(keyword("sides-supported", side));
    }
    if !caps.papers.is_empty() {
        for paper in &caps.papers {
            attrs.push(keyword("media-supported", &pwg_media(paper)));
        }
        for paper in &caps.papers {
            attrs.extend(media_size(paper));
        }
        for paper in &caps.papers {
            attrs.extend(media_col("media-col-database", paper));
        }
    }
    attrs.extend(media_col(
        "media-col-default",
        if client.preferences.paper.is_empty() {
            "A4"
        } else {
            &client.preferences.paper
        },
    ));
    if !caps.sources.is_empty() {
        attrs.push(keyword(
            "media-source-default",
            &normalize_source(if client.preferences.media_source.is_empty() {
                &caps.sources[0]
            } else {
                &client.preferences.media_source
            }),
        ));
        for source in &caps.sources {
            attrs.push(keyword("media-source-supported", &normalize_source(source)));
        }
    }
    let resolutions: Vec<u32> = if caps.resolutions.is_empty() {
        vec![300, 600]
    } else {
        caps.resolutions.clone()
    };
    for dpi in &resolutions {
        attrs.push(resolution("printer-resolution-supported", *dpi));
    }
    let default_dpi = if client.preferences.resolution > 0
        && resolutions.contains(&client.preferences.resolution)
    {
        client.preferences.resolution
    } else {
        resolutions[0]
    };
    attrs.push(resolution("printer-resolution-default", default_dpi));
    let max_copies = if caps.max_copies > 1 {
        caps.max_copies as i32
    } else {
        99
    };
    attrs.push(range("copies-supported", 1, max_copies));
    attrs
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trips_minimal_request() {
        let mut body = Vec::new();
        body.extend_from_slice(&[0x02, 0x00]);
        body.extend_from_slice(&OP_GET_PRINTER_ATTRIBUTES.to_be_bytes());
        body.extend_from_slice(&7u32.to_be_bytes());
        body.push(GROUP_OPERATION);
        for attribute in operation_attributes() {
            body.push(attribute.tag);
            body.extend_from_slice(&(attribute.name.len() as u16).to_be_bytes());
            body.extend_from_slice(attribute.name.as_bytes());
            body.extend_from_slice(&(attribute.value.len() as u16).to_be_bytes());
            body.extend_from_slice(&attribute.value);
        }
        body.push(GROUP_END);
        let request = IppRequest::parse(&body).unwrap();
        assert_eq!(request.operation, OP_GET_PRINTER_ATTRIBUTES);
        assert_eq!(
            request.string("attributes-charset").as_deref(),
            Some("utf-8")
        );
    }

    #[test]
    fn maps_paper_names() {
        assert_eq!(pwg_media("A4"), "iso_a4_210x297mm");
        assert_eq!(pwg_media("Letter"), "na_letter_8.5x11in");
        assert_eq!(pwg_media("A6"), "iso_a6_105x148mm");
    }

    fn client_with(duplex_modes: &[&str]) -> ClientPrinter {
        let mut client = ClientPrinter::default();
        client.id = 7;
        client.local_name = "lanPrint-Test".into();
        client.display_name = "Test".into();
        client.capabilities.duplex = !duplex_modes.is_empty();
        client.capabilities.color = true;
        client.capabilities.duplex_modes =
            duplex_modes.iter().map(|mode| mode.to_string()).collect();
        client.capabilities.papers = vec!["A4".into(), "A3".into()];
        client.capabilities.sources = vec!["Tray 1".into()];
        client.preferences.duplex = DuplexMode::ShortEdge;
        client
    }

    fn values(attrs: &[Attribute], name: &str) -> Vec<String> {
        attrs
            .iter()
            .filter(|attribute| attribute.name == name)
            .map(|attribute| String::from_utf8_lossy(&attribute.value).into_owned())
            .collect()
    }

    #[test]
    fn limits_sides_to_host_modes() {
        let attrs = printer_attributes(&client_with(&["long_edge"]));
        assert_eq!(
            values(&attrs, "sides-supported"),
            vec!["one-sided", "two-sided-long-edge"]
        );
        // Short edge is not available on this host, so the default degrades.
        assert_eq!(values(&attrs, "sides-default"), vec!["two-sided-long-edge"]);
    }

    #[test]
    fn advertises_both_duplex_modes() {
        let attrs = printer_attributes(&client_with(&["long_edge", "short_edge"]));
        assert_eq!(
            values(&attrs, "sides-supported"),
            vec!["one-sided", "two-sided-long-edge", "two-sided-short-edge"]
        );
        assert_eq!(
            values(&attrs, "sides-default"),
            vec!["two-sided-short-edge"]
        );
    }

    #[test]
    fn keeps_multi_value_attributes_together() {
        let attrs = printer_attributes(&client_with(&["long_edge"]));
        let mut seen: Vec<&str> = Vec::new();
        for attribute in attrs.iter().filter(|attribute| !attribute.name.is_empty()) {
            if seen.last() == Some(&attribute.name.as_str()) {
                continue;
            }
            assert!(
                !seen.contains(&attribute.name.as_str()),
                "{} is not contiguous",
                attribute.name
            );
            seen.push(&attribute.name);
        }
    }
}
