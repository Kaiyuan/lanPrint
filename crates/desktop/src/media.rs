//! Paper and media metadata shared by the IPP endpoint and the Windows queue
//! capability injection. Keeping one table means the sizes offered by the
//! virtual queue always match the sizes lanPrint advertises over IPP.

/// A standard paper size that both IPP Everywhere and the Windows print schema
/// know about.
pub struct MediaSize {
    /// Friendly name as reported by host printer drivers, e.g. `A4`.
    pub name: &'static str,
    /// Windows Print Schema `PageMediaSizeName`, e.g. `ISOA4`.
    pub keyword: &'static str,
    /// Standard Unidrv GPD paper option name, e.g. `A4` or `LETTER`.
    pub gpd: &'static str,
    /// IPP/PWG self-describing media name, e.g. `iso_a4_210x297mm`.
    pub pwg: &'static str,
    pub width_mm: f64,
    pub height_mm: f64,
    /// Alternative spellings accepted from host drivers.
    keys: &'static [&'static str],
}

/// Sizes that are offered to the client when the host reports them.
pub const MEDIA: &[MediaSize] = &[
    MediaSize {
        name: "A0",
        keyword: "ISOA0",
        gpd: "A0",
        pwg: "iso_a0_841x1189mm",
        width_mm: 841.0,
        height_mm: 1189.0,
        keys: &["a0"],
    },
    MediaSize {
        name: "A1",
        keyword: "ISOA1",
        gpd: "A1",
        pwg: "iso_a1_594x841mm",
        width_mm: 594.0,
        height_mm: 841.0,
        keys: &["a1"],
    },
    MediaSize {
        name: "A2",
        keyword: "ISOA2",
        gpd: "A2",
        pwg: "iso_a2_420x594mm",
        width_mm: 420.0,
        height_mm: 594.0,
        keys: &["a2"],
    },
    MediaSize {
        name: "A3",
        keyword: "ISOA3",
        gpd: "A3",
        pwg: "iso_a3_297x420mm",
        width_mm: 297.0,
        height_mm: 420.0,
        keys: &["a3"],
    },
    MediaSize {
        name: "A4",
        keyword: "ISOA4",
        gpd: "A4",
        pwg: "iso_a4_210x297mm",
        width_mm: 210.0,
        height_mm: 297.0,
        keys: &["a4"],
    },
    MediaSize {
        name: "A5",
        keyword: "ISOA5",
        gpd: "A5",
        pwg: "iso_a5_148x210mm",
        width_mm: 148.0,
        height_mm: 210.0,
        keys: &["a5"],
    },
    MediaSize {
        name: "A6",
        keyword: "ISOA6",
        gpd: "A6",
        pwg: "iso_a6_105x148mm",
        width_mm: 105.0,
        height_mm: 148.0,
        keys: &["a6"],
    },
    MediaSize {
        name: "B4",
        keyword: "JISB4",
        gpd: "B4",
        pwg: "jis_b4_257x364mm",
        width_mm: 257.0,
        height_mm: 364.0,
        keys: &["b4", "jisb4", "b4jis"],
    },
    MediaSize {
        name: "B5",
        keyword: "JISB5",
        gpd: "B5",
        pwg: "jis_b5_182x257mm",
        width_mm: 182.0,
        height_mm: 257.0,
        keys: &["b5", "jisb5", "b5jis"],
    },
    MediaSize {
        name: "B6",
        keyword: "ISOB6",
        gpd: "B6",
        pwg: "iso_b6_125x176mm",
        width_mm: 125.0,
        height_mm: 176.0,
        keys: &["b6"],
    },
    MediaSize {
        name: "Letter",
        keyword: "NorthAmericaLetter",
        gpd: "LETTER",
        pwg: "na_letter_8.5x11in",
        width_mm: 215.9,
        height_mm: 279.4,
        keys: &["letter", "usletter", "8511", "85x11"],
    },
    MediaSize {
        name: "Legal",
        keyword: "NorthAmericaLegal",
        gpd: "LEGAL",
        pwg: "na_legal_8.5x14in",
        width_mm: 215.9,
        height_mm: 355.6,
        keys: &["legal", "uslegal", "8514", "85x14"],
    },
    MediaSize {
        name: "Tabloid",
        keyword: "NorthAmericaTabloid",
        gpd: "TABLOID",
        pwg: "na_ledger_11x17in",
        width_mm: 279.4,
        height_mm: 431.8,
        keys: &["tabloid", "11x17"],
    },
    MediaSize {
        name: "Ledger",
        keyword: "NorthAmericaLedger",
        gpd: "LEDGER",
        pwg: "na_ledger_11x17in",
        width_mm: 431.8,
        height_mm: 279.4,
        keys: &["ledger", "17x11"],
    },
    MediaSize {
        name: "Executive",
        keyword: "NorthAmericaExecutive",
        gpd: "EXECUTIVE",
        pwg: "na_executive_7.25x10.5in",
        width_mm: 184.15,
        height_mm: 266.7,
        keys: &["executive", "725105", "725x105"],
    },
    MediaSize {
        name: "Statement",
        keyword: "NorthAmericaStatement",
        gpd: "STATEMENT",
        pwg: "na_statement_5.5x8.5in",
        width_mm: 139.7,
        height_mm: 215.9,
        keys: &["statement", "5585", "55x85"],
    },
    MediaSize {
        name: "Folio",
        keyword: "OtherMetricFolio",
        gpd: "FOLIO",
        pwg: "na_folio_8.5x13in",
        width_mm: 215.9,
        height_mm: 330.2,
        keys: &["folio", "8513", "85x13"],
    },
];

fn key(value: &str) -> String {
    // Host drivers spell inches as fractions ("8 1/2x11"), decimal inches
    // ("8.5x11") or millimetres ("210x297"). Normalise all of them to a
    // alphanumeric key so one table entry can match every spelling.
    let value = value
        .replace("3/4", ".75")
        .replace("1/4", ".25")
        .replace("1/2", ".5")
        .replace("5/8", ".625")
        .replace("3/8", ".375")
        .replace("7/8", ".875")
        .replace("1/8", ".125");
    value
        .chars()
        .filter(|c| c.is_ascii_alphanumeric())
        .collect::<String>()
        .to_ascii_lowercase()
}

/// Look up a paper name reported by a host printer driver.
pub fn lookup(paper: &str) -> Option<&'static MediaSize> {
    if let Some(size) = lookup_exact(paper) {
        return Some(size);
    }
    // Drivers often append dimensions or a qualifier: "A4 (210 x 297 mm)".
    let head = paper.split('(').next().unwrap_or(paper).trim();
    if head.is_empty() || head == paper.trim() {
        return None;
    }
    lookup_exact(head)
}

fn lookup_exact(paper: &str) -> Option<&'static MediaSize> {
    let needle = key(paper);
    if needle.is_empty() {
        return None;
    }
    MEDIA
        .iter()
        .find(|media| media.keys.contains(&needle.as_str()) || key(media.name) == needle)
}

/// IPP/PWG name for a paper. Unknown names are passed through in the same
/// normalised form the endpoint has always used.
pub fn pwg_media(paper: &str) -> String {
    if let Some(media) = lookup(paper) {
        return media.pwg.into();
    }
    paper
        .trim()
        .to_ascii_lowercase()
        .replace([' ', '_'], "-")
        .trim_matches('-')
        .replace('-', "_")
}

/// Paper dimensions in hundredths of a millimetre.
pub fn dimensions(paper: &str) -> Option<(i32, i32)> {
    lookup(paper).map(|media| {
        (
            (media.width_mm * 100.0).round() as i32,
            (media.height_mm * 100.0).round() as i32,
        )
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn recognises_driver_spellings() {
        assert_eq!(lookup("A4").unwrap().keyword, "ISOA4");
        assert_eq!(lookup("a3").unwrap().keyword, "ISOA3");
        assert_eq!(lookup("US Letter").unwrap().keyword, "NorthAmericaLetter");
        assert_eq!(lookup("8.5x14").unwrap().keyword, "NorthAmericaLegal");
        assert_eq!(lookup("JIS B5").unwrap().keyword, "JISB5");
        assert_eq!(lookup("A4 (210 x 297 mm)").unwrap().keyword, "ISOA4");
        assert_eq!(lookup("B4 (JIS)").unwrap().keyword, "JISB4");
        assert_eq!(lookup("8 1/2x11").unwrap().keyword, "NorthAmericaLetter");
        assert_eq!(lookup("Automatic").map(|m| m.name), None);
    }

    #[test]
    fn keeps_unknown_names_usable() {
        assert_eq!(pwg_media("A4"), "iso_a4_210x297mm");
        assert_eq!(pwg_media("Letter"), "na_letter_8.5x11in");
        assert_eq!(pwg_media("Custom 100x150"), "custom_100x150");
        assert_eq!(dimensions("A4"), Some((21000, 29700)));
        assert_eq!(dimensions("Letter"), Some((21590, 27940)));
    }
}

