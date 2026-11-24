//! Text extraction. Every parser here sees untrusted input, so failures become errors, never panics or hangs.

use std::io::Read as _;
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::path::Path;

use ego_tree::NodeRef;
use quick_xml::Reader;
use quick_xml::events::Event;
use scraper::{Html, Node, Selector};

#[derive(Debug, thiserror::Error)]
pub enum ExtractError {
    #[error("unsupported type {0}")]
    Unsupported(String),
    #[error("cannot read {0}: {1}")]
    Unreadable(&'static str, String),
    #[error(transparent)]
    Io(#[from] std::io::Error),
}

/// Cap on decompressed DOCX content, so a zip bomb cannot exhaust memory.
const MAX_DOCX_XML: u64 = 64 * 1024 * 1024;

/// `(title, pages)`. Plain formats are a single page.
pub fn extract(path: &Path) -> Result<(String, Vec<String>), ExtractError> {
    let ext = path.extension().and_then(|e| e.to_str()).map(str::to_lowercase).unwrap_or_default();
    let stem = path.file_stem().and_then(|s| s.to_str()).unwrap_or("").to_owned();
    match ext.as_str() {
        "html" | "htm" => {
            let (title, text) = html_text(&String::from_utf8_lossy(&std::fs::read(path)?));
            Ok((title.filter(|t| !t.is_empty()).unwrap_or(stem), vec![text]))
        }
        "pdf" => Ok((stem, pdf_pages(&std::fs::read(path)?)?)),
        "docx" => Ok((stem, vec![docx_text(&std::fs::read(path)?)?])),
        "txt" | "md" | "markdown" | "rst" | "csv" | "log" => {
            Ok((stem, vec![String::from_utf8_lossy(&std::fs::read(path)?).into_owned()]))
        }
        other => Err(ExtractError::Unsupported(format!(".{other}"))),
    }
}

/// Visible text of an HTML document with block elements on their own lines; `script` and `style` are skipped.
pub fn html_text(html: &str) -> (Option<String>, String) {
    let doc = Html::parse_document(html);
    let title = Selector::parse("title")
        .ok()
        .and_then(|s| doc.select(&s).next())
        .map(|t| t.text().collect::<String>().trim().to_owned());
    let mut out = String::new();
    walk(doc.tree.root(), &mut out);
    (title, out)
}

fn walk(node: NodeRef<'_, Node>, out: &mut String) {
    for child in node.children() {
        match child.value() {
            Node::Text(t) => out.push_str(t),
            Node::Element(el) => {
                let name = el.name();
                if matches!(name, "script" | "style" | "head" | "noscript" | "template") {
                    continue;
                }
                let block = matches!(
                    name,
                    "p" | "div" | "li" | "br" | "h1" | "h2" | "h3" | "h4" | "tr" | "section" | "article" | "table"
                );
                if block {
                    out.push('\n');
                }
                walk(child, out);
                if block {
                    out.push('\n');
                }
            }
            _ => {}
        }
    }
}

fn pdf_pages(bytes: &[u8]) -> Result<Vec<String>, ExtractError> {
    // The parser can panic on malformed files; a bad document must not take the process down.
    catch_unwind(AssertUnwindSafe(|| pdf_extract::extract_text_from_mem_by_pages(bytes)))
        .map_err(|_| ExtractError::Unreadable("PDF", "malformed file".into()))?
        .map_err(|e| ExtractError::Unreadable("PDF", e.to_string()))
}

fn docx_text(bytes: &[u8]) -> Result<String, ExtractError> {
    let bad = |e: String| ExtractError::Unreadable("DOCX", e);
    let mut zip = zip::ZipArchive::new(std::io::Cursor::new(bytes)).map_err(|e| bad(e.to_string()))?;
    let entry = zip.by_name("word/document.xml").map_err(|e| bad(e.to_string()))?;
    let mut xml = Vec::new();
    entry.take(MAX_DOCX_XML + 1).read_to_end(&mut xml)?;
    if xml.len() as u64 > MAX_DOCX_XML {
        return Err(bad("document.xml is too large".into()));
    }
    let mut reader = Reader::from_reader(xml.as_slice());
    let (mut out, mut in_text) = (String::new(), false);
    loop {
        match reader.read_event().map_err(|e| bad(e.to_string()))? {
            Event::Start(e) if e.local_name().as_ref() == "t" => in_text = true,
            Event::End(e) if e.local_name().as_ref() == "t" => in_text = false,
            Event::End(e) if e.local_name().as_ref() == "p" => out.push('\n'),
            Event::Text(t) if in_text => out.push_str(&t.xml10_content()),
            // since quick-xml 0.39 entities arrive as their own events instead of being expanded inside the text
            Event::GeneralRef(r) if in_text => {
                if let Some(c) = r.resolve_char_ref().map_err(|e| bad(e.to_string()))? {
                    out.push(c);
                } else {
                    match &*r {
                        "lt" => out.push('<'),
                        "gt" => out.push('>'),
                        "amp" => out.push('&'),
                        "apos" => out.push('\''),
                        "quot" => out.push('"'),
                        _ => {}
                    }
                }
            }
            Event::Eof => break,
            _ => {}
        }
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use std::io::Write as _;

    use super::*;

    #[test]
    fn html_skips_scripts_and_styles_and_finds_the_title() {
        let (title, text) = html_text(
            "<html><head><title>Onboarding</title><style>p{}</style></head><body><script>alert(1)</script><p>Laptops on day one.</p><div>Badge</div></body></html>",
        );
        assert_eq!(title.as_deref(), Some("Onboarding"));
        assert!(
            text.contains("Laptops on day one.")
                && text.contains("Badge")
                && !text.contains("alert")
                && !text.contains("p{}")
        );
    }

    #[test]
    fn plain_text_formats_and_unsupported_types() {
        let d = tempfile::tempdir().unwrap();
        std::fs::write(d.path().join("a.md"), "# Title\n\nbody text").unwrap();
        std::fs::write(d.path().join("bad.xyz"), "x").unwrap();
        let (t, pages) = extract(&d.path().join("a.md")).unwrap();
        assert_eq!((t.as_str(), pages.len()), ("a", 1));
        assert!(matches!(extract(&d.path().join("bad.xyz")), Err(ExtractError::Unsupported(_))));
        std::fs::write(d.path().join("lossy.txt"), [0x66, 0x6f, 0xff, 0x6f]).unwrap();
        assert!(
            extract(&d.path().join("lossy.txt")).unwrap().1[0].contains('\u{fffd}'),
            "invalid UTF-8 is replaced, not an error"
        );
    }

    fn make_docx(body_xml: &str) -> Vec<u8> {
        let mut buf = Vec::new();
        {
            let mut z = zip::ZipWriter::new(std::io::Cursor::new(&mut buf));
            z.start_file("word/document.xml", zip::write::SimpleFileOptions::default()).unwrap();
            z.write_all(format!(r#"<?xml version="1.0"?><w:document xmlns:w="http://schemas.openxmlformats.org/wordprocessingml/2006/main"><w:body>{body_xml}</w:body></w:document>"#).as_bytes()).unwrap();
            z.finish().unwrap();
        }
        buf
    }

    #[test]
    fn docx_paragraphs_are_extracted() {
        let d = tempfile::tempdir().unwrap();
        std::fs::write(
            d.path().join("r.docx"),
            make_docx(
                "<w:p><w:r><w:t>Quarterly review</w:t></w:r></w:p><w:p><w:r><w:t>covers revenue</w:t></w:r></w:p>",
            ),
        )
        .unwrap();
        let (_, pages) = extract(&d.path().join("r.docx")).unwrap();
        assert_eq!(pages[0].trim(), "Quarterly review\ncovers revenue");
    }

    #[test]
    fn docx_entities_and_character_references_are_decoded() {
        let d = tempfile::tempdir().unwrap();
        std::fs::write(
            d.path().join("e.docx"),
            make_docx("<w:p><w:r><w:t>Terms &amp; Conditions: 5 &lt; 6 &#233;t&#xE9; &quot;ok&quot;</w:t></w:r></w:p>"),
        )
        .unwrap();
        let (_, pages) = extract(&d.path().join("e.docx")).unwrap();
        assert_eq!(pages[0].trim(), "Terms & Conditions: 5 < 6 \u{e9}t\u{e9} \"ok\"");
    }

    #[test]
    fn hostile_files_become_errors_not_panics() {
        let d = tempfile::tempdir().unwrap();
        std::fs::write(d.path().join("bad.pdf"), b"not a pdf").unwrap();
        std::fs::write(d.path().join("bad.docx"), b"PK\x03\x04garbage").unwrap();
        std::fs::write(d.path().join("empty.pdf"), b"").unwrap();
        for f in ["bad.pdf", "bad.docx", "empty.pdf"] {
            assert!(matches!(extract(&d.path().join(f)), Err(ExtractError::Unreadable(..))), "{f}");
        }
    }
}
