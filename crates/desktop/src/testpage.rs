//! A colourful A4 test page rendered as a self-contained PDF.
//!
//! "Test print" uses this page instead of the driver test page so the user can
//! verify colour, alignment and the PDF print pipeline in one sheet. It is
//! written by hand to keep the desktop binary free of an extra PDF dependency.

use lanprint_core::VERSION;

const PAGE_W: f32 = 595.28;
const PAGE_H: f32 = 841.89;
const MARGIN: f32 = 40.0;

type Rgb = (f32, f32, f32);

const INDIGO: Rgb = (0.36, 0.20, 0.78);
const CYAN: Rgb = (0.02, 0.70, 0.83);
const INK: Rgb = (0.12, 0.15, 0.22);
const MUTED: Rgb = (0.36, 0.39, 0.46);
const PANEL: Rgb = (0.96, 0.95, 1.0);

/// Build the test page PDF for a host whose device name is `host`.
pub fn pdf(host: &str) -> Vec<u8> {
    let mut page = String::new();
    draw_header(&mut page);
    draw_intro(&mut page);
    draw_swatches(&mut page);
    draw_features(&mut page);
    draw_footer(&mut page, host);
    wrap_pdf(&page)
}

fn draw_header(page: &mut String) {
    // A smooth indigo-to-cyan band built from thin strips; no shading
    // dictionaries needed, and it exercises the colour path.
    let strips = 48;
    for i in 0..strips {
        let t = i as f32 / (strips - 1) as f32;
        let color = if t < 0.5 {
            mix(INDIGO, (0.45, 0.25, 0.86), t * 2.0)
        } else {
            mix((0.45, 0.25, 0.86), CYAN, (t - 0.5) * 2.0)
        };
        let width = PAGE_W / strips as f32 + 0.6;
        fill(
            page,
            i as f32 * PAGE_W / strips as f32,
            690.0,
            width,
            PAGE_H - 690.0,
            color,
        );
    }

    text(page, "F1", 42.0, MARGIN, 782.0, (1.0, 1.0, 1.0), "lanPrint");
    text(
        page,
        "F2",
        14.0,
        MARGIN,
        756.0,
        (0.94, 0.96, 1.0),
        "Share any printer. Print anywhere.",
    );
    text_right(
        page,
        "F2",
        8.5,
        PAGE_W - MARGIN,
        796.0,
        (0.90, 0.95, 1.0),
        "COLOR TEST PAGE",
    );

    text(
        page,
        "F1",
        13.0,
        MARGIN,
        650.0,
        INDIGO,
        "The shared printer is connected and working.",
    );
}

fn draw_intro(page: &mut String) {
    let lines = [
        "Every installation of lanPrint is both a host and a client. Pick a printer on the",
        "host, connect from another device, and it appears in every application's print",
        "dialog. The client sends each job as PDF; the host prints it with the real driver.",
    ];
    for (i, line) in lines.iter().enumerate() {
        text(
            page,
            "F2",
            10.5,
            MARGIN,
            624.0 - i as f32 * 16.0,
            MUTED,
            line,
        );
    }
}

fn draw_swatches(page: &mut String) {
    text(page, "F2", 8.5, MARGIN, 566.0, MUTED, "COLOR CHECK");

    let base: [Rgb; 8] = [
        (0.95, 0.10, 0.13),
        (0.98, 0.45, 0.05),
        (0.98, 0.82, 0.08),
        (0.05, 0.62, 0.25),
        (0.02, 0.62, 0.60),
        (0.05, 0.72, 0.90),
        (0.10, 0.30, 0.85),
        (0.85, 0.12, 0.55),
    ];
    let gap = 7.0;
    let count = base.len() as f32;
    let width = (PAGE_W - 2.0 * MARGIN - gap * (count - 1.0)) / count;
    let rows = [534.0, 492.0, 450.0];
    for (row, y) in rows.iter().enumerate() {
        for (i, color) in base.iter().enumerate() {
            let shade = match row {
                0 => *color,
                1 => mix(*color, (1.0, 1.0, 1.0), 0.55),
                _ => mix(*color, (0.0, 0.0, 0.0), 0.45),
            };
            let x = MARGIN + i as f32 * (width + gap);
            fill(page, x, *y, width, 30.0, shade);
        }
    }
    text(
        page,
        "F2",
        8.5,
        MARGIN,
        432.0,
        MUTED,
        "Primary colours, 55% tints and 45% shades. Every bar should be distinct.",
    );
}

fn draw_features(page: &mut String) {
    fill(page, MARGIN, 258.0, PAGE_W - 2.0 * MARGIN, 160.0, PANEL);
    text(
        page,
        "F1",
        10.5,
        MARGIN + 16.0,
        392.0,
        INDIGO,
        "WHAT IS LANPRINT?",
    );

    let items = [
        (
            (0.95, 0.10, 0.13),
            "Host and client in one small native app - no Samba, IPP or WSD.",
        ),
        (
            (0.98, 0.55, 0.05),
            "PDF jobs travel to the host and print through the real driver.",
        ),
        (
            (0.10, 0.55, 0.85),
            "Duplex, booklet, paper trays, colour and copies in one dialog.",
        ),
        (
            (0.10, 0.62, 0.35),
            "Scanning, cloud bridge and nine UI languages, MIT licensed.",
        ),
    ];
    for (i, (color, line)) in items.iter().enumerate() {
        let y = 368.0 - i as f32 * 25.0;
        fill(page, MARGIN + 16.0, y - 1.0, 6.0, 6.0, *color);
        text(page, "F2", 10.5, MARGIN + 30.0, y, INK, line);
    }
}

fn draw_footer(page: &mut String, host: &str) {
    fill(
        page,
        MARGIN,
        232.0,
        PAGE_W - 2.0 * MARGIN,
        1.0,
        (0.85, 0.86, 0.90),
    );

    text(
        page,
        "F1",
        12.0,
        MARGIN,
        204.0,
        INDIGO,
        "If this page prints in colour, the printer is ready.",
    );
    let host = clean(host);
    let host = if host.is_empty() {
        "this host"
    } else {
        host.as_str()
    };
    text(
        page,
        "F2",
        9.0,
        MARGIN,
        184.0,
        MUTED,
        &format!("Host: {host}"),
    );
    text(
        page,
        "F2",
        9.0,
        MARGIN,
        170.0,
        MUTED,
        &format!("Generated by lanPrint {VERSION}"),
    );

    // Bottom band.
    let strips = 48;
    for i in 0..strips {
        let t = i as f32 / (strips - 1) as f32;
        let color = if t < 0.5 {
            mix(INDIGO, (0.45, 0.25, 0.86), t * 2.0)
        } else {
            mix((0.45, 0.25, 0.86), CYAN, (t - 0.5) * 2.0)
        };
        let width = PAGE_W / strips as f32 + 0.6;
        fill(
            page,
            i as f32 * PAGE_W / strips as f32,
            0.0,
            width,
            48.0,
            color,
        );
    }
    text(
        page,
        "F2",
        9.0,
        MARGIN,
        26.0,
        (0.95, 0.97, 1.0),
        "https://github.com/Kaiyuan/lanPrint",
    );
    text_right(
        page,
        "F2",
        9.0,
        PAGE_W - MARGIN,
        26.0,
        (0.95, 0.97, 1.0),
        "MIT License",
    );
}

fn mix(a: Rgb, b: Rgb, t: f32) -> Rgb {
    (
        a.0 + (b.0 - a.0) * t,
        a.1 + (b.1 - a.1) * t,
        a.2 + (b.2 - a.2) * t,
    )
}

fn fill(page: &mut String, x: f32, y: f32, w: f32, h: f32, color: Rgb) {
    page.push_str(&format!(
        "{:.3} {:.3} {:.3} rg\n{:.2} {:.2} {:.2} {:.2} re f\n",
        color.0, color.1, color.2, x, y, w, h
    ));
}

fn text(page: &mut String, font: &str, size: f32, x: f32, y: f32, color: Rgb, value: &str) {
    page.push_str(&format!(
        "{:.3} {:.3} {:.3} rg\nBT /{font} {size} Tf 1 0 0 1 {x:.2} {y:.2} Tm ({}) Tj ET\n",
        color.0,
        color.1,
        color.2,
        escape(value)
    ));
}

fn text_right(
    page: &mut String,
    font: &str,
    size: f32,
    right: f32,
    y: f32,
    color: Rgb,
    value: &str,
) {
    // Helvetica averages roughly half the point size per character. The page
    // is only an approximation, so the estimate keeps labels close enough.
    let width = value.chars().count() as f32 * size * 0.52;
    text(page, font, size, right - width, y, color, value);
}

fn escape(value: &str) -> String {
    let mut out = String::with_capacity(value.len());
    for ch in value.chars() {
        match ch {
            '(' => out.push_str("\\("),
            ')' => out.push_str("\\)"),
            '\\' => out.push_str("\\\\"),
            c if c.is_ascii_graphic() || c == ' ' => out.push(c),
            _ => out.push('?'),
        }
    }
    out
}

fn clean(value: &str) -> String {
    let value = escape(value.trim());
    value.chars().take(48).collect()
}

fn add_object(out: &mut Vec<u8>, offsets: &mut Vec<usize>, body: &[u8]) {
    offsets.push(out.len());
    out.extend_from_slice(body);
}

fn wrap_pdf(stream: &str) -> Vec<u8> {
    let mut out = Vec::new();
    let mut offsets: Vec<usize> = Vec::new();
    out.extend_from_slice(b"%PDF-1.4\n%\xE2\xE3\xCF\xD3\n");

    add_object(
        &mut out,
        &mut offsets,
        b"1 0 obj\n<< /Type /Catalog /Pages 2 0 R >>\nendobj\n",
    );
    add_object(
        &mut out,
        &mut offsets,
        b"2 0 obj\n<< /Type /Pages /Kids [3 0 R] /Count 1 >>\nendobj\n",
    );
    let page = format!(
        "3 0 obj\n<< /Type /Page /Parent 2 0 R /MediaBox [0 0 {PAGE_W:.2} {PAGE_H:.2}] /Resources << /Font << /F1 5 0 R /F2 6 0 R >> >> /Contents 4 0 R >>\nendobj\n"
    );
    add_object(&mut out, &mut offsets, page.as_bytes());

    let bytes = stream.as_bytes();
    let header = format!("4 0 obj\n<< /Length {} >>\nstream\n", bytes.len());
    offsets.push(out.len());
    out.extend_from_slice(header.as_bytes());
    out.extend_from_slice(bytes);
    out.extend_from_slice(b"\nendstream\nendobj\n");

    add_object(
        &mut out,
        &mut offsets,
        b"5 0 obj\n<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica-Bold /Encoding /WinAnsiEncoding >>\nendobj\n",
    );
    add_object(
        &mut out,
        &mut offsets,
        b"6 0 obj\n<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica /Encoding /WinAnsiEncoding >>\nendobj\n",
    );

    let xref = out.len();
    out.extend_from_slice(b"xref\n0 7\n0000000000 65535 f \n");
    for offset in &offsets {
        out.extend_from_slice(format!("{offset:010} 00000 n \n").as_bytes());
    }
    out.extend_from_slice(
        format!("trailer\n<< /Size 7 /Root 1 0 R >>\nstartxref\n{xref}\n%%EOF\n").as_bytes(),
    );
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample() -> Vec<u8> {
        pdf("Design-PC")
    }

    #[test]
    fn builds_a_single_page_pdf() {
        let data = sample();
        let text = String::from_utf8_lossy(&data);
        assert!(data.starts_with(b"%PDF-1.4"));
        assert!(text.contains("/Count 1"));
        assert!(text.contains("/MediaBox [0 0 595.28 841.89]"));
        assert!(text.contains("(Design-PC)") || text.contains("Host: Design-PC"));
        assert!(text.ends_with("%%EOF\n"));
    }

    #[test]
    fn startxref_points_at_the_table() {
        let data = sample();
        let marker = b"startxref\n";
        let start = data
            .windows(marker.len())
            .rposition(|w| w == marker)
            .expect("startxref")
            + marker.len();
        let offset: usize = String::from_utf8_lossy(&data[start..])
            .split_whitespace()
            .next()
            .expect("offset")
            .parse()
            .expect("number");
        assert!(data[offset..].starts_with(b"xref\n0 7\n"));
    }

    #[test]
    fn escapes_pdf_strings() {
        assert_eq!(escape("a(b)c\\d"), "a\\(b\\)c\\\\d");
        assert_eq!(clean("测试"), "??");
    }
}
