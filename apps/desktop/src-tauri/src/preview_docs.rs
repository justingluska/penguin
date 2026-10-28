//! Attachment previews beyond pictures, PDFs and text (attachments.rs uses
//! these), all decided by the bytes, never the claimed type:
//!  - audio and video: recognized from their headers, played by the
//!    preview's own player on a blob: URL;
//!  - Word documents (.docx): read from their XML into HTML that Penguin
//!    writes itself (paragraphs, headings, bold/italic/underline/strike,
//!    lists, tables, line breaks). Every character of the document is
//!    escaped, so nothing in the file is ever markup, and the UI shows it in
//!    a sandboxed frame with no scripts;
//!  - spreadsheets (.xlsx, .xlsm, .xlsb, .xls, .ods through calamine; .csv
//!    and .tsv parsed here): the first rows and columns of each sheet as
//!    text cells.

use std::io::{Cursor, Read};

use calamine::{open_workbook_auto_from_rs, Data, Reader};
use quick_xml::events::Event;
use serde::Serialize;

/// The most of `word/document.xml` read (uncompressed), against zip bombs.
const MAX_DOC_XML: u64 = 32 * 1024 * 1024;
/// The most HTML a document preview carries; the rest is marked truncated.
const MAX_DOC_HTML: usize = 2 * 1024 * 1024;
pub const MAX_SHEETS: usize = 24;
pub const MAX_SHEET_ROWS: usize = 1000;
pub const MAX_SHEET_COLS: usize = 60;
const MAX_CELL_CHARS: usize = 2000;

/// Audio or video, by its first bytes: (MIME type for the blob, is video).
pub fn sniff_media(b: &[u8]) -> Option<(&'static str, bool)> {
    if b.len() >= 12 && &b[4..8] == b"ftyp" {
        // ISO base media (MP4, MOV, M4A, M4V, 3GP): the major brand says which.
        return Some(match &b[8..12] {
            b"qt  " => ("video/quicktime", true),
            b"M4A " | b"M4B " | b"M4P " => ("audio/mp4", false),
            _ => ("video/mp4", true),
        });
    }
    if b.starts_with(&[0x1A, 0x45, 0xDF, 0xA3]) {
        return Some(("video/webm", true));
    }
    if b.len() >= 12 && b.starts_with(b"RIFF") && &b[8..12] == b"WAVE" {
        return Some(("audio/wav", false));
    }
    if b.starts_with(b"fLaC") {
        return Some(("audio/flac", false));
    }
    if b.starts_with(b"ID3") {
        return Some(("audio/mpeg", false));
    }
    if b.len() >= 2 && b[0] == 0xFF {
        // ADTS AAC (layer bits 00) before MPEG audio frames (layers I–III).
        if b[1] & 0xF6 == 0xF0 {
            return Some(("audio/aac", false));
        }
        if b[1] & 0xE0 == 0xE0 && b[1] & 0x06 != 0 {
            return Some(("audio/mpeg", false));
        }
    }
    None
}

fn escape(s: &str, out: &mut String) {
    for c in s.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&#39;"),
            c => out.push(c),
        }
    }
}

/// A run's formatting (w:rPr).
#[derive(Default, Clone, Copy)]
struct RunStyle {
    bold: bool,
    italic: bool,
    underline: bool,
    strike: bool,
}

/// `w:b`, `w:i`… are on unless `w:val` says 0/false/none.
fn toggle_on(e: &quick_xml::events::BytesStart, dec: quick_xml::Decoder) -> bool {
    for a in e.attributes().flatten() {
        if a.key.local_name().as_ref() == b"val" {
            let v = a.decoded_and_normalized_value(quick_xml::XmlVersion::Implicit1_0, dec).unwrap_or_default();
            return !matches!(v.as_ref(), "0" | "false" | "off" | "none");
        }
    }
    true
}

fn attr_val(e: &quick_xml::events::BytesStart, dec: quick_xml::Decoder) -> Option<String> {
    e.attributes()
        .flatten()
        .find(|a| a.key.local_name().as_ref() == b"val")
        .and_then(|a| a.decoded_and_normalized_value(quick_xml::XmlVersion::Implicit1_0, dec).ok().map(|v| v.into_owned()))
}

/// The HTML tag a paragraph style gets: headings by their style id.
fn heading_tag(style: &str) -> Option<&'static str> {
    let s = style.to_ascii_lowercase().replace([' ', '-', '_'], "");
    match s.as_str() {
        "title" => Some("h1"),
        "subtitle" => Some("h2"),
        _ => s
            .strip_prefix("heading")
            .and_then(|n| n.parse::<u8>().ok())
            .map(|n| match n {
                1 => "h1",
                2 => "h2",
                3 => "h3",
                4 => "h4",
                5 => "h5",
                _ => "h6",
            }),
    }
}

/// A Word document's body as HTML Penguin wrote (see the module docs), and
/// whether it was cut short. None: not a readable .docx.
pub fn docx_html(bytes: &[u8]) -> Option<(String, bool)> {
    let mut zip = zip::ZipArchive::new(Cursor::new(bytes)).ok()?;
    let mut xml = Vec::new();
    zip.by_name("word/document.xml")
        .ok()?
        .take(MAX_DOC_XML)
        .read_to_end(&mut xml)
        .ok()?;
    Some(docx_xml_to_html(&xml))
}

/// The paragraph, run and list state while reading `word/document.xml`.
#[derive(Default)]
struct Doc {
    out: String,
    /// Paragraph being built: its style and list-ness come from w:pPr, before its runs.
    para: String,
    in_para: bool,
    para_style: Option<String>,
    para_list: bool,
    in_ppr: bool,
    /// Run being built.
    run: String,
    style: RunStyle,
    in_rpr: bool,
    in_text: bool,
    /// A list is open at each table depth (a list inside a cell closes with it).
    list_open: Vec<bool>,
}

impl Doc {
    fn close_list(&mut self) {
        if let Some(open) = self.list_open.last_mut() {
            if *open {
                self.out.push_str("</ul>");
                *open = false;
            }
        }
    }

    fn start(&mut self, e: &quick_xml::events::BytesStart, empty: bool, dec: quick_xml::Decoder) {
        match e.local_name().as_ref() {
            b"p" => {
                self.in_para = true;
                self.para.clear();
                self.para_style = None;
                self.para_list = false;
                if empty {
                    self.end_para();
                }
            }
            b"pPr" if !empty => self.in_ppr = true,
            b"pStyle" if self.in_ppr => self.para_style = attr_val(e, dec),
            b"numPr" if self.in_ppr => self.para_list = true,
            b"r" if !empty => {
                self.run.clear();
                self.style = RunStyle::default();
            }
            b"rPr" if !self.in_ppr && !empty => self.in_rpr = true,
            b"b" if self.in_rpr => self.style.bold = toggle_on(e, dec),
            b"i" if self.in_rpr => self.style.italic = toggle_on(e, dec),
            b"u" if self.in_rpr => self.style.underline = toggle_on(e, dec),
            b"strike" | b"dstrike" if self.in_rpr => self.style.strike = toggle_on(e, dec),
            b"t" if !empty => self.in_text = true,
            b"tab" if self.in_para && !self.in_ppr => self.run.push('\t'),
            b"br" | b"cr" if self.in_para => self.run.push_str("<br>"),
            b"tbl" if !empty => {
                self.close_list();
                self.out.push_str("<table>");
                self.list_open.push(false);
            }
            b"tr" if !empty => self.out.push_str("<tr>"),
            b"tc" if !empty => self.out.push_str("<td>"),
            _ => {}
        }
    }

    fn end(&mut self, name: &[u8]) {
        match name {
            b"t" => self.in_text = false,
            b"rPr" => self.in_rpr = false,
            b"pPr" => self.in_ppr = false,
            b"r" => self.end_run(),
            b"p" => self.end_para(),
            b"tc" => {
                self.close_list();
                self.out.push_str("</td>");
            }
            b"tr" => self.out.push_str("</tr>"),
            b"tbl" => {
                self.close_list();
                self.list_open.pop();
                if self.list_open.is_empty() {
                    self.list_open.push(false);
                }
                self.out.push_str("</table>");
            }
            _ => {}
        }
    }

    fn end_run(&mut self) {
        if self.run.is_empty() {
            return;
        }
        let mut s = std::mem::take(&mut self.run);
        let st = self.style;
        if st.bold {
            s = format!("<b>{s}</b>");
        }
        if st.italic {
            s = format!("<i>{s}</i>");
        }
        if st.underline {
            s = format!("<u>{s}</u>");
        }
        if st.strike {
            s = format!("<s>{s}</s>");
        }
        self.para.push_str(&s);
    }

    fn end_para(&mut self) {
        self.in_para = false;
        let para = std::mem::take(&mut self.para);
        let heading = self.para_style.as_deref().and_then(heading_tag);
        if self.para_list && heading.is_none() {
            if let Some(open) = self.list_open.last_mut() {
                if !*open {
                    self.out.push_str("<ul>");
                    *open = true;
                }
            }
            self.out.push_str("<li>");
            self.out.push_str(&para);
            self.out.push_str("</li>");
        } else {
            self.close_list();
            let tag = heading.unwrap_or("p");
            self.out.push_str(&format!("<{tag}>{para}</{tag}>"));
        }
    }

    fn text(&mut self, s: &str) {
        if self.in_text {
            escape(s, &mut self.run);
        }
    }
}

/// `word/document.xml` → HTML (split out for tests).
fn docx_xml_to_html(xml: &[u8]) -> (String, bool) {
    let mut reader = quick_xml::Reader::from_reader(xml);
    reader.config_mut().trim_text(false);
    let mut d = Doc {
        list_open: vec![false],
        ..Doc::default()
    };
    let mut truncated = false;
    loop {
        if d.out.len() > MAX_DOC_HTML {
            truncated = true;
            break;
        }
        match reader.read_event() {
            Ok(Event::Eof) | Err(_) => break,
            Ok(Event::Start(e)) => d.start(&e, false, reader.decoder()),
            Ok(Event::Empty(e)) => d.start(&e, true, reader.decoder()),
            Ok(Event::End(e)) => d.end(e.local_name().as_ref()),
            Ok(Event::Text(t)) => {
                if let Ok(s) = t.decode() {
                    d.text(&s);
                }
            }
            Ok(Event::GeneralRef(r)) => {
                if let Ok(Some(c)) = r.resolve_char_ref() {
                    d.text(&c.to_string());
                } else if let Ok(name) = r.decode() {
                    let c = match name.as_ref() {
                        "amp" => "&",
                        "lt" => "<",
                        "gt" => ">",
                        "quot" => "\"",
                        "apos" => "'",
                        _ => "",
                    };
                    d.text(c);
                }
            }
            Ok(_) => {}
        }
    }
    d.close_list();
    (d.out, truncated)
}

/// One sheet of a spreadsheet preview: its first rows (text cells, trailing
/// empty columns dropped) and how big it really is.
#[derive(Debug, Clone, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct SheetPreview {
    pub name: String,
    pub rows: Vec<Vec<String>>,
    pub total_rows: u32,
    pub total_cols: u32,
}

fn cell_text(d: &Data) -> String {
    let s = match d {
        Data::Empty => String::new(),
        Data::DateTime(dt) => match dt.as_datetime() {
            Some(t) if t.time() == chrono::NaiveTime::MIN => t.format("%Y-%m-%d").to_string(),
            Some(t) => t.format("%Y-%m-%d %H:%M").to_string(),
            None => d.to_string(),
        },
        Data::Error(e) => format!("#{e:?}"),
        _ => d.to_string(),
    };
    clip(s)
}

fn clip(mut s: String) -> String {
    if s.chars().count() > MAX_CELL_CHARS {
        s = s.chars().take(MAX_CELL_CHARS).collect::<String>() + "…";
    }
    s
}

/// Cap a sheet's rows and columns, dropping the empty columns at its right.
fn shape(name: String, all: Vec<Vec<String>>) -> SheetPreview {
    let total_rows = all.len() as u32;
    let width = all
        .iter()
        .map(|r| r.iter().rposition(|c| !c.is_empty()).map_or(0, |i| i + 1))
        .max()
        .unwrap_or(0);
    let rows = all
        .into_iter()
        .take(MAX_SHEET_ROWS)
        .map(|mut r| {
            r.truncate(width.min(MAX_SHEET_COLS));
            r.resize(width.min(MAX_SHEET_COLS), String::new());
            r
        })
        .collect();
    SheetPreview {
        name,
        rows,
        total_rows,
        total_cols: width as u32,
    }
}

/// An Excel or OpenDocument workbook's sheets. None: not a readable workbook.
pub fn workbook_sheets(bytes: &[u8]) -> Option<Vec<SheetPreview>> {
    let mut wb = open_workbook_auto_from_rs(Cursor::new(bytes)).ok()?;
    let names: Vec<String> = wb.sheet_names().into_iter().take(MAX_SHEETS).collect();
    let mut out = Vec::new();
    for name in names {
        let Ok(range) = wb.worksheet_range(&name) else { continue };
        let rows: Vec<Vec<String>> = range
            .rows()
            .take(MAX_SHEET_ROWS + 1)
            .map(|r| r.iter().take(MAX_SHEET_COLS + 1).map(cell_text).collect())
            .collect();
        let mut sheet = shape(name, rows);
        sheet.total_rows = range.height() as u32;
        // Cut at MAX_SHEET_COLS: say how wide it really is.
        if sheet.total_cols as usize > MAX_SHEET_COLS {
            sheet.total_cols = range.width() as u32;
        }
        out.push(sheet);
    }
    Some(out)
}

/// A CSV or TSV file as one sheet (quoted fields, doubled quotes, newlines in
/// quotes; a .csv using `;` is recognized from its first line).
pub fn delimited_sheet(name: &str, text: &str, tab: bool) -> SheetPreview {
    let text = text.strip_prefix('\u{feff}').unwrap_or(text);
    let delim = if tab {
        '\t'
    } else {
        let first = text.lines().next().unwrap_or("");
        let count = |d: char| first.chars().filter(|&c| c == d).count();
        if count(';') > count(',') { ';' } else { ',' }
    };
    let mut rows: Vec<Vec<String>> = Vec::new();
    let mut row: Vec<String> = Vec::new();
    let mut field = String::new();
    let mut quoted = false;
    let mut chars = text.chars().peekable();
    while let Some(c) = chars.next() {
        if quoted {
            if c == '"' {
                if chars.peek() == Some(&'"') {
                    field.push('"');
                    chars.next();
                } else {
                    quoted = false;
                }
            } else {
                field.push(c);
            }
            continue;
        }
        match c {
            '"' if field.is_empty() => quoted = true,
            c if c == delim => row.push(clip(std::mem::take(&mut field))),
            '\r' => {}
            '\n' => {
                row.push(clip(std::mem::take(&mut field)));
                rows.push(std::mem::take(&mut row));
                if rows.len() > MAX_SHEET_ROWS * 4 {
                    break;
                }
            }
            c => field.push(c),
        }
    }
    if !field.is_empty() || !row.is_empty() {
        row.push(clip(field));
        rows.push(row);
    }
    let total = rows.len() as u32;
    let mut sheet = shape(name.to_string(), rows);
    sheet.total_rows = total;
    sheet
}

/// File types these previewers handle, for `attachments::plan`.
pub fn is_docx(ext: &str, mime: &str) -> bool {
    ext == "docx" || mime == "application/vnd.openxmlformats-officedocument.wordprocessingml.document"
}

pub fn is_workbook(ext: &str, mime: &str) -> bool {
    matches!(ext, "xlsx" | "xlsm" | "xlsb" | "xls" | "ods")
        || matches!(
            mime,
            "application/vnd.openxmlformats-officedocument.spreadsheetml.sheet"
                | "application/vnd.ms-excel"
                | "application/vnd.ms-excel.sheet.macroenabled.12"
                | "application/vnd.ms-excel.sheet.binary.macroenabled.12"
                | "application/vnd.oasis.opendocument.spreadsheet"
        )
}

pub fn delimited(ext: &str, mime: &str) -> Option<bool> {
    match (ext, mime) {
        ("tsv", _) | (_, "text/tab-separated-values") => Some(true),
        ("csv", _) | (_, "text/csv") => Some(false),
        _ => None,
    }
}

pub fn is_media(ext: &str, mime: &str) -> bool {
    const EXTS: [&str; 12] = ["mp4", "m4v", "mov", "webm", "mp3", "m4a", "wav", "aac", "flac", "3gp", "m4b", "mpga"];
    EXTS.contains(&ext) || mime.starts_with("video/") || mime.starts_with("audio/")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn media_is_recognized_by_its_bytes() {
        let mp4 = [&[0, 0, 0, 0x18][..], b"ftypisom", &[0; 8]].concat();
        assert_eq!(sniff_media(&mp4), Some(("video/mp4", true)));
        let mov = [&[0, 0, 0, 0x14][..], b"ftypqt  ", &[0; 8]].concat();
        assert_eq!(sniff_media(&mov), Some(("video/quicktime", true)));
        let m4a = [&[0, 0, 0, 0x20][..], b"ftypM4A ", &[0; 8]].concat();
        assert_eq!(sniff_media(&m4a), Some(("audio/mp4", false)));
        assert_eq!(sniff_media(b"ID3\x04\0\0\0\0\0\0"), Some(("audio/mpeg", false)));
        assert_eq!(sniff_media(b"RIFF\0\0\0\0WAVEfmt "), Some(("audio/wav", false)));
        assert_eq!(sniff_media(&[0x1A, 0x45, 0xDF, 0xA3, 0]), Some(("video/webm", true)));
        assert_eq!(sniff_media(&[0xFF, 0xF1, 0x50]), Some(("audio/aac", false)));
        assert_eq!(sniff_media(&[0xFF, 0xFB, 0x90]), Some(("audio/mpeg", false)));
        assert_eq!(sniff_media(b"%PDF-1.4"), None);
        assert_eq!(sniff_media(b"<html>"), None);
    }

    const W: &str = r#"xmlns:w="http://schemas.openxmlformats.org/wordprocessingml/2006/main""#;

    fn doc(body: &str) -> String {
        format!(r#"<?xml version="1.0"?><w:document {W}><w:body>{body}</w:body></w:document>"#)
    }

    #[test]
    fn word_paragraphs_headings_and_emphasis() {
        let xml = doc(concat!(
            r#"<w:p><w:pPr><w:pStyle w:val="Heading1"/></w:pPr><w:r><w:t>Lease renewal</w:t></w:r></w:p>"#,
            r#"<w:p><w:r><w:t xml:space="preserve">Rent is </w:t></w:r><w:r><w:rPr><w:b/></w:rPr><w:t>$2,450</w:t></w:r>"#,
            r#"<w:r><w:rPr><w:i w:val="0"/></w:rPr><w:t> a month.</w:t></w:r></w:p>"#,
            r#"<w:p/>"#,
        ));
        let (html, cut) = docx_xml_to_html(xml.as_bytes());
        assert!(!cut);
        assert_eq!(html, "<h1>Lease renewal</h1><p>Rent is <b>$2,450</b> a month.</p><p></p>");
    }

    #[test]
    fn word_text_is_never_markup() {
        let xml = doc(r#"<w:p><w:r><w:t>&lt;script&gt;alert(1)&lt;/script&gt; &amp; "quotes" &#169;</w:t></w:r></w:p>"#);
        let (html, _) = docx_xml_to_html(xml.as_bytes());
        assert_eq!(html, "<p>&lt;script&gt;alert(1)&lt;/script&gt; &amp; &quot;quotes&quot; ©</p>");
        assert!(!html.contains("<script"));
    }

    #[test]
    fn word_lists_and_tables() {
        let li = |t: &str| format!(r#"<w:p><w:pPr><w:numPr><w:ilvl w:val="0"/></w:numPr></w:pPr><w:r><w:t>{t}</w:t></w:r></w:p>"#);
        let cell = |t: &str| format!(r#"<w:tc><w:p><w:r><w:t>{t}</w:t></w:r></w:p></w:tc>"#);
        let xml = doc(&format!(
            "{}{}<w:p><w:r><w:t>After</w:t></w:r></w:p><w:tbl><w:tr>{}{}</w:tr></w:tbl>",
            li("One"),
            li("Two"),
            cell("A1"),
            cell("B1")
        ));
        let (html, _) = docx_xml_to_html(xml.as_bytes());
        assert_eq!(
            html,
            "<ul><li>One</li><li>Two</li></ul><p>After</p><table><tr><td><p>A1</p></td><td><p>B1</p></td></tr></table>"
        );
    }

    #[test]
    fn a_real_docx_zip_is_read() {
        let mut buf = Cursor::new(Vec::new());
        {
            let mut z = zip::ZipWriter::new(&mut buf);
            let opts = zip::write::SimpleFileOptions::default();
            z.start_file("word/document.xml", opts).unwrap();
            std::io::Write::write_all(&mut z, doc(r#"<w:p><w:r><w:t>Hello</w:t></w:r></w:p>"#).as_bytes()).unwrap();
            z.finish().unwrap();
        }
        assert_eq!(docx_html(buf.get_ref()), Some(("<p>Hello</p>".to_string(), false)));
        assert_eq!(docx_html(b"not a zip"), None);
    }

    #[test]
    fn csv_quotes_semicolons_and_shape() {
        let s = delimited_sheet("people.csv", "name,note,\n\"Okafor, Sam\",\"said \"\"hi\"\"\nthen left\",\nAna,,\n", false);
        assert_eq!(
            s.rows,
            vec![
                vec!["name".to_string(), "note".to_string()],
                vec!["Okafor, Sam".to_string(), "said \"hi\"\nthen left".to_string()],
                vec!["Ana".to_string(), String::new()],
            ]
        );
        assert_eq!((s.total_rows, s.total_cols), (3, 2));
        let eu = delimited_sheet("x.csv", "a;b\n1,5;2\n", false);
        assert_eq!(eu.rows[1], vec!["1,5".to_string(), "2".to_string()]);
        let tsv = delimited_sheet("x.tsv", "a\tb\n", true);
        assert_eq!(tsv.rows[0], vec!["a".to_string(), "b".to_string()]);
    }

    /// A minimal .xlsx: one sheet, a shared string, a number, a date.
    fn xlsx() -> Vec<u8> {
        let files = [
            ("[Content_Types].xml", r#"<?xml version="1.0"?><Types xmlns="http://schemas.openxmlformats.org/package/2006/content-types"><Default Extension="rels" ContentType="application/vnd.openxmlformats-package.relationships+xml"/><Default Extension="xml" ContentType="application/xml"/><Override PartName="/xl/workbook.xml" ContentType="application/vnd.openxmlformats-officedocument.spreadsheetml.sheet.main+xml"/><Override PartName="/xl/worksheets/sheet1.xml" ContentType="application/vnd.openxmlformats-officedocument.spreadsheetml.worksheet+xml"/><Override PartName="/xl/sharedStrings.xml" ContentType="application/vnd.openxmlformats-officedocument.spreadsheetml.sharedStrings+xml"/><Override PartName="/xl/styles.xml" ContentType="application/vnd.openxmlformats-officedocument.spreadsheetml.styles+xml"/></Types>"#),
            ("_rels/.rels", r#"<?xml version="1.0"?><Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships"><Relationship Id="rId1" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/officeDocument" Target="xl/workbook.xml"/></Relationships>"#),
            ("xl/workbook.xml", r#"<?xml version="1.0"?><workbook xmlns="http://schemas.openxmlformats.org/spreadsheetml/2006/main" xmlns:r="http://schemas.openxmlformats.org/officeDocument/2006/relationships"><sheets><sheet name="Budget" sheetId="1" r:id="rId1"/></sheets></workbook>"#),
            ("xl/_rels/workbook.xml.rels", r#"<?xml version="1.0"?><Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships"><Relationship Id="rId1" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/worksheet" Target="worksheets/sheet1.xml"/><Relationship Id="rId2" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/sharedStrings" Target="sharedStrings.xml"/><Relationship Id="rId3" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/styles" Target="styles.xml"/></Relationships>"#),
            ("xl/sharedStrings.xml", r#"<?xml version="1.0"?><sst xmlns="http://schemas.openxmlformats.org/spreadsheetml/2006/main" count="2" uniqueCount="2"><si><t>Item</t></si><si><t>Rent</t></si></sst>"#),
            ("xl/styles.xml", r#"<?xml version="1.0"?><styleSheet xmlns="http://schemas.openxmlformats.org/spreadsheetml/2006/main"><cellXfs count="2"><xf numFmtId="0"/><xf numFmtId="14" applyNumberFormat="1"/></cellXfs></styleSheet>"#),
            ("xl/worksheets/sheet1.xml", r#"<?xml version="1.0"?><worksheet xmlns="http://schemas.openxmlformats.org/spreadsheetml/2006/main"><sheetData><row r="1"><c r="A1" t="s"><v>0</v></c></row><row r="2"><c r="A2" t="s"><v>1</v></c><c r="B2"><v>2450</v></c><c r="C2" s="1"><v>46296</v></c></row></sheetData></worksheet>"#),
        ];
        let mut buf = Cursor::new(Vec::new());
        {
            let mut z = zip::ZipWriter::new(&mut buf);
            for (name, body) in files {
                z.start_file(name, zip::write::SimpleFileOptions::default()).unwrap();
                std::io::Write::write_all(&mut z, body.as_bytes()).unwrap();
            }
            z.finish().unwrap();
        }
        buf.into_inner()
    }

    #[test]
    fn a_real_workbook_is_read() {
        let sheets = workbook_sheets(&xlsx()).expect("readable");
        assert_eq!(sheets.len(), 1);
        assert_eq!(sheets[0].name, "Budget");
        assert_eq!(
            sheets[0].rows,
            vec![
                vec!["Item".to_string(), String::new(), String::new()],
                vec!["Rent".to_string(), "2450".to_string(), "2026-10-01".to_string()],
            ]
        );
    }

    #[test]
    fn unreadable_workbooks_are_none() {
        assert_eq!(workbook_sheets(b"PK\x03\x04 nope"), None);
    }
}
