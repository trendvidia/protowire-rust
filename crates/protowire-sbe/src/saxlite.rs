// SPDX-License-Identifier: MIT
// Copyright (c) 2026 TrendVidia, LLC.
//! A small XML tokenizer for SBE schemas. protowire-go reads a schema with
//! Go's `encoding/xml` (a strict `Decoder`, no entity map, no
//! `CharsetReader`), and this is a port of the part of it that reading runs,
//! so it accepts and refuses what that does, with its texts (#144): a
//! syntax error is `XML syntax error on line N: <what>`, and an input cut
//! off anywhere is `unexpected EOF`.
//!
//! Handles the prolog and other processing instructions, comments,
//! `<!DOCTYPE …>` and other directives (skipped), CDATA sections, open,
//! close and self-closing tags, single- or double-quoted attributes, the five
//! named entities and numeric character references. Namespace prefixes are
//! stripped from names, and `xmlns` attributes are dropped.
//!
//! Go checks a non-ASCII name against the XML 1.0 name tables; this port
//! takes a letter for a name's first character and a letter or digit after
//! it. SBE schema names are ASCII, where the two agree.

use std::collections::HashMap;
use std::fmt;

/// An error reading an XML schema, with protowire-go's text.
///
/// `message` is what Go's reader says: a syntax error with its line
/// (`XML syntax error on line 3: unexpected EOF`), a prolog it refuses
/// (`xml: unsupported version "1.1"; only version 1.0 is supported`), or a
/// value that does not parse (`strconv.ParseUint: parsing "x": invalid
/// syntax`). `offset` is the byte offset the error was found at. It shows as
/// protowire-go's `ParseXMLSchema` error does: `sbe: parse XML schema:
/// <message>`.
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
        write!(f, "sbe: parse XML schema: {}", self.message)
    }
}

pub trait SaxHandler {
    fn open(&mut self, name: &str, attrs: &HashMap<String, String>) -> Result<(), SaxError>;
    fn close(&mut self, name: &str) -> Result<(), SaxError>;
    /// Called with character data between tags, CDATA sections included.
    /// May be all whitespace.
    fn text(&mut self, value: &str) -> Result<(), SaxError>;
}

/// Parse `input` and dispatch SAX events to `handler`, as Go's
/// `Decoder.Token` reads a document: to its end, a self-closing tag as an
/// open and a close, every close checked against the open it ends.
pub fn parse_xml(input: &str, handler: &mut dyn SaxHandler) -> Result<(), SaxError> {
    let mut t = Tokenizer::new(input);
    while let Some(tok) = t.token()? {
        match tok {
            Token::Start(name, attrs) => {
                let map = attrs
                    .into_iter()
                    .filter(|a| !a.is_xmlns())
                    .map(|a| (a.name.local, a.value))
                    .collect();
                handler.open(&name.local, &map)?;
            }
            Token::End(name) => handler.close(&name.local)?,
            Token::Text(s) => handler.text(&s)?,
            Token::Other => {}
        }
    }
    Ok(())
}

/// A name and its namespace prefix, as written (Go's `xml.Name` before
/// translation).
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Name {
    pub space: String,
    pub local: String,
}

#[derive(Debug, Clone)]
pub(crate) struct Attr {
    pub name: Name,
    pub value: String,
}

impl Attr {
    /// A namespace declaration, `xmlns` or `xmlns:p`.
    pub fn is_xmlns(&self) -> bool {
        self.name.space == "xmlns" || (self.name.space.is_empty() && self.name.local == "xmlns")
    }
}

#[derive(Debug)]
pub(crate) enum Token {
    Start(Name, Vec<Attr>),
    End(Name),
    Text(String),
    /// A comment, processing instruction or directive.
    Other,
}

/// Go's `xml.Decoder`, strict, read one byte at a time.
pub(crate) struct Tokenizer<'a> {
    b: &'a [u8],
    pos: usize,
    stack: Vec<Name>,
    need_close: Option<Name>,
}

impl<'a> Tokenizer<'a> {
    pub fn new(input: &'a str) -> Self {
        Self {
            b: input.as_bytes(),
            pos: 0,
            stack: Vec::new(),
            need_close: None,
        }
    }

    fn getc(&mut self) -> Option<u8> {
        let b = *self.b.get(self.pos)?;
        self.pos += 1;
        Some(b)
    }

    fn ungetc(&mut self) {
        self.pos -= 1;
    }

    /// `XML syntax error on line N: msg`, the line being the one reading
    /// has reached.
    fn syntax(&self, msg: impl fmt::Display) -> SaxError {
        let line = 1 + self.b[..self.pos].iter().filter(|&&c| c == b'\n').count();
        SaxError::new(format!("XML syntax error on line {line}: {msg}"), self.pos)
    }

    fn mustgetc(&mut self) -> Result<u8, SaxError> {
        self.getc().ok_or_else(|| self.syntax("unexpected EOF"))
    }

    fn space(&mut self) {
        while let Some(b) = self.getc() {
            if !matches!(b, b' ' | b'\r' | b'\n' | b'\t') {
                self.ungetc();
                return;
            }
        }
    }

    /// Go's `readName`: the bytes of a name, or `None` when the next byte
    /// cannot start one.
    fn read_name(&mut self) -> Result<Option<&'a str>, SaxError> {
        let start = self.pos;
        let b = self.mustgetc()?;
        if b < 0x80 && !is_name_byte(b) {
            self.ungetc();
            return Ok(None);
        }
        loop {
            let b = self.mustgetc()?;
            if b < 0x80 && !is_name_byte(b) {
                self.ungetc();
                break;
            }
        }
        // The input is a str, and a name stops only at an ASCII byte.
        Ok(Some(
            std::str::from_utf8(&self.b[start..self.pos]).expect("utf-8"),
        ))
    }

    /// Go's `name`: a name that is a valid XML name.
    fn name(&mut self) -> Result<Option<&'a str>, SaxError> {
        let Some(s) = self.read_name()? else {
            return Ok(None);
        };
        if !is_name(s) {
            return Err(self.syntax(format!("invalid XML name: {s}")));
        }
        Ok(Some(s))
    }

    /// Go's `nsname`: a name split at its prefix.
    fn nsname(&mut self) -> Result<Option<Name>, SaxError> {
        let Some(s) = self.name()? else {
            return Ok(None);
        };
        if s.matches(':').count() > 1 {
            return Ok(None);
        }
        Ok(Some(match s.split_once(':') {
            Some((space, local)) if !space.is_empty() && !local.is_empty() => Name {
                space: space.to_string(),
                local: local.to_string(),
            },
            _ => Name {
                space: String::new(),
                local: s.to_string(),
            },
        }))
    }

    /// Go's `text`: character data up to a `<` (`quote` `None`), an
    /// attribute value up to its closing quote, or a CDATA section up to
    /// `]]>`, with entities replaced and line ends made `\n`.
    fn text(&mut self, quote: Option<u8>, cdata: bool) -> Result<String, SaxError> {
        let (mut b0, mut b1) = (0u8, 0u8);
        let mut buf: Vec<u8> = Vec::new();
        let mut trunc = 0;
        loop {
            let Some(b) = self.getc() else {
                if cdata {
                    return Err(self.syntax("unexpected EOF in CDATA section"));
                }
                break;
            };
            if quote.is_none() && b0 == b']' && b1 == b']' && b == b'>' {
                if cdata {
                    trunc = 2;
                    break;
                }
                return Err(self.syntax("unescaped ]]> not in CDATA section"));
            }
            if b == b'<' && !cdata {
                if quote.is_some() {
                    return Err(self.syntax("unescaped < inside quoted string"));
                }
                self.ungetc();
                break;
            }
            if quote == Some(b) {
                break;
            }
            if b == b'&' && !cdata {
                let before = buf.len();
                buf.push(b'&');
                let mut text: Option<String> = None;
                let mut b = self.mustgetc()?;
                if b == b'#' {
                    buf.push(b);
                    b = self.mustgetc()?;
                    let mut base = 10;
                    if b == b'x' {
                        base = 16;
                        buf.push(b);
                        b = self.mustgetc()?;
                    }
                    let start = buf.len();
                    while b.is_ascii_digit() || base == 16 && b.is_ascii_hexdigit() {
                        buf.push(b);
                        b = self.mustgetc()?;
                    }
                    if b != b';' {
                        self.ungetc();
                    } else {
                        let digits = String::from_utf8(buf[start..].to_vec()).expect("ascii");
                        buf.push(b';');
                        text = u64::from_str_radix(&digits, base)
                            .ok()
                            .filter(|&n| n <= 0x10FFFF)
                            .map(|n| char::from_u32(n as u32).unwrap_or('\u{FFFD}').to_string());
                    }
                } else {
                    self.ungetc();
                    let name = self.read_name()?;
                    if let Some(name) = name {
                        buf.extend_from_slice(name.as_bytes());
                    }
                    let b = self.mustgetc()?;
                    if b != b';' {
                        self.ungetc();
                    } else {
                        buf.push(b';');
                        text = name.filter(|n| is_name(n)).and_then(|n| {
                            Some(
                                match n {
                                    "lt" => "<",
                                    "gt" => ">",
                                    "amp" => "&",
                                    "apos" => "'",
                                    "quot" => "\"",
                                    _ => return None,
                                }
                                .to_string(),
                            )
                        });
                    }
                }
                if let Some(t) = text {
                    buf.truncate(before);
                    buf.extend_from_slice(t.as_bytes());
                    (b0, b1) = (0, 0);
                    continue;
                }
                let mut ent = String::from_utf8_lossy(&buf[before..]).into_owned();
                if !ent.ends_with(';') {
                    ent.push_str(" (no semicolon)");
                }
                return Err(self.syntax(format!("invalid character entity {ent}")));
            }
            if b == b'\r' {
                buf.push(b'\n');
            } else if b1 == b'\r' && b == b'\n' {
                // \r\n: the \n is written already.
            } else {
                buf.push(b);
            }
            (b0, b1) = (b1, b);
        }
        buf.truncate(buf.len() - trunc);
        // Entities can only add whole characters to text that was UTF-8.
        let s = String::from_utf8(buf).expect("utf-8");
        if let Some(c) = s.chars().find(|&c| !in_character_range(c)) {
            return Err(self.syntax(format!("illegal character code U+{:04X}", c as u32)));
        }
        Ok(s)
    }

    fn attrval(&mut self) -> Result<String, SaxError> {
        let b = self.mustgetc()?;
        if b == b'"' || b == b'\'' {
            return self.text(Some(b), false);
        }
        Err(self.syntax("unquoted or missing attribute value in element"))
    }

    /// Go's `rawToken`: the next token, unchecked against the open
    /// elements; `None` at the end of the input.
    fn raw_token(&mut self) -> Result<Option<Token>, SaxError> {
        if let Some(name) = self.need_close.take() {
            return Ok(Some(Token::End(name)));
        }
        let Some(b) = self.getc() else {
            return Ok(None);
        };
        if b != b'<' {
            self.ungetc();
            return Ok(Some(Token::Text(self.text(None, false)?)));
        }
        match self.mustgetc()? {
            b'/' => {
                let Some(name) = self.nsname()? else {
                    return Err(self.syntax("expected element name after </"));
                };
                self.space();
                if self.mustgetc()? != b'>' {
                    return Err(
                        self.syntax(format!("invalid characters between </{} and >", name.local))
                    );
                }
                Ok(Some(Token::End(name)))
            }
            b'?' => {
                let Some(target) = self.name()? else {
                    return Err(self.syntax("expected target name after <?"));
                };
                self.space();
                let start = self.pos;
                let mut b0 = 0u8;
                loop {
                    let b = self.mustgetc()?;
                    if b0 == b'?' && b == b'>' {
                        break;
                    }
                    b0 = b;
                }
                if target == "xml" {
                    let content = std::str::from_utf8(&self.b[start..self.pos - 2]).expect("utf-8");
                    let ver = proc_inst("version", content);
                    if !ver.is_empty() && ver != "1.0" {
                        return Err(SaxError::new(
                            format!(
                                "xml: unsupported version {}; only version 1.0 is supported",
                                go_quote(ver)
                            ),
                            self.pos,
                        ));
                    }
                    let enc = proc_inst("encoding", content);
                    if !enc.is_empty() && !enc.eq_ignore_ascii_case("utf-8") {
                        return Err(SaxError::new(
                            format!(
                                "xml: encoding {} declared but Decoder.CharsetReader is nil",
                                go_quote(enc)
                            ),
                            self.pos,
                        ));
                    }
                }
                Ok(Some(Token::Other))
            }
            b'!' => match self.mustgetc()? {
                b'-' => {
                    if self.mustgetc()? != b'-' {
                        return Err(self.syntax("invalid sequence <!- not part of <!--"));
                    }
                    let (mut b0, mut b1) = (0u8, 0u8);
                    loop {
                        let b = self.mustgetc()?;
                        if b0 == b'-' && b1 == b'-' {
                            if b != b'>' {
                                return Err(
                                    self.syntax(r#"invalid sequence "--" not allowed in comments"#)
                                );
                            }
                            break;
                        }
                        (b0, b1) = (b1, b);
                    }
                    Ok(Some(Token::Other))
                }
                b'[' => {
                    for want in b"CDATA[" {
                        if self.mustgetc()? != *want {
                            return Err(self.syntax("invalid <![ sequence"));
                        }
                    }
                    Ok(Some(Token::Text(self.text(None, true)?)))
                }
                _ => {
                    self.directive()?;
                    Ok(Some(Token::Other))
                }
            },
            _ => {
                self.ungetc();
                let Some(name) = self.nsname()? else {
                    return Err(self.syntax("expected element name after <"));
                };
                let mut attrs = Vec::new();
                loop {
                    self.space();
                    let b = self.mustgetc()?;
                    if b == b'/' {
                        if self.mustgetc()? != b'>' {
                            return Err(self.syntax("expected /> in element"));
                        }
                        self.need_close = Some(name.clone());
                        break;
                    }
                    if b == b'>' {
                        break;
                    }
                    self.ungetc();
                    let Some(attr) = self.nsname()? else {
                        return Err(self.syntax("expected attribute name in element"));
                    };
                    self.space();
                    if self.mustgetc()? != b'=' {
                        return Err(self.syntax("attribute name without = in element"));
                    }
                    self.space();
                    let value = self.attrval()?;
                    attrs.push(Attr { name: attr, value });
                }
                Ok(Some(Token::Start(name, attrs)))
            }
        }
    }

    /// The rest of a `<!…>` directive, its first byte read: up to the `>`
    /// that closes it, past quoted text, nested `<…>` and comments. As in
    /// Go, the first byte is not looked at, and a `<` that does not open a
    /// comment nests.
    fn directive(&mut self) -> Result<(), SaxError> {
        let (mut inquote, mut depth) = (0u8, 0usize);
        loop {
            let mut b = self.mustgetc()?;
            if inquote == 0 && b == b'>' && depth == 0 {
                return Ok(());
            }
            // Go's HandleB, which a `<` that opens no comment re-enters
            // with the byte after it.
            'handle: loop {
                if b == inquote {
                    inquote = 0;
                } else if inquote != 0 {
                } else if b == b'\'' || b == b'"' {
                    inquote = b;
                } else if b == b'>' {
                    depth = depth.saturating_sub(1);
                } else if b == b'<' {
                    for want in b"!--" {
                        b = self.mustgetc()?;
                        if b != *want {
                            depth += 1;
                            continue 'handle;
                        }
                    }
                    let (mut b0, mut b1) = (0u8, 0u8);
                    loop {
                        let c = self.mustgetc()?;
                        if b0 == b'-' && b1 == b'-' && c == b'>' {
                            break;
                        }
                        (b0, b1) = (b1, c);
                    }
                }
                break;
            }
        }
    }

    /// Go's `Decoder.Token`: the next token, every close checked against
    /// the open it ends; `None` at the end of a document whose elements are
    /// all closed.
    pub fn token(&mut self) -> Result<Option<Token>, SaxError> {
        let Some(tok) = self.raw_token()? else {
            if !self.stack.is_empty() {
                return Err(self.syntax("unexpected EOF"));
            }
            return Ok(None);
        };
        match &tok {
            Token::Start(name, _) => self.stack.push(name.clone()),
            Token::End(name) => {
                let Some(open) = self.stack.pop() else {
                    return Err(self.syntax(format!("unexpected end element </{}>", name.local)));
                };
                if open.local != name.local {
                    return Err(self.syntax(format!(
                        "element <{}> closed by </{}>",
                        open.local, name.local
                    )));
                }
                if open.space != name.space {
                    let ns = if name.space.is_empty() {
                        r#""""#
                    } else {
                        &name.space
                    };
                    return Err(self.syntax(format!(
                        "element <{}> in space {} closed by </{}> in space {ns}",
                        open.local, open.space, name.local
                    )));
                }
            }
            _ => {}
        }
        Ok(Some(tok))
    }
}

fn is_name_byte(c: u8) -> bool {
    c.is_ascii_alphanumeric() || matches!(c, b'_' | b':' | b'.' | b'-')
}

/// Go's `isName`: an XML name. See the module doc for non-ASCII names.
fn is_name(s: &str) -> bool {
    let mut chars = s.chars();
    let Some(c) = chars.next() else {
        return false;
    };
    let first = |c: char| {
        c.is_ascii_alphabetic() || c == '_' || c == ':' || (!c.is_ascii() && c.is_alphabetic())
    };
    first(c)
        && chars.all(|c| {
            first(c)
                || c.is_ascii_digit()
                || c == '-'
                || c == '.'
                || c == '\u{B7}'
                || (!c.is_ascii() && c.is_alphanumeric())
        })
}

/// Go's `isInCharacterRange`: a character XML 1.0 allows.
fn in_character_range(c: char) -> bool {
    matches!(c as u32,
        0x09 | 0x0A | 0x0D | 0x20..=0xD7FF | 0xE000..=0xFFFD | 0x10000..=0x10FFFF)
}

/// Go's `procInst`: the value of `param="…"` in a processing instruction's
/// content, or "".
fn proc_inst<'s>(param: &str, s: &'s str) -> &'s str {
    let param = format!("{param}=");
    let mut i = 0;
    let mut sep = 0u8;
    while i < s.len() {
        let sub = &s[i..];
        let Some(k) = sub.find(&param) else {
            return "";
        };
        if param.len() + k >= sub.len() {
            return "";
        }
        i += param.len() + k + 1;
        let c = sub.as_bytes()[param.len() + k];
        if c == b'\'' || c == b'"' {
            sep = c;
            break;
        }
    }
    if sep == 0 {
        return "";
    }
    match s[i..].find(sep as char) {
        Some(j) => &s[i..i + j],
        None => "",
    }
}

/// Go's `strconv.Quote`.
pub(crate) fn go_quote(s: &str) -> String {
    let mut out = String::from("\"");
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\u{7}' => out.push_str("\\a"),
            '\u{8}' => out.push_str("\\b"),
            '\u{c}' => out.push_str("\\f"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            '\u{b}' => out.push_str("\\v"),
            c if (c as u32) < 0x20 || c as u32 == 0x7f => {
                out.push_str(&format!("\\x{:02x}", c as u32))
            }
            c if c.is_control() => out.push_str(&format!("\\u{:04x}", c as u32)),
            c => out.push(c),
        }
    }
    out.push('"');
    out
}
