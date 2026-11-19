//! PDF rendering and printing for Windows hosts.
//!
//! Client virtual printers render their jobs to PDF. The host turns each page
//! into a bitmap with the PDF engine that ships with Windows and sends it to
//! the real printer through GDI. Printing through GDI (instead of asking a PDF
//! viewer to print) keeps the job silent and lets us apply the driver's own
//! DEVMODE settings, so duplex, paper, colour, trays, copies and booklet
//! imposition behave exactly like a local print job.

use std::mem::size_of;
use std::ptr::{null, null_mut};

use lanprint_core::{DuplexMode, PrinterPreferences};
use tracing::debug;
use winapi::shared::minwindef::{DWORD, WORD};
use winapi::shared::windef::{HDC, POINT};
use winapi::um::wingdi::{
    CreateDCW, DeleteDC, DeviceCapabilitiesW, EndDoc, EndPage, GetDeviceCaps, SetStretchBltMode,
    StartDocW, StartPage, StretchDIBits, BITMAPINFO, BITMAPINFOHEADER, BI_RGB, DC_BINNAMES,
    DC_BINS, DC_COLORDEVICE, DC_COPIES, DC_DUPLEX, DC_ENUMRESOLUTIONS, DC_PAPERS, DC_PAPERSIZE,
    DIB_RGB_COLORS, DMCOLLATE_FALSE, DMCOLLATE_TRUE, DMCOLOR_COLOR, DMCOLOR_MONOCHROME,
    DMDUP_HORIZONTAL, DMDUP_SIMPLEX, DMDUP_VERTICAL, DMORIENT_LANDSCAPE, DMORIENT_PORTRAIT,
    DM_COLLATE, DM_COLOR, DM_COPIES, DM_DEFAULTSOURCE, DM_DUPLEX, DM_ORIENTATION, DM_PAPERSIZE,
    DM_PRINTQUALITY, DOCINFOW, HALFTONE, HORZRES, LOGPIXELSX, LOGPIXELSY, SRCCOPY, VERTRES,
};
use winapi::um::winspool::{ClosePrinter, GetPrinterW, OpenPrinterW, PRINTER_INFO_2W};
use windows::Data::Pdf::{PdfDocument, PdfPageRenderOptions};
use windows::Storage::Streams::{DataReader, DataWriter, InMemoryRandomAccessStream};
use windows::Win32::System::WinRT::{RoInitialize, RO_INIT_MULTITHREADED};

use crate::error::{AppError, Result};

const DEFAULT_DPI: u32 = 300;
const MAX_DPI: u32 = 600;

pub fn print_pdf(printer: &str, data: &[u8], prefs: &PrinterPreferences) -> Result<()> {
    let pdf = PdfSource::open(data)?;
    let pages = pdf.page_count()?;
    if pages == 0 {
        return Err(AppError::Print("PDF document has no pages".into()));
    }
    let dpi = if prefs.resolution > 0 {
        prefs.resolution.clamp(72, MAX_DPI)
    } else {
        DEFAULT_DPI
    };
    let booklet = prefs.booklet;
    let landscape = !booklet && pdf.page_is_landscape(0)?;

    let mut devmode = DevMode::from_printer(printer);
    if let Some(dm) = devmode.as_mut() {
        apply_preferences(printer, dm, prefs, landscape, booklet);
    }
    let dc = DeviceContext::create(printer, devmode.as_ref())?;
    let (paper_w, paper_h) = dc.printable();
    let (dpi_x, dpi_y) = dc.dpi();
    unsafe {
        SetStretchBltMode(dc.0, HALFTONE);
    }

    let doc_name = wide("lanPrint document");
    let info = DOCINFOW {
        cbSize: size_of::<DOCINFOW>() as i32,
        lpszDocName: doc_name.as_ptr(),
        lpszOutput: null(),
        lpszDatatype: null(),
        fwType: 0,
    };
    if unsafe { StartDocW(dc.0, &info) } <= 0 {
        return Err(last_error(format!("StartDoc failed for {printer}")));
    }

    let outcome = if booklet {
        print_booklet(&pdf, dc.0, (paper_w, paper_h), (dpi_x, dpi_y), dpi, pages)
    } else {
        print_pages(&pdf, dc.0, (paper_w, paper_h), dpi, pages)
    };
    unsafe {
        EndDoc(dc.0);
    }
    outcome?;
    debug!("printed PDF to {printer} ({pages} page(s), booklet={booklet})");
    Ok(())
}

fn print_pages(pdf: &PdfSource, dc: HDC, area: (i32, i32), dpi: u32, pages: u32) -> Result<()> {
    for index in 0..pages {
        let image = pdf.render(index, dpi)?;
        begin_page(dc)?;
        let drawn = draw_page(dc, &image, area);
        end_page(dc)?;
        drawn?;
    }
    Ok(())
}

fn print_booklet(
    pdf: &PdfSource,
    dc: HDC,
    area: (i32, i32),
    printer_dpi: (i32, i32),
    dpi: u32,
    pages: u32,
) -> Result<()> {
    let sheets = pages.div_ceil(4);
    // Working pixels for one printed side at the requested rendering DPI.
    let canvas_w =
        ((area.0 as f64 * dpi as f64 / printer_dpi.0.max(1) as f64).round() as u32).max(2);
    let canvas_h =
        ((area.1 as f64 * dpi as f64 / printer_dpi.1.max(1) as f64).round() as u32).max(2);
    for sheet in 0..sheets {
        let front = [4 * sheets - 1 - 2 * sheet, 2 * sheet];
        let back = [2 * sheet + 1, 4 * sheets - 2 - 2 * sheet];
        for pair in [front, back] {
            let mut canvas = vec![255u8; canvas_w as usize * canvas_h as usize * 4];
            for (slot, page) in pair.into_iter().enumerate() {
                if page >= pages {
                    continue;
                }
                let image = pdf.render(page, dpi)?;
                let slot_w = canvas_w / 2;
                let gutter = (slot_w / 40).max(4);
                let x = slot as u32 * slot_w + gutter;
                let y = gutter;
                let w = slot_w.saturating_sub(2 * gutter);
                let h = canvas_h.saturating_sub(2 * gutter);
                let (dx, dy, dw, dh) = fit(image.width, image.height, w, h);
                draw_scaled(
                    &mut canvas,
                    canvas_w,
                    canvas_h,
                    &image,
                    (x as i32 + dx, y as i32 + dy, dw as i32, dh as i32),
                );
            }
            // The spooler flips the back side itself (we print with long-edge
            // duplex), so both sides are drawn the way the reader sees them.
            let side_image = PageImage {
                width: canvas_w,
                height: canvas_h,
                bgra: canvas,
            };
            begin_page(dc)?;
            let drawn = draw_page(dc, &side_image, area);
            end_page(dc)?;
            drawn?;
        }
    }
    Ok(())
}

fn begin_page(dc: HDC) -> Result<()> {
    if unsafe { StartPage(dc) } <= 0 {
        return Err(last_error("StartPage failed".into()));
    }
    Ok(())
}

fn end_page(dc: HDC) -> Result<()> {
    if unsafe { EndPage(dc) } <= 0 {
        return Err(last_error("EndPage failed".into()));
    }
    Ok(())
}

fn draw_page(dc: HDC, image: &PageImage, area: (i32, i32)) -> Result<()> {
    let (x, y, w, h) = fit(
        image.width,
        image.height,
        area.0.max(1) as u32,
        area.1.max(1) as u32,
    );
    let mut bmi: BITMAPINFO = unsafe { std::mem::zeroed() };
    bmi.bmiHeader.biSize = size_of::<BITMAPINFOHEADER>() as u32;
    bmi.bmiHeader.biWidth = image.width as i32;
    bmi.bmiHeader.biHeight = -(image.height as i32);
    bmi.bmiHeader.biPlanes = 1;
    bmi.bmiHeader.biBitCount = 32;
    bmi.bmiHeader.biCompression = BI_RGB;
    let ok = unsafe {
        StretchDIBits(
            dc,
            x,
            y,
            w as i32,
            h as i32,
            0,
            0,
            image.width as i32,
            image.height as i32,
            image.bgra.as_ptr() as *const _,
            &bmi,
            DIB_RGB_COLORS,
            SRCCOPY,
        )
    };
    if ok == 0 {
        return Err(last_error("StretchDIBits failed".into()));
    }
    Ok(())
}

/// Fit `src` into `dst` while preserving the aspect ratio, centred.
fn fit(src_w: u32, src_h: u32, dst_w: u32, dst_h: u32) -> (i32, i32, u32, u32) {
    if src_w == 0 || src_h == 0 || dst_w == 0 || dst_h == 0 {
        return (0, 0, dst_w.max(1), dst_h.max(1));
    }
    let scale = (dst_w as f64 / src_w as f64).min(dst_h as f64 / src_h as f64);
    let w = (src_w as f64 * scale).round().max(1.0) as u32;
    let h = (src_h as f64 * scale).round().max(1.0) as u32;
    (((dst_w - w) / 2) as i32, ((dst_h - h) / 2) as i32, w, h)
}

/// Bilinear scale of `src` into the `rect` of a top-down BGRA `dst`.
fn draw_scaled(
    dst: &mut [u8],
    dst_w: u32,
    dst_h: u32,
    src: &PageImage,
    rect: (i32, i32, i32, i32),
) {
    let (rx, ry, rw, rh) = rect;
    if rw <= 0 || rh <= 0 || src.width == 0 || src.height == 0 {
        return;
    }
    let step_x = src.width as f64 / rw as f64;
    let step_y = src.height as f64 / rh as f64;
    for y in 0..rh {
        let fy = (y as f64 + 0.5) * step_y - 0.5;
        let y0 = fy.floor().max(0.0) as u32;
        let y1 = (y0 + 1).min(src.height - 1);
        let wy = (fy - y0 as f64).clamp(0.0, 1.0);
        let dy = ry + y;
        if dy < 0 || dy >= dst_h as i32 {
            continue;
        }
        for x in 0..rw {
            let fx = (x as f64 + 0.5) * step_x - 0.5;
            let x0 = fx.floor().max(0.0) as u32;
            let x1 = (x0 + 1).min(src.width - 1);
            let wx = (fx - x0 as f64).clamp(0.0, 1.0);
            let dx = rx + x;
            if dx < 0 || dx >= dst_w as i32 {
                continue;
            }
            let p00 = ((y0 * src.width + x0) * 4) as usize;
            let p10 = ((y0 * src.width + x1) * 4) as usize;
            let p01 = ((y1 * src.width + x0) * 4) as usize;
            let p11 = ((y1 * src.width + x1) * 4) as usize;
            for c in 0..4 {
                let top = src.bgra[p00 + c] as f64 * (1.0 - wx) + src.bgra[p10 + c] as f64 * wx;
                let bottom = src.bgra[p01 + c] as f64 * (1.0 - wx) + src.bgra[p11 + c] as f64 * wx;
                let value = top * (1.0 - wy) + bottom * wy;
                dst[((dy as u32 * dst_w + dx as u32) * 4) as usize + c] = value.round() as u8;
            }
        }
    }
}

struct PageImage {
    width: u32,
    height: u32,
    bgra: Vec<u8>,
}

struct PdfSource {
    document: PdfDocument,
}

impl PdfSource {
    fn open(data: &[u8]) -> Result<Self> {
        unsafe {
            // RPC_E_CHANGED_MODE is fine: the thread already runs in COM.
            let _ = RoInitialize(RO_INIT_MULTITHREADED);
        }
        let stream = InMemoryRandomAccessStream::new().map_err(winrt)?;
        let writer = DataWriter::new().map_err(winrt)?;
        writer.WriteBytes(data).map_err(winrt)?;
        let buffer = writer.DetachBuffer().map_err(winrt)?;
        stream
            .WriteAsync(&buffer)
            .map_err(winrt)?
            .join()
            .map_err(winrt)?;
        stream.Seek(0).map_err(winrt)?;
        let document = PdfDocument::LoadFromStreamAsync(&stream)
            .map_err(winrt)?
            .join()
            .map_err(winrt)?;
        Ok(Self { document })
    }

    fn page_count(&self) -> Result<u32> {
        self.document.PageCount().map_err(winrt)
    }

    fn page_is_landscape(&self, index: u32) -> Result<bool> {
        let page = self.document.GetPage(index).map_err(winrt)?;
        let size = page.Size().map_err(winrt)?;
        Ok(size.Width > size.Height)
    }

    fn render(&self, index: u32, dpi: u32) -> Result<PageImage> {
        let page = self.document.GetPage(index).map_err(winrt)?;
        let size = page.Size().map_err(winrt)?;
        let width = ((size.Width.max(1.0) as f64) * dpi as f64 / 96.0).round() as u32;
        let height = ((size.Height.max(1.0) as f64) * dpi as f64 / 96.0).round() as u32;
        let options = PdfPageRenderOptions::new().map_err(winrt)?;
        options.SetDestinationWidth(width.max(1)).map_err(winrt)?;
        options.SetDestinationHeight(height.max(1)).map_err(winrt)?;
        let stream = InMemoryRandomAccessStream::new().map_err(winrt)?;
        page.RenderWithOptionsToStreamAsync(&stream, &options)
            .map_err(winrt)?
            .join()
            .map_err(winrt)?;
        let len = stream.Size().map_err(winrt)? as usize;
        if len == 0 {
            return Err(AppError::Print("PDF page produced no image".into()));
        }
        let input = stream.GetInputStreamAt(0).map_err(winrt)?;
        let reader = DataReader::CreateDataReader(&input).map_err(winrt)?;
        let loaded = reader
            .LoadAsync(len as u32)
            .map_err(winrt)?
            .join()
            .map_err(winrt)? as usize;
        let mut png = vec![0u8; loaded];
        reader.ReadBytes(&mut png).map_err(winrt)?;
        decode_png(&png)
    }
}

fn decode_png(data: &[u8]) -> Result<PageImage> {
    let mut decoder = png::Decoder::new(std::io::Cursor::new(data));
    decoder.set_transformations(png::Transformations::normalize_to_color8());
    let mut reader = decoder
        .read_info()
        .map_err(|e| AppError::Print(format!("cannot read PDF page image: {e}")))?;
    let mut buffer = vec![
        0u8;
        reader.output_buffer_size().ok_or_else(|| AppError::Print(
            "PDF page image is too large".into()
        ))?
    ];
    let info = reader
        .next_frame(&mut buffer)
        .map_err(|e| AppError::Print(format!("cannot decode PDF page image: {e}")))?;
    let src = &buffer[..info.buffer_size()];
    let (width, height) = (info.width, info.height);
    let mut bgra = vec![255u8; width as usize * height as usize * 4];
    let pixels = width as usize * height as usize;
    match info.color_type {
        png::ColorType::Rgba => {
            for i in 0..pixels {
                bgra[i * 4] = src[i * 4 + 2];
                bgra[i * 4 + 1] = src[i * 4 + 1];
                bgra[i * 4 + 2] = src[i * 4];
                bgra[i * 4 + 3] = src[i * 4 + 3];
            }
        }
        png::ColorType::Rgb => {
            for i in 0..pixels {
                bgra[i * 4] = src[i * 3 + 2];
                bgra[i * 4 + 1] = src[i * 3 + 1];
                bgra[i * 4 + 2] = src[i * 3];
            }
        }
        png::ColorType::Grayscale => {
            for i in 0..pixels {
                let v = src[i];
                bgra[i * 4] = v;
                bgra[i * 4 + 1] = v;
                bgra[i * 4 + 2] = v;
            }
        }
        png::ColorType::GrayscaleAlpha => {
            for i in 0..pixels {
                let v = src[i * 2];
                bgra[i * 4] = v;
                bgra[i * 4 + 1] = v;
                bgra[i * 4 + 2] = v;
                bgra[i * 4 + 3] = src[i * 2 + 1];
            }
        }
        other => {
            return Err(AppError::Print(format!(
                "unsupported PDF page image format: {other:?}"
            )))
        }
    }
    Ok(PageImage {
        width,
        height,
        bgra,
    })
}

fn winrt(err: windows::core::Error) -> AppError {
    AppError::Print(format!("Windows PDF engine: {err}"))
}

struct DeviceContext(HDC);

impl DeviceContext {
    fn create(printer: &str, devmode: Option<&DevMode>) -> Result<Self> {
        let driver = wide("WINSPOOL");
        let name = wide(printer);
        let dm = devmode.map(|d| d.dm() as *const _).unwrap_or_else(null);
        let dc = unsafe { CreateDCW(driver.as_ptr(), name.as_ptr(), null(), dm) };
        if dc.is_null() {
            return Err(last_error(format!("CreateDC failed for {printer}")));
        }
        Ok(Self(dc))
    }

    fn printable(&self) -> (i32, i32) {
        unsafe {
            (
                GetDeviceCaps(self.0, HORZRES),
                GetDeviceCaps(self.0, VERTRES),
            )
        }
    }

    fn dpi(&self) -> (i32, i32) {
        unsafe {
            (
                GetDeviceCaps(self.0, LOGPIXELSX).max(96),
                GetDeviceCaps(self.0, LOGPIXELSY).max(96),
            )
        }
    }
}

impl Drop for DeviceContext {
    fn drop(&mut self) {
        unsafe {
            DeleteDC(self.0);
        }
    }
}

struct DevMode {
    words: Vec<u32>,
    size: usize,
    port: String,
}

impl DevMode {
    fn from_printer(printer: &str) -> Option<Self> {
        unsafe {
            let name = wide(printer);
            let mut handle = null_mut();
            if OpenPrinterW(name.as_ptr() as *mut _, &mut handle, null_mut()) == 0 {
                return None;
            }
            let mut needed = 0u32;
            GetPrinterW(handle, 2, null_mut(), 0, &mut needed);
            let mut buffer = vec![0u32; needed as usize / 4 + 8];
            let mut got = 0u32;
            let ok = GetPrinterW(handle, 2, buffer.as_mut_ptr() as *mut _, needed, &mut got);
            let result = if ok == 0 {
                None
            } else {
                let info = &*(buffer.as_ptr() as *const PRINTER_INFO_2W);
                let port = if info.pPortName.is_null() {
                    String::new()
                } else {
                    cstr(info.pPortName)
                };
                if info.pDevMode.is_null() {
                    None
                } else {
                    let dm = info.pDevMode as *const winapi::um::wingdi::DEVMODEW;
                    let total = (*dm).dmSize as usize + (*dm).dmDriverExtra as usize;
                    if total == 0 {
                        None
                    } else {
                        let mut words = vec![0u32; total.div_ceil(4)];
                        std::ptr::copy_nonoverlapping(
                            dm as *const u8,
                            words.as_mut_ptr() as *mut u8,
                            total,
                        );
                        Some(Self {
                            words,
                            size: total,
                            port,
                        })
                    }
                }
            };
            ClosePrinter(handle);
            result
        }
    }

    fn dm(&self) -> &winapi::um::wingdi::DEVMODEW {
        unsafe { &*(self.words.as_ptr() as *const winapi::um::wingdi::DEVMODEW) }
    }

    fn dm_mut(&mut self) -> &mut winapi::um::wingdi::DEVMODEW {
        unsafe { &mut *(self.words.as_mut_ptr() as *mut winapi::um::wingdi::DEVMODEW) }
    }
}

impl Drop for DevMode {
    fn drop(&mut self) {
        let _ = self.size;
    }
}

fn apply_preferences(
    printer: &str,
    devmode: &mut DevMode,
    prefs: &PrinterPreferences,
    landscape: bool,
    booklet: bool,
) {
    let port = devmode.port.clone();
    let media_source = if prefs.media_source.is_empty() {
        None
    } else {
        bin_id(printer, &port, &prefs.media_source)
    };
    let dm = devmode.dm_mut();
    unsafe {
        if let Some(id) = paper_id(&prefs.paper) {
            dm.dmFields |= DM_PAPERSIZE;
            dm.u1.s1_mut().dmPaperSize = id;
        }
        dm.dmFields |= DM_ORIENTATION;
        dm.u1.s1_mut().dmOrientation = if landscape {
            DMORIENT_LANDSCAPE
        } else {
            DMORIENT_PORTRAIT
        } as i16;
        dm.dmFields |= DM_COLOR;
        dm.dmColor = if prefs.color {
            DMCOLOR_COLOR
        } else {
            DMCOLOR_MONOCHROME
        } as i16;
        if prefs.resolution > 0 {
            dm.dmFields |= DM_PRINTQUALITY;
            dm.u1.s1_mut().dmPrintQuality = prefs.resolution.min(1200) as i16;
            dm.dmYResolution = prefs.resolution.min(1200) as i16;
        }
        let copies = prefs.copies();
        dm.dmFields |= DM_COPIES | DM_COLLATE;
        dm.u1.s1_mut().dmCopies = copies.min(i16::MAX as u32) as i16;
        dm.dmCollate = if copies > 1 {
            DMCOLLATE_TRUE
        } else {
            DMCOLLATE_FALSE
        } as i16;
        let duplex = if booklet {
            DuplexMode::LongEdge
        } else {
            prefs.duplex.clone()
        };
        dm.dmFields |= DM_DUPLEX;
        dm.dmDuplex = match duplex {
            DuplexMode::Simplex => DMDUP_SIMPLEX,
            DuplexMode::LongEdge => DMDUP_VERTICAL,
            DuplexMode::ShortEdge => DMDUP_HORIZONTAL,
        } as i16;
        if let Some(id) = media_source {
            dm.dmFields |= DM_DEFAULTSOURCE;
            dm.u1.s1_mut().dmDefaultSource = id as i16;
        }
    }
}

fn bin_id(printer: &str, port: &str, name: &str) -> Option<u16> {
    let printer = wide(printer);
    let port = wide(port);
    unsafe {
        let count = winapi::um::wingdi::DeviceCapabilitiesW(
            printer.as_ptr(),
            port.as_ptr(),
            winapi::um::wingdi::DC_BINS as WORD,
            null_mut(),
            null(),
        );
        if count <= 0 {
            return None;
        }
        let mut ids = vec![0u16; count as usize];
        winapi::um::wingdi::DeviceCapabilitiesW(
            printer.as_ptr(),
            port.as_ptr(),
            winapi::um::wingdi::DC_BINS as WORD,
            ids.as_mut_ptr(),
            null(),
        );
        let mut names = vec![0u16; count as usize * 64];
        winapi::um::wingdi::DeviceCapabilitiesW(
            printer.as_ptr(),
            port.as_ptr(),
            winapi::um::wingdi::DC_BINNAMES as WORD,
            names.as_mut_ptr(),
            null(),
        );
        for (index, id) in ids.iter().enumerate() {
            let start = index * 64;
            let label = cstr(&names[start]);
            if label.eq_ignore_ascii_case(name) {
                return Some(*id);
            }
        }
    }
    None
}

fn paper_id(name: &str) -> Option<i16> {
    Some(match name.trim().to_ascii_uppercase().as_str() {
        "LETTER" => 1,
        "TABLOID" => 3,
        "LEDGER" => 4,
        "LEGAL" => 5,
        "EXECUTIVE" => 7,
        "A3" => 8,
        "A4" => 9,
        "A4SMALL" => 10,
        "A5" => 11,
        "B4" => 12,
        "B5" => 13,
        "FOLIO" => 14,
        "A6" => 70,
        _ => return None,
    })
}

#[allow(dead_code)]
fn paper_size_of(printer: &str, port: &str) -> Vec<POINT> {
    let printer = wide(printer);
    let port = wide(port);
    unsafe {
        let count = winapi::um::wingdi::DeviceCapabilitiesW(
            printer.as_ptr(),
            port.as_ptr(),
            winapi::um::wingdi::DC_PAPERSIZE as WORD,
            null_mut(),
            null(),
        );
        if count <= 0 {
            return Vec::new();
        }
        let mut sizes = vec![POINT { x: 0, y: 0 }; count as usize];
        winapi::um::wingdi::DeviceCapabilitiesW(
            printer.as_ptr(),
            port.as_ptr(),
            winapi::um::wingdi::DC_PAPERSIZE as WORD,
            sizes.as_mut_ptr() as *mut _,
            null(),
        );
        sizes
    }
}

fn wide(value: &str) -> Vec<u16> {
    value.encode_utf16().chain(std::iter::once(0)).collect()
}

fn cstr(ptr: *const u16) -> String {
    if ptr.is_null() {
        return String::new();
    }
    let mut len = 0;
    unsafe {
        while *ptr.add(len) != 0 {
            len += 1;
        }
        String::from_utf16_lossy(std::slice::from_raw_parts(ptr, len))
    }
}

fn last_error(context: String) -> AppError {
    AppError::Print(format!("{context}: {}", std::io::Error::last_os_error()))
}

/// Everything the spooler driver reports about one printer.
#[derive(Default)]
pub struct DriverCaps {
    pub papers: Vec<String>,
    pub sources: Vec<String>,
    pub resolutions: Vec<u32>,
    pub duplex: Option<bool>,
    pub color: Option<bool>,
    pub max_copies: u32,
    pub preferences: PrinterPreferences,
}

pub fn driver_caps(printer: &str, port: &str) -> DriverCaps {
    let printer_w = wide(printer);
    let port_w = wide(port);
    let mut caps = DriverCaps {
        preferences: PrinterPreferences::default(),
        ..Default::default()
    };
    unsafe {
        let count = DeviceCapabilitiesW(
            printer_w.as_ptr(),
            port_w.as_ptr(),
            DC_PAPERS,
            null_mut(),
            null(),
        );
        if count > 0 {
            let mut ids = vec![0u16; count as usize];
            DeviceCapabilitiesW(
                printer_w.as_ptr(),
                port_w.as_ptr(),
                DC_PAPERS,
                ids.as_mut_ptr(),
                null(),
            );
            let mut sizes = vec![POINT { x: 0, y: 0 }; count as usize];
            let sized = DeviceCapabilitiesW(
                printer_w.as_ptr(),
                port_w.as_ptr(),
                DC_PAPERSIZE,
                sizes.as_mut_ptr() as *mut _,
                null(),
            );
            for (index, id) in ids.iter().enumerate() {
                let by_size = (sized > index as i32)
                    .then(|| paper_name_from_size(sizes[index].x, sizes[index].y))
                    .flatten();
                if let Some(name) = by_size.or_else(|| paper_name_from_id(*id as i16)) {
                    if !caps.papers.iter().any(|p| p == name) {
                        caps.papers.push(name.to_string());
                    }
                }
            }
        }

        let bins = DeviceCapabilitiesW(
            printer_w.as_ptr(),
            port_w.as_ptr(),
            DC_BINS,
            null_mut(),
            null(),
        );
        if bins > 0 {
            let mut ids = vec![0u16; bins as usize];
            DeviceCapabilitiesW(
                printer_w.as_ptr(),
                port_w.as_ptr(),
                DC_BINS,
                ids.as_mut_ptr(),
                null(),
            );
            let mut names = vec![0u16; bins as usize * 64];
            DeviceCapabilitiesW(
                printer_w.as_ptr(),
                port_w.as_ptr(),
                DC_BINNAMES,
                names.as_mut_ptr(),
                null(),
            );
            for index in 0..bins as usize {
                let label = cstr(&names[index * 64]);
                if !label.is_empty() && !caps.sources.iter().any(|s| s.eq_ignore_ascii_case(&label))
                {
                    caps.sources.push(label);
                }
            }
        }

        let resolutions = DeviceCapabilitiesW(
            printer_w.as_ptr(),
            port_w.as_ptr(),
            DC_ENUMRESOLUTIONS,
            null_mut(),
            null(),
        );
        if resolutions > 0 {
            let mut values = vec![0i32; resolutions as usize * 2];
            DeviceCapabilitiesW(
                printer_w.as_ptr(),
                port_w.as_ptr(),
                DC_ENUMRESOLUTIONS,
                values.as_mut_ptr() as *mut _,
                null(),
            );
            for pair in values.chunks(2) {
                let dpi = pair[0];
                if dpi > 0 && !caps.resolutions.contains(&(dpi as u32)) {
                    caps.resolutions.push(dpi as u32);
                }
            }
            caps.resolutions.sort_unstable();
        }

        let duplex = DeviceCapabilitiesW(
            printer_w.as_ptr(),
            port_w.as_ptr(),
            DC_DUPLEX,
            null_mut(),
            null(),
        );
        if duplex >= 0 {
            caps.duplex = Some(duplex > 0);
        }
        let color = DeviceCapabilitiesW(
            printer_w.as_ptr(),
            port_w.as_ptr(),
            DC_COLORDEVICE,
            null_mut(),
            null(),
        );
        if color >= 0 {
            caps.color = Some(color > 0);
        }
        let copies = DeviceCapabilitiesW(
            printer_w.as_ptr(),
            port_w.as_ptr(),
            DC_COPIES,
            null_mut(),
            null(),
        );
        if copies > 1 {
            caps.max_copies = copies as u32;
        }
    }

    if let Some(devmode) = DevMode::from_printer(printer) {
        let dm = devmode.dm();
        let (paper_size, quality, copies_setting, default_source) = unsafe {
            (
                dm.u1.s1().dmPaperSize,
                dm.u1.s1().dmPrintQuality,
                dm.u1.s1().dmCopies,
                dm.u1.s1().dmDefaultSource,
            )
        };
        if dm.dmFields & DM_PAPERSIZE as DWORD != 0 {
            if let Some(name) = paper_name_from_id(paper_size) {
                caps.preferences.paper = name.to_string();
            }
        }
        if dm.dmFields & DM_COLOR as DWORD != 0 && dm.dmColor == DMCOLOR_COLOR as i16 {
            caps.preferences.color = true;
        }
        if dm.dmFields & DM_DUPLEX as DWORD != 0 {
            caps.duplex = Some(true);
            caps.preferences.duplex = match dm.dmDuplex as u32 {
                DMDUP_VERTICAL => DuplexMode::LongEdge,
                DMDUP_HORIZONTAL => DuplexMode::ShortEdge,
                _ => DuplexMode::Simplex,
            };
        }
        if dm.dmFields & DM_PRINTQUALITY as DWORD != 0 && quality > 0 {
            caps.preferences.resolution = quality as u32;
        }
        if dm.dmFields & DM_COPIES as DWORD != 0 && copies_setting > 0 {
            caps.preferences.copies = copies_setting as u32;
        }
        if dm.dmFields & DM_DEFAULTSOURCE as DWORD != 0 && !caps.sources.is_empty() {
            if let Some(source) = bin_name(printer, &devmode.port, default_source as u16) {
                caps.preferences.media_source = source;
            }
        }
    }
    if !caps.papers.is_empty() && !caps.papers.contains(&caps.preferences.paper) {
        caps.preferences.paper = caps.papers[0].clone();
    }
    caps
}

fn bin_name(printer: &str, port: &str, id: u16) -> Option<String> {
    let printer = wide(printer);
    let port = wide(port);
    unsafe {
        let count =
            DeviceCapabilitiesW(printer.as_ptr(), port.as_ptr(), DC_BINS, null_mut(), null());
        if count <= 0 {
            return None;
        }
        let mut ids = vec![0u16; count as usize];
        DeviceCapabilitiesW(
            printer.as_ptr(),
            port.as_ptr(),
            DC_BINS,
            ids.as_mut_ptr(),
            null(),
        );
        let mut names = vec![0u16; count as usize * 64];
        DeviceCapabilitiesW(
            printer.as_ptr(),
            port.as_ptr(),
            DC_BINNAMES,
            names.as_mut_ptr(),
            null(),
        );
        ids.iter()
            .position(|value| *value == id)
            .map(|index| cstr(&names[index * 64]))
            .filter(|name| !name.is_empty())
    }
}

fn paper_name_from_id(id: i16) -> Option<&'static str> {
    Some(match id {
        1 => "Letter",
        3 => "Tabloid",
        4 => "Ledger",
        5 => "Legal",
        7 => "Executive",
        8 => "A3",
        9 => "A4",
        10 => "A4",
        11 => "A5",
        12 => "B4",
        13 => "B5",
        14 => "Folio",
        70 => "A6",
        _ => return None,
    })
}

/// Map a paper size in tenths of a millimetre to a stable name.
fn paper_name_from_size(width: i32, height: i32) -> Option<&'static str> {
    if width <= 0 || height <= 0 {
        return None;
    }
    let (short, long) = if width <= height {
        (width, height)
    } else {
        (height, width)
    };
    let near = |a: i32, b: i32| (short - a).abs() <= 20 && (long - b).abs() <= 20;
    if near(2100, 2970) {
        Some("A4")
    } else if near(2970, 4200) {
        Some("A3")
    } else if near(1480, 2100) {
        Some("A5")
    } else if near(1050, 1480) {
        Some("A6")
    } else if near(2159, 2794) {
        Some("Letter")
    } else if near(2159, 3556) {
        Some("Legal")
    } else if near(1760, 2500) {
        Some("B5")
    } else if near(2570, 3640) || near(2500, 3530) {
        Some("B4")
    } else if near(2794, 4318) {
        Some("Tabloid")
    } else if near(1842, 2667) {
        Some("Executive")
    } else {
        None
    }
}
