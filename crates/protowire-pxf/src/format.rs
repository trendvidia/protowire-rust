// SPDX-License-Identifier: MIT
// Copyright (c) 2026 TrendVidia, LLC.
//! Format an AST `Document` back to PXF source text, preserving comments.
//!
//! Mirrors `protowire/encoding/pxf/format.go`. This is the round-trip-via-AST
//! path used by the `protowire fmt` subcommand. It is lossy in two ways the
//! Go formatter is also lossy:
//!  - List elements are always comma-separated on output (commas are optional
//!    in input).
//!  - The string quoter only re-emits the escapes the lexer accepts (`\"`,
//!    `\\`, `\n`, `\t`, `\r`); other characters — including non-printable
//!    control chars — pass through verbatim. Use a `bytes` field for arbitrary
//!    binary data.

use crate::ast::{Document, Entry, Value};

#[derive(Debug, Clone)]
pub struct FormatOptions {
    /// Indent unit; default is two spaces.
    pub indent: String,
}

impl Default for FormatOptions {
    fn default() -> Self {
        Self {
            indent: "  ".to_string(),
        }
    }
}

pub fn format(doc: &Document) -> String {
    format_with_options(doc, &FormatOptions::default())
}

pub fn format_with_options(doc: &Document, opts: &FormatOptions) -> String {
    let mut f = Formatter {
        out: String::new(),
        indent: &opts.indent,
    };

    if !doc.type_url.is_empty() {
        f.out.push_str("@type ");
        f.out.push_str(&doc.type_url);
        f.out.push_str("\n\n");
    }

    f.write_comments(&doc.leading_comments, 0);
    f.format_entries(&doc.entries, 0);

    f.out
}

struct Formatter<'a> {
    out: String,
    indent: &'a str,
}

impl Formatter<'_> {
    fn write_indent(&mut self, level: usize) {
        for _ in 0..level {
            self.out.push_str(self.indent);
        }
    }

    fn write_comments(&mut self, comments: &[crate::ast::Comment], level: usize) {
        for c in comments {
            self.write_indent(level);
            self.out.push_str(&c.text);
            self.out.push('\n');
        }
    }

    fn format_entries(&mut self, entries: &[Entry], level: usize) {
        for entry in entries {
            match entry {
                Entry::Assignment(a) => {
                    self.write_comments(&a.leading_comments, level);
                    self.write_indent(level);
                    self.out.push_str(&a.key);
                    self.out.push_str(" = ");
                    self.format_value(&a.value, level);
                    if !a.trailing_comment.is_empty() {
                        self.out.push(' ');
                        self.out.push_str(&a.trailing_comment);
                    }
                    self.out.push('\n');
                }
                Entry::MapEntry(m) => {
                    self.write_comments(&m.leading_comments, level);
                    self.write_indent(level);
                    if needs_quoting(&m.key) {
                        self.out.push_str(&quote_string(&m.key));
                    } else {
                        self.out.push_str(&m.key);
                    }
                    self.out.push_str(": ");
                    self.format_value(&m.value, level);
                    if !m.trailing_comment.is_empty() {
                        self.out.push(' ');
                        self.out.push_str(&m.trailing_comment);
                    }
                    self.out.push('\n');
                }
                Entry::Block(b) => {
                    self.write_comments(&b.leading_comments, level);
                    self.write_indent(level);
                    self.out.push_str(&b.name);
                    self.out.push_str(" {\n");
                    self.format_entries(&b.entries, level + 1);
                    self.write_indent(level);
                    self.out.push_str("}\n");
                }
            }
        }
    }

    fn format_value(&mut self, val: &Value, level: usize) {
        match val {
            Value::String(s) => self.out.push_str(&quote_string(&s.value)),
            Value::Int(i) => self.out.push_str(&i.raw),
            Value::Float(f) => self.out.push_str(&f.raw),
            Value::Bool(b) => self.out.push_str(if b.value { "true" } else { "false" }),
            Value::Bytes(bv) => {
                self.out.push_str("b\"");
                self.out.push_str(&encode_base64(&bv.value));
                self.out.push('"');
            }
            Value::Null(_) => self.out.push_str("null"),
            Value::Ident(i) => self.out.push_str(&i.name),
            Value::Timestamp(t) => self.out.push_str(&t.raw),
            Value::Duration(d) => self.out.push_str(&d.raw),
            Value::List(l) => {
                self.out.push_str("[\n");
                let last = l.elements.len().saturating_sub(1);
                for (i, elem) in l.elements.iter().enumerate() {
                    self.write_indent(level + 1);
                    self.format_value(elem, level + 1);
                    if i < last {
                        self.out.push(',');
                    }
                    self.out.push('\n');
                }
                self.write_indent(level);
                self.out.push(']');
            }
            Value::Block(b) => {
                self.out.push_str("{\n");
                self.format_entries(&b.entries, level + 1);
                self.write_indent(level);
                self.out.push('}');
            }
        }
    }
}

/// Re-emit only the escape sequences the lexer recognizes. Non-printable
/// characters that aren't in the list pass through unchanged — a limitation
/// users should avoid by using a `bytes` field.
fn quote_string(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 2);
    out.push('"');
    for ch in s.chars() {
        match ch {
            '\\' => out.push_str("\\\\"),
            '"' => out.push_str("\\\""),
            '\n' => out.push_str("\\n"),
            '\t' => out.push_str("\\t"),
            '\r' => out.push_str("\\r"),
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

/// A map key needs quoting unless it's a valid identifier — first char must
/// be `[A-Za-z_]`, subsequent chars must be `[A-Za-z0-9_]`. Numeric keys
/// (e.g. `404`) end up quoted because the first char isn't ident-start.
fn needs_quoting(s: &str) -> bool {
    if s.is_empty() {
        return true;
    }
    for (i, ch) in s.chars().enumerate() {
        if i == 0 {
            if !is_ident_start_char(ch) {
                return true;
            }
        } else if !is_ident_start_char(ch) && !ch.is_ascii_digit() {
            return true;
        }
    }
    false
}

fn is_ident_start_char(ch: char) -> bool {
    ch.is_ascii_alphabetic() || ch == '_'
}

/// Standard base64 encode with `=` padding (Go's `base64.StdEncoding`).
fn encode_base64(bytes: &[u8]) -> String {
    const ALPHA: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity((bytes.len() + 2) / 3 * 4);
    let mut i = 0;
    while i + 3 <= bytes.len() {
        let n = ((bytes[i] as u32) << 16) | ((bytes[i + 1] as u32) << 8) | (bytes[i + 2] as u32);
        out.push(ALPHA[((n >> 18) & 0x3f) as usize] as char);
        out.push(ALPHA[((n >> 12) & 0x3f) as usize] as char);
        out.push(ALPHA[((n >> 6) & 0x3f) as usize] as char);
        out.push(ALPHA[(n & 0x3f) as usize] as char);
        i += 3;
    }
    let rem = bytes.len() - i;
    if rem == 1 {
        let n = (bytes[i] as u32) << 16;
        out.push(ALPHA[((n >> 18) & 0x3f) as usize] as char);
        out.push(ALPHA[((n >> 12) & 0x3f) as usize] as char);
        out.push_str("==");
    } else if rem == 2 {
        let n = ((bytes[i] as u32) << 16) | ((bytes[i + 1] as u32) << 8);
        out.push(ALPHA[((n >> 18) & 0x3f) as usize] as char);
        out.push(ALPHA[((n >> 12) & 0x3f) as usize] as char);
        out.push(ALPHA[((n >> 6) & 0x3f) as usize] as char);
        out.push('=');
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::parser::parse;

    fn round_trip(src: &str) -> String {
        format(&parse(src).unwrap())
    }

    // ---------------- scalar values ----------------

    #[test]
    fn scalar_string() {
        assert_eq!(round_trip("name = \"Alice\""), "name = \"Alice\"\n");
    }

    #[test]
    fn scalar_escape_sequences_re_emitted() {
        let src = "msg = \"line1\\nline2\\t\\\"q\\\"\"";
        let expected = "msg = \"line1\\nline2\\t\\\"q\\\"\"\n";
        assert_eq!(round_trip(src), expected);
    }

    #[test]
    fn scalar_int_negative_int_float() {
        assert_eq!(round_trip("port = 8443"), "port = 8443\n");
        assert_eq!(round_trip("delta = -42"), "delta = -42\n");
        assert_eq!(round_trip("ratio = 0.85"), "ratio = 0.85\n");
    }

    #[test]
    fn scalar_bool_null_ident() {
        assert_eq!(round_trip("enabled = true"), "enabled = true\n");
        assert_eq!(round_trip("email = null"), "email = null\n");
        assert_eq!(
            round_trip("status = STATUS_SERVING"),
            "status = STATUS_SERVING\n"
        );
    }

    #[test]
    fn scalar_timestamp_and_duration_use_raw_lexeme() {
        assert_eq!(
            round_trip("created_at = 2024-01-15T10:30:00Z"),
            "created_at = 2024-01-15T10:30:00Z\n"
        );
        assert_eq!(round_trip("timeout = 1h30m"), "timeout = 1h30m\n");
    }

    #[test]
    fn scalar_bytes_re_encode_to_padded_base64() {
        // "Hello" → SGVsbG8= regardless of whether the input was raw or padded.
        assert_eq!(round_trip("raw = b\"SGVsbG8\""), "raw = b\"SGVsbG8=\"\n");
        assert_eq!(round_trip("raw = b\"SGVsbG8=\""), "raw = b\"SGVsbG8=\"\n");
    }

    // ---------------- @type directive and document leading comments ----------------

    #[test]
    fn at_type_appears_at_top_with_blank_line_after() {
        let out = round_trip("@type pkg.M\nname = \"x\"");
        assert_eq!(out, "@type pkg.M\n\nname = \"x\"\n");
    }

    #[test]
    fn doc_level_leading_comments_emitted_before_entries() {
        let src = "# top of file\n@type pkg.M\nname = \"x\"";
        assert_eq!(
            round_trip(src),
            "@type pkg.M\n\n# top of file\nname = \"x\"\n"
        );
    }

    // ---------------- blocks ----------------

    #[test]
    fn block_indents_two_spaces_by_default() {
        let src = "tls {\n  cert_file = \"/etc/ssl/cert.pem\"\n  verify    = true\n}";
        assert_eq!(
            round_trip(src),
            "tls {\n  cert_file = \"/etc/ssl/cert.pem\"\n  verify = true\n}\n"
        );
    }

    #[test]
    fn block_custom_indent_option_four_space() {
        let src = "tls { verify = true }";
        let opts = FormatOptions {
            indent: "    ".to_string(),
        };
        let out = format_with_options(&parse(src).unwrap(), &opts);
        assert_eq!(out, "tls {\n    verify = true\n}\n");
    }

    #[test]
    fn block_nested_blocks() {
        let out = round_trip("a { b { c = 1 } }");
        assert_eq!(out, "a {\n  b {\n    c = 1\n  }\n}\n");
    }

    // ---------------- lists ----------------

    #[test]
    fn list_scalar_emits_commas_between_elements_none_after_last() {
        let out = round_trip("tags = [\"a\", \"b\", \"c\"]");
        assert_eq!(out, "tags = [\n  \"a\",\n  \"b\",\n  \"c\"\n]\n");
    }

    #[test]
    fn list_normalizes_commaless_into_comma_separated() {
        let src = "tags = [\n  \"a\"\n  \"b\"\n]";
        assert_eq!(round_trip(src), "tags = [\n  \"a\",\n  \"b\"\n]\n");
    }

    #[test]
    fn list_of_inline_blocks() {
        let src = "endpoints = [\n  { path = \"/api\" }\n  { path = \"/health\" }\n]";
        let out = round_trip(src);
        assert_eq!(
            out,
            "endpoints = [\n  {\n    path = \"/api\"\n  },\n  {\n    path = \"/health\"\n  }\n]\n"
        );
    }

    // ---------------- maps ----------------

    #[test]
    fn map_string_keyed() {
        let src = "labels = {\n  env: \"prod\"\n  team: \"platform\"\n}";
        assert_eq!(
            round_trip(src),
            "labels = {\n  env: \"prod\"\n  team: \"platform\"\n}\n"
        );
    }

    #[test]
    fn map_keys_with_non_ident_chars_get_quoted() {
        let src = "labels = {\n  \"key with space\": \"v\"\n}";
        assert_eq!(
            round_trip(src),
            "labels = {\n  \"key with space\": \"v\"\n}\n"
        );
    }

    #[test]
    fn map_int_keyed_quotes_numeric_keys() {
        // "404" / "500" begin with a digit, so needs_quoting returns true and
        // the formatter wraps them in quotes.
        let src = "codes = {\n  404: \"Not Found\"\n  500: \"Internal\"\n}";
        assert_eq!(
            round_trip(src),
            "codes = {\n  \"404\": \"Not Found\"\n  \"500\": \"Internal\"\n}\n"
        );
    }

    // ---------------- comment preservation ----------------

    #[test]
    fn comments_leading_on_entries_preserved() {
        let src = "# explain this\nname = \"x\"";
        assert_eq!(round_trip(src), "# explain this\nname = \"x\"\n");
    }

    #[test]
    fn comments_inside_a_block_stay_inside() {
        let src = "tls {\n  # cert path\n  cert_file = \"/etc/cert.pem\"\n}";
        assert_eq!(
            round_trip(src),
            "tls {\n  # cert path\n  cert_file = \"/etc/cert.pem\"\n}\n"
        );
    }

    #[test]
    fn comments_slash_and_block_styles_round_trip_verbatim() {
        let src = "// slash comment\n/* block comment */\nname = \"x\"";
        assert_eq!(
            round_trip(src),
            "// slash comment\n/* block comment */\nname = \"x\"\n"
        );
    }

    // ---------------- end-to-end idempotence ----------------

    #[test]
    fn formatting_twice_is_a_no_op() {
        let src = "@type pkg.M\n\
                   \n\
                   # header\n\
                   name = \"Alice\"\n\
                   port = 8443\n\
                   enabled = true\n\
                   tls {\n  cert_file = \"/etc/cert.pem\"\n}\n\
                   tags = [\n  \"a\",\n  \"b\"\n]\n\
                   labels = {\n  env: \"prod\"\n}\n";
        let once = format(&parse(src).unwrap());
        let twice = format(&parse(&once).unwrap());
        assert_eq!(twice, once);
    }

    #[test]
    fn end_to_end_pxf_readme_sample_is_idempotent() {
        let src = "@type infra.v1.ServerConfig\n\
                   \n\
                   hostname = \"web-01.prod.example.com\"\n\
                   port = 8443\n\
                   enabled = true\n\
                   status = STATUS_SERVING\n\
                   created_at = 2024-01-15T10:30:00Z\n\
                   timeout = 30s\n\
                   tls {\n  cert_file = \"/etc/ssl/cert.pem\"\n  verify = true\n}\n\
                   tags = [\n  \"production\",\n  \"us-east\"\n]\n\
                   labels = {\n  env: \"production\"\n}\n";
        let once = format(&parse(src).unwrap());
        let twice = format(&parse(&once).unwrap());
        assert_eq!(twice, once);
    }
}
