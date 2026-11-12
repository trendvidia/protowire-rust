// SPDX-License-Identifier: MIT
// Copyright (c) 2026 TrendVidia, LLC.
//! An illegal token is named as protowire-go's lexer names it (#149): a
//! character no token starts with is "unexpected character" and the
//! character as Go's `%q` writes it, escaped wherever `strconv.IsPrint`
//! refuses it, and `@`, `+`, `-` and a raw newline in a string carry Go's
//! sentences. At a value, `parse` reports the lexer's text alone, as Go's
//! `parseValue` does.
//!
//! Each row is what protowire-go v1.17.0's `Parse` gave for the document,
//! built with go1.27.4.

use protowire_pxf::parse;

#[rustfmt::skip]
const ROWS: &[(&str, Option<&str>)] = &[
    ("$ = 1", Some("1:1: expected identifier, string, integer, or bool, got unexpected character '$'")),
    ("% = 1", Some("1:1: expected identifier, string, integer, or bool, got unexpected character '%'")),
    ("^ = 1", Some("1:1: expected identifier, string, integer, or bool, got unexpected character '^'")),
    ("& = 1", Some("1:1: expected identifier, string, integer, or bool, got unexpected character '&'")),
    ("~ = 1", Some("1:1: expected identifier, string, integer, or bool, got unexpected character '~'")),
    ("! = 1", Some("1:1: expected identifier, string, integer, or bool, got unexpected character '!'")),
    ("? = 1", Some("1:1: expected identifier, string, integer, or bool, got unexpected character '?'")),
    ("` = 1", Some("1:1: expected identifier, string, integer, or bool, got unexpected character '`'")),
    ("\\ = 1", Some("1:1: expected identifier, string, integer, or bool, got unexpected character '\\\\'")),
    ("; = 1", Some("1:1: expected identifier, string, integer, or bool, got unexpected character ';'")),
    ("| = 1", Some("1:1: expected identifier, string, integer, or bool, got unexpected character '|'")),
    ("' = 1", Some("1:1: expected identifier, string, integer, or bool, got unexpected character '\\''")),
    ("* = 1", Some("1:1: expected identifier, string, integer, or bool, got unexpected character '*'")),
    ("\u{0} = 1", Some("1:1: expected identifier, string, integer, or bool, got unexpected character '\\x00'")),
    ("\u{1} = 1", Some("1:1: expected identifier, string, integer, or bool, got unexpected character '\\x01'")),
    ("\u{7} = 1", Some("1:1: expected identifier, string, integer, or bool, got unexpected character '\\a'")),
    ("\u{8} = 1", Some("1:1: expected identifier, string, integer, or bool, got unexpected character '\\b'")),
    ("\u{b} = 1", Some("1:1: expected identifier, string, integer, or bool, got unexpected character '\\v'")),
    ("\u{c} = 1", Some("1:1: expected identifier, string, integer, or bool, got unexpected character '\\f'")),
    ("\u{1b} = 1", Some("1:1: expected identifier, string, integer, or bool, got unexpected character '\\x1b'")),
    ("\u{7f} = 1", Some("1:1: expected identifier, string, integer, or bool, got unexpected character '\\x7f'")),
    ("\u{3bc} = 1", Some("1:1: expected identifier, string, integer, or bool, got unexpected character '\u{3bc}'")),
    ("\u{e9} = 1", Some("1:1: expected identifier, string, integer, or bool, got unexpected character '\u{e9}'")),
    ("\u{301} = 1", Some("1:1: expected identifier, string, integer, or bool, got unexpected character '\u{301}'")),
    ("\u{a0} = 1", Some("1:1: expected identifier, string, integer, or bool, got unexpected character '\\u00a0'")),
    ("\u{200b} = 1", Some("1:1: expected identifier, string, integer, or bool, got unexpected character '\\u200b'")),
    ("\u{2028} = 1", Some("1:1: expected identifier, string, integer, or bool, got unexpected character '\\u2028'")),
    ("\u{feff} = 1", Some("1:1: expected identifier, string, integer, or bool, got unexpected character '\\ufeff'")),
    ("\u{e000} = 1", Some("1:1: expected identifier, string, integer, or bool, got unexpected character '\\ue000'")),
    ("\u{1f600} = 1", Some("1:1: expected identifier, string, integer, or bool, got unexpected character '\u{1f600}'")),
    ("\u{e0001} = 1", Some("1:1: expected identifier, string, integer, or bool, got unexpected character '\\U000e0001'")),
    ("\u{3000} = 1", Some("1:1: expected identifier, string, integer, or bool, got unexpected character '\\u3000'")),
    ("\u{4e2d} = 1", Some("1:1: expected identifier, string, integer, or bool, got unexpected character '\u{4e2d}'")),
    ("\u{ad} = 1", Some("1:1: expected identifier, string, integer, or bool, got unexpected character '\\u00ad'")),
    ("\u{378} = 1", Some("1:1: expected identifier, string, integer, or bool, got unexpected character '\\u0378'")),
    ("\u{10ffff} = 1", Some("1:1: expected identifier, string, integer, or bool, got unexpected character '\\U0010ffff'")),
    ("\u{85} = 1", Some("1:1: expected identifier, string, integer, or bool, got unexpected character '\\u0085'")),
    ("\u{61c} = 1", Some("1:1: expected identifier, string, integer, or bool, got unexpected character '\\u061c'")),
    ("\u{1f3fb} = 1", Some("1:1: expected identifier, string, integer, or bool, got unexpected character '\u{1f3fb}'")),
    ("x = +", Some("1:5: \"+\" is valid only in \"+inf\"")),
    ("x = +1", Some("1:5: \"+\" is valid only in \"+inf\"")),
    ("x = -", Some("1:5: \"-\" must be followed by a digit or \"inf\"")),
    ("x = -a", Some("1:5: \"-\" must be followed by a digit or \"inf\"")),
    ("@", Some("1:1: expected identifier, string, integer, or bool, got \"@\" must be followed by a directive name")),
    ("@ = 1", Some("1:1: expected identifier, string, integer, or bool, got \"@\" must be followed by a directive name")),
    ("x = @", Some("1:5: \"@\" must be followed by a directive name")),
    ("x = \"a\nb\"", Some("1:5: newline in string literal: write it as \\n, or use a \"\"\"triple-quoted\"\"\" string")),
    ("x = \"a\r\nb\"", Some("1:5: newline in string literal: write it as \\n, or use a \"\"\"triple-quoted\"\"\" string")),
    ("x = /", Some("1:5: unexpected character '/'")),
    ("/", Some("1:1: expected identifier, string, integer, or bool, got unexpected character '/'")),
    ("x = 1 /", Some("1:7: expected identifier, string, integer, or bool, got unexpected character '/'")),
    ("x = \"\\q\"", Some("1:5: unknown escape sequence \\q")),
    ("x = b\"abc", Some("1:5: unterminated bytes literal")),
    ("x = b", None),
    ("x = 5seconds", Some("1:5: invalid duration: 5seconds")),
];

#[test]
fn an_illegal_token_is_named_as_protowire_go_names_it() {
    let mut checked = 0;
    for &(doc, want) in ROWS {
        let got = parse(doc).err().map(|e| e.to_string());
        assert_eq!(got.as_deref(), want, "parse {doc:?}");
        checked += 1;
    }
    assert_eq!(checked, 55);
}
