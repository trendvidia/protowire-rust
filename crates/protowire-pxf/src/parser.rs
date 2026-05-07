// SPDX-License-Identifier: MIT
// Copyright (c) 2026 TrendVidia, LLC.
//! Recursive-descent parser for PXF.
//!
//! Mirrors `protowire/encoding/pxf/parser.go`. Newlines and comments are
//! absorbed at the lexer-token boundary: comments accumulate in a pending
//! buffer and attach as `leading_comments` to the next entry. Trailing
//! inline comments are not yet captured (the formatter populates them).

use crate::ast::{
    Assignment, Block, BlockVal, BoolVal, BytesVal, Comment, Document, DurationVal, Entry,
    FloatVal, IdentVal, IntVal, ListVal, MapEntry, NullVal, StringVal, TimestampVal, Value,
};
use crate::errors::PxfError;
use crate::lexer::Lexer;
use crate::token::{Position, Token, TokenKind};

/// HARDENING.md `MaxNestingDepth`: caps `{` and `[` nesting at 100. The same
/// constant lives on [`crate::decode::MAX_NESTING_DEPTH`]; both apply per the
/// HARDENING.md threat model so adversarial input can't overflow the native
/// recursion stack regardless of which entry point is used.
pub const MAX_NESTING_DEPTH: usize = 100;

pub fn parse(input: &str) -> Result<Document, PxfError> {
    Parser::new(input).parse_document()
}

struct Parser<'a> {
    lex: Lexer<'a>,
    current: Token,
    pending: Vec<Comment>,
    /// Live `{` + `[` depth, incremented at each opening token and
    /// decremented at the matching close. Exceeds → reject before recursing.
    depth: usize,
}

impl<'a> Parser<'a> {
    fn new(input: &'a str) -> Self {
        let mut p = Self {
            lex: Lexer::new(input),
            current: Token::new(TokenKind::Eof, "", Position::new(1, 1)),
            pending: Vec::new(),
            depth: 0,
        };
        p.advance();
        p
    }

    fn enter(&mut self, pos: Position) -> Result<(), PxfError> {
        if self.depth >= MAX_NESTING_DEPTH {
            return Err(PxfError::new(
                pos,
                format!("nesting depth exceeds MaxNestingDepth ({})", MAX_NESTING_DEPTH),
            ));
        }
        self.depth += 1;
        Ok(())
    }

    fn leave(&mut self) {
        self.depth -= 1;
    }

    /// Consume the next token, swallowing newlines and accumulating comments
    /// into the pending buffer so they can be attached to the following entry.
    fn advance(&mut self) {
        loop {
            self.current = self.lex.next_token();
            match self.current.kind {
                TokenKind::Newline => continue,
                TokenKind::Comment => {
                    self.pending.push(Comment {
                        pos: self.current.pos,
                        text: std::mem::take(&mut self.current.value),
                    });
                    continue;
                }
                _ => return,
            }
        }
    }

    fn flush_comments(&mut self) -> Vec<Comment> {
        std::mem::take(&mut self.pending)
    }

    fn parse_document(&mut self) -> Result<Document, PxfError> {
        let leading_comments = self.flush_comments();
        let mut type_url = String::new();

        if matches!(self.current.kind, TokenKind::AtType) {
            self.advance(); // consume @type
            if !matches!(self.current.kind, TokenKind::Ident) {
                return Err(PxfError::new(
                    self.current.pos,
                    format!(
                        "expected type name after @type, got {}",
                        self.current.kind.name()
                    ),
                ));
            }
            type_url = std::mem::take(&mut self.current.value);
            self.advance();
        }

        let mut entries = Vec::new();
        while !matches!(self.current.kind, TokenKind::Eof) {
            // Top-level: only field_entry is allowed. The document represents
            // a proto message, never a map<K,V>; map_entry (`:` form) is
            // reserved for the inside of a '{ ... }' block. See
            // docs/grammar.ebnf -> document.
            entries.push(self.parse_entry(false)?);
        }
        Ok(Document {
            type_url,
            entries,
            leading_comments,
        })
    }

    /// `allow_map_entry` gates the `:` (map-entry) form: false at document
    /// top level, true inside any '{ ... }' block.
    fn parse_entry(&mut self, allow_map_entry: bool) -> Result<Entry, PxfError> {
        let leading_comments = self.flush_comments();
        let pos = self.current.pos;
        let k = self.current.kind;

        if !matches!(k, TokenKind::Ident | TokenKind::String | TokenKind::Int) {
            return Err(PxfError::new(
                pos,
                format!(
                    "expected identifier, string, or integer, got {} ({:?})",
                    k.name(),
                    self.current.value
                ),
            ));
        }
        let key_kind = k;
        let key = std::mem::take(&mut self.current.value);
        self.advance();

        match self.current.kind {
            TokenKind::Equals => {
                // `=` denotes a field assignment on a proto message; the key
                // must be an identifier. Map-style keys (string / integer) are
                // only valid with `:`.
                if !matches!(key_kind, TokenKind::Ident) {
                    return Err(PxfError::new(
                        pos,
                        format!(
                            "field assignment with '=' requires an identifier key, got {} ({:?}); use ':' for map entries",
                            key_kind.name(),
                            key
                        ),
                    ));
                }
                self.advance();
                let value = self.parse_value()?;
                Ok(Entry::Assignment(Assignment {
                    pos,
                    key,
                    value,
                    leading_comments,
                    trailing_comment: String::new(),
                }))
            }
            TokenKind::Colon => {
                // Map entry. Only allowed inside a '{ ... }' block, never at
                // document top level.
                if !allow_map_entry {
                    return Err(PxfError::new(
                        pos,
                        "map entry (':' form) is only allowed inside a '{ … }' block; use '=' for top-level field assignments".to_string(),
                    ));
                }
                self.advance();
                let value = self.parse_value()?;
                Ok(Entry::MapEntry(MapEntry {
                    pos,
                    key,
                    value,
                    leading_comments,
                    trailing_comment: String::new(),
                }))
            }
            TokenKind::LBrace => {
                // `{ ... }` denotes a submessage field; same identifier-only
                // rule as `=` applies.
                if !matches!(key_kind, TokenKind::Ident) {
                    return Err(PxfError::new(
                        pos,
                        format!(
                            "submessage block requires an identifier key, got {} ({:?})",
                            key_kind.name(),
                            key
                        ),
                    ));
                }
                self.advance();
                let entries = self.parse_body(open_pos)?;
                Ok(Entry::Block(Block {
                    pos,
                    name: key,
                    entries,
                    leading_comments,
                }))
            }
            other => Err(PxfError::new(
                self.current.pos,
                format!(
                    "expected '=', ':', or '{{' after {:?}, got {}",
                    key,
                    other.name()
                ),
            )),
        }
    }

    fn parse_value(&mut self) -> Result<Value, PxfError> {
        let pos = self.current.pos;

        match self.current.kind {
            TokenKind::String => {
                let value = std::mem::take(&mut self.current.value);
                self.advance();
                Ok(Value::String(StringVal { pos, value }))
            }
            TokenKind::Int => {
                let raw = std::mem::take(&mut self.current.value);
                self.advance();
                Ok(Value::Int(IntVal { pos, raw }))
            }
            TokenKind::Float => {
                let raw = std::mem::take(&mut self.current.value);
                self.advance();
                Ok(Value::Float(FloatVal { pos, raw }))
            }
            TokenKind::Bool => {
                let value = self.current.value == "true";
                self.advance();
                Ok(Value::Bool(BoolVal { pos, value }))
            }
            TokenKind::Bytes => {
                let decoded = decode_base64(&self.current.value);
                self.advance();
                Ok(Value::Bytes(BytesVal { pos, value: decoded }))
            }
            TokenKind::Timestamp => {
                let raw = std::mem::take(&mut self.current.value);
                self.advance();
                Ok(Value::Timestamp(TimestampVal { pos, raw }))
            }
            TokenKind::Duration => {
                let raw = std::mem::take(&mut self.current.value);
                self.advance();
                Ok(Value::Duration(DurationVal { pos, raw }))
            }
            TokenKind::Null => {
                self.advance();
                Ok(Value::Null(NullVal { pos }))
            }
            TokenKind::Ident => {
                let name = std::mem::take(&mut self.current.value);
                self.advance();
                Ok(Value::Ident(IdentVal { pos, name }))
            }
            TokenKind::LBracket => self.parse_list(),
            TokenKind::LBrace => self.parse_block_val(),
            other => Err(PxfError::new(
                pos,
                format!(
                    "expected value, got {} ({:?})",
                    other.name(),
                    self.current.value
                ),
            )),
        }
    }

    fn parse_list(&mut self) -> Result<Value, PxfError> {
        let pos = self.current.pos;
        self.enter(pos)?;
        self.advance(); // consume [

        let mut elements = Vec::new();
        while !matches!(self.current.kind, TokenKind::RBracket | TokenKind::Eof) {
            elements.push(self.parse_value()?);
            if matches!(self.current.kind, TokenKind::Comma) {
                self.advance();
            }
        }
        if !matches!(self.current.kind, TokenKind::RBracket) {
            return Err(PxfError::new(
                self.current.pos,
                format!("expected ']', got {}", self.current.kind.name()),
            ));
        }
        self.advance();
        self.leave();
        Ok(Value::List(ListVal { pos, elements }))
    }

    fn parse_block_val(&mut self) -> Result<Value, PxfError> {
        let pos = self.current.pos;
        self.advance(); // consume {
        let entries = self.parse_body(pos)?;
        Ok(Value::Block(BlockVal { pos, entries }))
    }

    fn parse_body(&mut self, open_pos: Position) -> Result<Vec<Entry>, PxfError> {
        self.enter(open_pos)?;
        let mut entries = Vec::new();
        while !matches!(self.current.kind, TokenKind::RBrace | TokenKind::Eof) {
            // Inside a '{ ... }' block both forms are accepted; the schema
            // layer disambiguates submessage vs map<K,V>.
            entries.push(self.parse_entry(true)?);
        }
        if !matches!(self.current.kind, TokenKind::RBrace) {
            return Err(PxfError::new(
                self.current.pos,
                format!("expected '}}', got {}", self.current.kind.name()),
            ));
        }
        self.advance();
        self.leave();
        Ok(entries)
    }
}

/// Decode a base64-encoded string (standard or raw — the lexer accepts both).
/// Padding (`=`) is treated as a stop marker; the lexer already rejected
/// length-mod-4 == 1 inputs.
fn decode_base64(s: &str) -> Vec<u8> {
    let bytes = s.as_bytes();
    let mut out: Vec<u8> = Vec::with_capacity(bytes.len() * 3 / 4);
    let mut buf: u32 = 0;
    let mut bits: u32 = 0;
    for &b in bytes {
        if b == b'=' {
            break;
        }
        let v: u8 = match b {
            b'A'..=b'Z' => b - b'A',
            b'a'..=b'z' => b - b'a' + 26,
            b'0'..=b'9' => b - b'0' + 52,
            b'+' => 62,
            b'/' => 63,
            _ => continue,
        };
        buf = (buf << 6) | v as u32;
        bits += 6;
        if bits >= 8 {
            bits -= 8;
            out.push(((buf >> bits) & 0xff) as u8);
            buf &= (1u32 << bits).wrapping_sub(1);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn assignment(e: &Entry) -> &Assignment {
        match e {
            Entry::Assignment(a) => a,
            other => panic!("expected Assignment, got {:?}", other),
        }
    }

    fn block(e: &Entry) -> &Block {
        match e {
            Entry::Block(b) => b,
            other => panic!("expected Block, got {:?}", other),
        }
    }

    fn map_entry(e: &Entry) -> &MapEntry {
        match e {
            Entry::MapEntry(m) => m,
            other => panic!("expected MapEntry, got {:?}", other),
        }
    }

    fn list(v: &Value) -> &ListVal {
        match v {
            Value::List(l) => l,
            other => panic!("expected ListVal, got {:?}", other),
        }
    }

    fn block_val(v: &Value) -> &BlockVal {
        match v {
            Value::Block(b) => b,
            other => panic!("expected BlockVal, got {:?}", other),
        }
    }

    // ---------------- empty / whitespace ----------------

    #[test]
    fn empty_input_empty_document() {
        let doc = parse("").unwrap();
        assert_eq!(doc.type_url, "");
        assert!(doc.entries.is_empty());
        assert!(doc.leading_comments.is_empty());
    }

    #[test]
    fn whitespace_only_empty_document() {
        let doc = parse("\n\n   \n\t\n").unwrap();
        assert!(doc.entries.is_empty());
    }

    // ---------------- @type directive ----------------

    #[test]
    fn at_type_captures_the_type_name() {
        let doc = parse("@type infra.v1.ServerConfig").unwrap();
        assert_eq!(doc.type_url, "infra.v1.ServerConfig");
        assert!(doc.entries.is_empty());
    }

    #[test]
    fn at_type_then_entries() {
        let doc = parse("@type pkg.M\nname = \"x\"").unwrap();
        assert_eq!(doc.type_url, "pkg.M");
        assert_eq!(doc.entries.len(), 1);
        let a = assignment(&doc.entries[0]);
        assert_eq!(a.key, "name");
    }

    #[test]
    fn at_type_missing_identifier_is_pxferror() {
        let err = parse("@type =").unwrap_err();
        assert!(err.msg.contains("expected type name after @type"));
    }

    // ---------------- scalar assignments ----------------

    #[test]
    fn scalar_string() {
        let doc = parse("hostname = \"web-01\"").unwrap();
        let a = assignment(&doc.entries[0]);
        assert_eq!(a.key, "hostname");
        match &a.value {
            Value::String(s) => assert_eq!(s.value, "web-01"),
            other => panic!("expected String, got {:?}", other),
        }
    }

    #[test]
    fn scalar_int() {
        let doc = parse("port = 8443").unwrap();
        let a = assignment(&doc.entries[0]);
        match &a.value {
            Value::Int(i) => assert_eq!(i.raw, "8443"),
            other => panic!("expected Int, got {:?}", other),
        }
    }

    #[test]
    fn scalar_negative_int() {
        let doc = parse("delta = -42").unwrap();
        let a = assignment(&doc.entries[0]);
        match &a.value {
            Value::Int(i) => assert_eq!(i.raw, "-42"),
            other => panic!("expected Int, got {:?}", other),
        }
    }

    #[test]
    fn scalar_float() {
        let doc = parse("ratio = 0.85").unwrap();
        let a = assignment(&doc.entries[0]);
        match &a.value {
            Value::Float(f) => assert_eq!(f.raw, "0.85"),
            other => panic!("expected Float, got {:?}", other),
        }
    }

    #[test]
    fn scalar_bool() {
        let t = parse("enabled = true").unwrap();
        match &assignment(&t.entries[0]).value {
            Value::Bool(b) => assert!(b.value),
            other => panic!("expected Bool, got {:?}", other),
        }
        let f = parse("enabled = false").unwrap();
        match &assignment(&f.entries[0]).value {
            Value::Bool(b) => assert!(!b.value),
            other => panic!("expected Bool, got {:?}", other),
        }
    }

    #[test]
    fn scalar_null() {
        let doc = parse("email = null").unwrap();
        match &assignment(&doc.entries[0]).value {
            Value::Null(_) => {}
            other => panic!("expected Null, got {:?}", other),
        }
    }

    #[test]
    fn scalar_ident_enum_value() {
        let doc = parse("status = STATUS_SERVING").unwrap();
        match &assignment(&doc.entries[0]).value {
            Value::Ident(i) => assert_eq!(i.name, "STATUS_SERVING"),
            other => panic!("expected Ident, got {:?}", other),
        }
    }

    #[test]
    fn scalar_timestamp_keeps_raw() {
        let doc = parse("created_at = 2024-01-15T10:30:00Z").unwrap();
        match &assignment(&doc.entries[0]).value {
            Value::Timestamp(t) => assert_eq!(t.raw, "2024-01-15T10:30:00Z"),
            other => panic!("expected Timestamp, got {:?}", other),
        }
    }

    #[test]
    fn scalar_duration_keeps_raw() {
        let doc = parse("timeout = 1h30m").unwrap();
        match &assignment(&doc.entries[0]).value {
            Value::Duration(d) => assert_eq!(d.raw, "1h30m"),
            other => panic!("expected Duration, got {:?}", other),
        }
    }

    // ---------------- bytes literals ----------------

    #[test]
    fn bytes_padded_base64_decodes_to_vec() {
        let doc = parse("raw = b\"SGVsbG8=\"").unwrap();
        match &assignment(&doc.entries[0]).value {
            Value::Bytes(b) => assert_eq!(b.value, vec![72, 101, 108, 108, 111]),
            other => panic!("expected Bytes, got {:?}", other),
        }
    }

    #[test]
    fn bytes_raw_unpadded_base64_decodes_correctly() {
        let doc = parse("raw = b\"SGVsbG8\"").unwrap();
        match &assignment(&doc.entries[0]).value {
            Value::Bytes(b) => assert_eq!(b.value, vec![72, 101, 108, 108, 111]),
            other => panic!("expected Bytes, got {:?}", other),
        }
    }

    // ---------------- nested blocks ----------------

    #[test]
    fn block_tls_with_assignments() {
        let src = "tls {\n  cert_file = \"/etc/ssl/cert.pem\"\n  verify    = true\n}";
        let doc = parse(src).unwrap();
        assert_eq!(doc.entries.len(), 1);
        let b = block(&doc.entries[0]);
        assert_eq!(b.name, "tls");
        assert_eq!(b.entries.len(), 2);
        assert_eq!(assignment(&b.entries[0]).key, "cert_file");
    }

    #[test]
    fn block_nested_two_levels() {
        let src = "outer {\n  inner {\n    leaf = 1\n  }\n}";
        let doc = parse(src).unwrap();
        let outer = block(&doc.entries[0]);
        let inner = block(&outer.entries[0]);
        assert_eq!(inner.name, "inner");
        assert_eq!(assignment(&inner.entries[0]).key, "leaf");
    }

    // ---------------- lists ----------------

    #[test]
    fn list_scalar_with_commas() {
        let doc = parse("tags = [\"a\", \"b\", \"c\"]").unwrap();
        let a = assignment(&doc.entries[0]);
        let l = list(&a.value);
        assert_eq!(l.elements.len(), 3);
        let strings: Vec<&str> = l
            .elements
            .iter()
            .map(|v| match v {
                Value::String(s) => s.value.as_str(),
                _ => panic!("expected String"),
            })
            .collect();
        assert_eq!(strings, vec!["a", "b", "c"]);
    }

    #[test]
    fn list_scalar_optional_commas_newline_separated() {
        let src = "tags = [\n  \"a\"\n  \"b\"\n  \"c\"\n]";
        let doc = parse(src).unwrap();
        let a = assignment(&doc.entries[0]);
        assert_eq!(list(&a.value).elements.len(), 3);
    }

    #[test]
    fn list_of_inline_blocks() {
        let src = "endpoints = [\n  { path = \"/api\" }\n  { path = \"/health\" }\n]";
        let doc = parse(src).unwrap();
        let a = assignment(&doc.entries[0]);
        let l = list(&a.value);
        assert_eq!(l.elements.len(), 2);
        match &l.elements[0] {
            Value::Block(_) => {}
            other => panic!("expected BlockVal, got {:?}", other),
        }
    }

    // ---------------- maps ----------------

    #[test]
    fn map_string_keyed() {
        let src = "labels = {\n  env: \"prod\"\n  team: \"platform\"\n}";
        let doc = parse(src).unwrap();
        let a = assignment(&doc.entries[0]);
        let b = block_val(&a.value);
        assert_eq!(b.entries.len(), 2);
        let m0 = map_entry(&b.entries[0]);
        assert_eq!(m0.key, "env");
        match &m0.value {
            Value::String(_) => {}
            other => panic!("expected String, got {:?}", other),
        }
    }

    #[test]
    fn map_quoted_string_keys() {
        let src = "labels = {\n  \"key with space\": \"v\"\n}";
        let doc = parse(src).unwrap();
        let a = assignment(&doc.entries[0]);
        let b = block_val(&a.value);
        assert_eq!(map_entry(&b.entries[0]).key, "key with space");
    }

    #[test]
    fn map_int_keyed() {
        let src = "codes = {\n  404: \"Not Found\"\n  500: \"Internal\"\n}";
        let doc = parse(src).unwrap();
        let a = assignment(&doc.entries[0]);
        let b = block_val(&a.value);
        assert_eq!(map_entry(&b.entries[0]).key, "404");
        assert_eq!(map_entry(&b.entries[1]).key, "500");
    }

    // ---------------- comment attachment ----------------

    #[test]
    fn comments_top_of_document_attach_to_doc_leading() {
        let src = "# leading 1\n# leading 2\nname = \"x\"";
        let doc = parse(src).unwrap();
        assert_eq!(doc.leading_comments.len(), 2);
        assert_eq!(doc.leading_comments[0].text, "# leading 1");
        let a = assignment(&doc.entries[0]);
        assert!(a.leading_comments.is_empty());
    }

    #[test]
    fn comments_after_at_type_attach_to_first_entry() {
        let src = "@type pkg.M\n# header comment\nname = \"x\"";
        let doc = parse(src).unwrap();
        assert!(doc.leading_comments.is_empty());
        let a = assignment(&doc.entries[0]);
        assert_eq!(a.leading_comments.len(), 1);
        assert_eq!(a.leading_comments[0].text, "# header comment");
    }

    #[test]
    fn comments_before_at_type_land_in_doc_leading() {
        let src = "# top of file\n@type pkg.M\nname = \"x\"";
        let doc = parse(src).unwrap();
        assert_eq!(doc.leading_comments.len(), 1);
        assert_eq!(doc.leading_comments[0].text, "# top of file");
    }

    #[test]
    fn comments_inside_block_attach_to_next_entry() {
        let src = "outer {\n  # before leaf\n  leaf = 1\n}";
        let doc = parse(src).unwrap();
        let outer = block(&doc.entries[0]);
        let leaf = assignment(&outer.entries[0]);
        assert_eq!(leaf.leading_comments.len(), 1);
    }

    // ---------------- error positions ----------------

    #[test]
    fn error_expected_after_key() {
        let err = parse("name xyz").unwrap_err();
        assert!(err.msg.contains("expected '=', ':', or '{'"), "msg: {}", err.msg);
    }

    #[test]
    fn error_missing_closing_brace() {
        let err = parse("outer {\n  leaf = 1\n").unwrap_err();
        assert!(err.msg.contains("expected '}'"), "msg: {}", err.msg);
    }

    #[test]
    fn error_missing_closing_bracket() {
        let err = parse("tags = [1, 2").unwrap_err();
        assert!(err.msg.contains("expected ']'"), "msg: {}", err.msg);
    }

    #[test]
    fn error_includes_line_col() {
        let err = parse("\n\nname xyz").unwrap_err();
        let s = format!("{}", err);
        assert!(s.starts_with("3:"), "expected 3:..., got {}", s);
    }

    // ---------------- HARDENING.md §Recursion ----------------

    #[test]
    fn deep_nesting_at_limit_is_accepted() {
        // 100 levels of `{` is exactly at the cap and must parse.
        let mut src = String::from("root ");
        for _ in 0..99 {
            src.push_str("{ child ");
        }
        src.push_str("{ leaf = 1 ");
        for _ in 0..100 {
            src.push('}');
        }
        parse(&src).unwrap();
    }

    #[test]
    fn deep_nesting_past_limit_is_rejected() {
        // 200 levels of `{` must reject with the depth error, not crash.
        let mut src = String::from("root ");
        for _ in 0..200 {
            src.push_str("{ child ");
        }
        src.push_str("{ leaf = 1 ");
        for _ in 0..201 {
            src.push('}');
        }
        let err = parse(&src).unwrap_err();
        assert!(
            err.msg.contains("MaxNestingDepth"),
            "expected depth error, got: {}",
            err.msg
        );
    }

    #[test]
    fn deep_nesting_extreme_does_not_overflow_stack() {
        // The crash case from issue #1: 100 000 nested `{`. Must reject
        // cleanly — recursive descent would otherwise SIGABRT here.
        let mut src = String::from("root ");
        for _ in 0..100_000 {
            src.push_str("{ child ");
        }
        src.push_str("{ leaf = 1 ");
        for _ in 0..100_001 {
            src.push('}');
        }
        let err = parse(&src).unwrap_err();
        assert!(err.msg.contains("MaxNestingDepth"));
    }

    #[test]
    fn deep_list_nesting_past_limit_is_rejected() {
        // `[` nesting also counts.
        let mut src = String::from("xs = ");
        for _ in 0..200 {
            src.push('[');
        }
        for _ in 0..200 {
            src.push(']');
        }
        let err = parse(&src).unwrap_err();
        assert!(
            err.msg.contains("MaxNestingDepth"),
            "expected depth error, got: {}",
            err.msg
        );
    }

    // ---------------- end-to-end ----------------

    #[test]
    fn end_to_end_pxf_readme_sample() {
        let src = "@type infra.v1.ServerConfig\n\
                   \n\
                   hostname = \"web-01.prod.example.com\"\n\
                   port     = 8443\n\
                   enabled  = true\n\
                   status   = STATUS_SERVING\n\
                   \n\
                   # Well-known type literals\n\
                   created_at = 2024-01-15T10:30:00Z\n\
                   timeout    = 30s\n\
                   \n\
                   # Nested messages use block syntax\n\
                   tls {\n\
                     cert_file = \"/etc/ssl/cert.pem\"\n\
                     key_file  = \"/etc/ssl/key.pem\"\n\
                     verify    = true\n\
                   }\n\
                   \n\
                   # Repeated fields use list syntax\n\
                   tags = [\"production\", \"us-east\", \"frontend\"]\n\
                   \n\
                   # Maps use : for key-value pairs\n\
                   labels = {\n\
                     env: \"production\"\n\
                     team: \"platform\"\n\
                     \"hello world\": \"quoted keys supported\"\n\
                   }\n\
                   \n\
                   # Repeated messages\n\
                   endpoints = [\n\
                     {\n\
                       path = \"/api/v1/users\"\n\
                       method = \"GET\"\n\
                     }\n\
                     {\n\
                       path = \"/health\"\n\
                       method = \"GET\"\n\
                     }\n\
                   ]\n\
                   \n\
                   # Wrapper type sugar\n\
                   nullable_name = \"present\"\n";
        let doc = parse(src).unwrap();
        assert_eq!(doc.type_url, "infra.v1.ServerConfig");
        assert!(doc.entries.len() > 5);

        // Spot-check by name.
        let mut hostname = false;
        let mut tls = false;
        let mut tags_is_list = false;
        let mut labels_is_block_val = false;
        for e in &doc.entries {
            match e {
                Entry::Assignment(a) => {
                    if a.key == "hostname" {
                        hostname = true;
                    }
                    if a.key == "tags" {
                        tags_is_list = matches!(a.value, Value::List(_));
                    }
                    if a.key == "labels" {
                        labels_is_block_val = matches!(a.value, Value::Block(_));
                    }
                }
                Entry::Block(b) => {
                    if b.name == "tls" {
                        tls = true;
                    }
                }
                Entry::MapEntry(_) => {}
            }
        }
        assert!(hostname);
        assert!(tls);
        assert!(tags_is_list);
        assert!(labels_is_block_val);
    }
}
