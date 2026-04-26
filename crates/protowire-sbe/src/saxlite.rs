//! Tiny SAX-style XML parser, sized for SBE schemas only. Mirrors
//! `protowire/encoding/sbe/xmlschema.go` (the SAX section) and the TS port's
//! `sbe/saxlite.ts` line-for-line.
//!
//! Handles: prolog (`<?xml ...?>`), comments, DOCTYPE (skipped), open / close
//! / self-closing tags, attributes (single- or double-quoted), char data,
//! the five named entities, and namespace prefixes (stripped, with
//! `xmlns:*` attributes silently dropped).
//!
//! Does NOT handle: CDATA sections, processing instructions other than the
//! prolog, numeric character references, custom entities, or DTDs. The SBE
//! schema vocabulary doesn't need any of those.

use std::collections::HashMap;
use std::fmt;

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub struct SaxError {
    pub message: String,
    pub offset: usize,
}

impl SaxError {
    pub fn new(message: impl Into<String>, offset: usize) -> Self {
        Self {
            message: message.into(),
            offset,
        }
    }
}

impl fmt::Display for SaxError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "sbe-sax: {} at offset {}", self.message, self.offset)
    }
}

pub trait SaxHandler {
    fn open(&mut self, name: &str, attrs: &HashMap<String, String>) -> Result<(), SaxError>;
    fn close(&mut self, name: &str) -> Result<(), SaxError>;
    /// Called with raw character data between tags. May be all whitespace.
    fn text(&mut self, value: &str) -> Result<(), SaxError>;
}

/// Parse `input` and dispatch SAX events to `handler`. Operates byte-by-byte
/// on the input's bytes; SBE schemas are ASCII for tags/attributes (UTF-8 in
/// text content is passed through unchanged).
pub fn parse_xml(input: &str, handler: &mut dyn SaxHandler) -> Result<(), SaxError> {
    let bytes = input.as_bytes();
    let len = bytes.len();
    let mut i = 0;

    while i < len {
        if bytes[i] != b'<' {
            let start = i;
            while i < len && bytes[i] != b'<' {
                i += 1;
            }
            let raw = &input[start..i];
            handler.text(&decode_entities(raw))?;
            continue;
        }

        // bytes[i] == '<' — figure out which markup token.
        if starts_with(bytes, i, b"<!--") {
            let end = find(bytes, i + 4, b"-->")
                .ok_or_else(|| SaxError::new("unterminated comment", i))?;
            i = end + 3;
            continue;
        }
        if starts_with(bytes, i, b"<?") {
            let end = find(bytes, i + 2, b"?>").ok_or_else(|| {
                SaxError::new("unterminated processing instruction", i)
            })?;
            i = end + 2;
            continue;
        }
        if starts_with(bytes, i, b"<!") {
            let end = find(bytes, i + 2, b">")
                .ok_or_else(|| SaxError::new("unterminated declaration", i))?;
            i = end + 1;
            continue;
        }

        if peek(bytes, i + 1) == Some(b'/') {
            // Close tag: </name>
            i += 2;
            let name_end = find_tag_name_end(bytes, i);
            let raw_name = &input[i..name_end];
            i = name_end;
            i = skip_space(bytes, i);
            if peek(bytes, i) != Some(b'>') {
                return Err(SaxError::new("expected '>' to end close tag", i));
            }
            i += 1;
            handler.close(strip_namespace(raw_name))?;
            continue;
        }

        // Open or self-closing tag.
        i += 1;
        let name_end = find_tag_name_end(bytes, i);
        if name_end == i {
            return Err(SaxError::new("expected tag name after '<'", i));
        }
        let raw_name = &input[i..name_end];
        i = name_end;

        let mut attrs: HashMap<String, String> = HashMap::new();
        let mut self_close = false;
        loop {
            i = skip_space(bytes, i);
            if i >= len {
                return Err(SaxError::new("unterminated tag", i));
            }
            let c = bytes[i];
            if c == b'>' {
                i += 1;
                break;
            }
            if c == b'/' && peek(bytes, i + 1) == Some(b'>') {
                self_close = true;
                i += 2;
                break;
            }

            let attr_start = i;
            while i < len && !is_attr_name_terminator(bytes[i]) {
                i += 1;
            }
            if i == attr_start {
                return Err(SaxError::new("expected attribute name", i));
            }
            let attr_name = &input[attr_start..i];

            i = skip_space(bytes, i);
            if peek(bytes, i) != Some(b'=') {
                return Err(SaxError::new(
                    format!("expected '=' after attribute {}", attr_name),
                    i,
                ));
            }
            i += 1;
            i = skip_space(bytes, i);

            let quote = peek(bytes, i);
            if quote != Some(b'"') && quote != Some(b'\'') {
                return Err(SaxError::new(
                    format!("expected quoted value for attribute {}", attr_name),
                    i,
                ));
            }
            let quote_byte = quote.unwrap();
            i += 1;
            let val_start = i;
            while i < len && bytes[i] != quote_byte {
                i += 1;
            }
            if i >= len {
                return Err(SaxError::new("unterminated attribute value", val_start));
            }
            let raw_value = &input[val_start..i];
            i += 1;

            // Drop xmlns declarations; the caller handles namespaces uniformly
            // via strip_namespace on element/attribute names.
            if attr_name == "xmlns" || attr_name.starts_with("xmlns:") {
                continue;
            }
            attrs.insert(
                strip_namespace(attr_name).to_string(),
                decode_entities(raw_value),
            );
        }

        let name = strip_namespace(raw_name);
        handler.open(name, &attrs)?;
        if self_close {
            handler.close(name)?;
        }
    }
    Ok(())
}

fn starts_with(bytes: &[u8], i: usize, needle: &[u8]) -> bool {
    if i + needle.len() > bytes.len() {
        return false;
    }
    &bytes[i..i + needle.len()] == needle
}

fn find(bytes: &[u8], start: usize, needle: &[u8]) -> Option<usize> {
    if needle.is_empty() || start > bytes.len() {
        return None;
    }
    let mut i = start;
    while i + needle.len() <= bytes.len() {
        if &bytes[i..i + needle.len()] == needle {
            return Some(i);
        }
        i += 1;
    }
    None
}

fn peek(bytes: &[u8], i: usize) -> Option<u8> {
    bytes.get(i).copied()
}

fn find_tag_name_end(bytes: &[u8], start: usize) -> usize {
    let mut i = start;
    while i < bytes.len() && !is_name_terminator(bytes[i]) {
        i += 1;
    }
    i
}

fn is_name_terminator(c: u8) -> bool {
    matches!(c, b' ' | b'\t' | b'\n' | b'\r' | b'>' | b'/')
}

fn is_attr_name_terminator(c: u8) -> bool {
    matches!(c, b'=' | b' ' | b'\t' | b'\n' | b'\r' | b'/' | b'>')
}

fn skip_space(bytes: &[u8], mut i: usize) -> usize {
    while i < bytes.len() && matches!(bytes[i], b' ' | b'\t' | b'\n' | b'\r') {
        i += 1;
    }
    i
}

fn strip_namespace(name: &str) -> &str {
    match name.find(':') {
        Some(idx) => &name[idx + 1..],
        None => name,
    }
}

fn decode_entities(s: &str) -> String {
    if !s.contains('&') {
        return s.to_string();
    }
    s.replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"")
        .replace("&apos;", "'")
        .replace("&amp;", "&")
}
