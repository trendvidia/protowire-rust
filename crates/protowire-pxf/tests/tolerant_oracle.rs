// SPDX-License-Identifier: MIT
// Copyright (c) 2026 TrendVidia, LLC.
//! `parse_tolerant` held to protowire-go's `ParseTolerant` (#130): the same
//! errors at the same positions, and the same tree, placeholders included.
//! Each case also holds `parse` to protowire-go's `Parse`.
//!
//! `testdata/conformance/tolerant-oracle.tsv` is what protowire-go did with
//! each case of `tolerant-cases.tsv`; its header names the commit, and
//! `scripts/gen-tolerant-oracle.sh` regenerates it. The cases are every
//! document of protowire-go's `tolerant_test.go`, a document for each error
//! path of its parser and lexer, every prefix of three documents, and every
//! vendored `.pxf` fixture. A tree is compared as text, in the dump format
//! of `scripts/tolerant-oracle/main.go`, which `dump` below writes the same
//! way.

use std::fmt::Write as _;
use std::path::Path;

use protowire_pxf::ast::ProtoShape;
use protowire_pxf::{parse, parse_tolerant, Comment, DatasetRow, Document, Entry, PxfError, Value};

const ORACLE: &str = include_str!("../testdata/conformance/tolerant-oracle.tsv");

/// The oracle's escapes undone: `\\`, `\t` and `\n`.
fn unescape(s: &str) -> String {
    let mut out = String::new();
    let mut chars = s.chars();
    while let Some(c) = chars.next() {
        if c != '\\' {
            out.push(c);
            continue;
        }
        match chars.next() {
            Some('n') => out.push('\n'),
            Some('t') => out.push('\t'),
            Some(c) => out.push(c),
            None => out.push('\\'),
        }
    }
    out
}

/// Free text: printable ASCII but `\` and `"` as is, every other byte as
/// `\xHH`, in quotes.
fn q(s: &[u8]) -> String {
    let mut out = String::from("\"");
    for &c in s {
        if (0x20..0x7f).contains(&c) && c != b'\\' && c != b'"' {
            out.push(c as char);
        } else {
            write!(out, "\\x{c:02x}").unwrap();
        }
    }
    out.push('"');
    out
}

fn qs(s: &str) -> String {
    q(s.as_bytes())
}

fn pos(p: protowire_pxf::Position) -> String {
    format!("{}:{}:{}", p.line, p.column, p.offset)
}

fn bit(b: bool) -> &'static str {
    if b {
        "1"
    } else {
        "0"
    }
}

fn comments(cs: &[Comment]) -> String {
    let parts: Vec<String> = cs
        .iter()
        .map(|c| format!("c({},{})", pos(c.pos), qs(&c.text)))
        .collect();
    format!("[{}]", parts.join(","))
}

fn value(v: Option<&Value>) -> String {
    let Some(v) = v else {
        return "_".to_string();
    };
    match v {
        Value::String(v) => format!("s({},{})", pos(v.pos), qs(&v.value)),
        Value::Int(v) => format!("i({},{},{})", pos(v.pos), qs(&v.raw), qs(&v.prec)),
        Value::Float(v) => format!("f({},{},{})", pos(v.pos), qs(&v.raw), qs(&v.prec)),
        Value::Bool(v) => format!("bo({},{})", pos(v.pos), bit(v.value)),
        Value::Bytes(v) => format!("by({},{})", pos(v.pos), q(&v.value)),
        Value::Null(v) => format!("n({})", pos(v.pos)),
        Value::Ident(v) => format!("id({},{})", pos(v.pos), qs(&v.name)),
        Value::Timestamp(v) => format!("ts({},{})", pos(v.pos), qs(&v.raw)),
        Value::Duration(v) => format!("du({},{})", pos(v.pos), qs(&v.raw)),
        Value::List(v) => {
            let els: Vec<String> = v.elements.iter().map(|e| value(Some(e))).collect();
            format!("l({},[{}])", pos(v.pos), els.join(","))
        }
        Value::Block(v) => format!(
            "bl({},{},{})",
            pos(v.pos),
            entries(&v.entries),
            comments(&v.trailing_comments)
        ),
        Value::Bad(v) => format!("bad({})", pos(v.pos)),
    }
}

fn entries(es: &[Entry]) -> String {
    let parts: Vec<String> = es
        .iter()
        .map(|e| match e {
            Entry::Assignment(a) => format!(
                "a({},{},{},{},{},{})",
                pos(a.pos),
                qs(&a.key),
                bit(a.key_quoted),
                value(Some(&a.value)),
                comments(&a.leading_comments),
                qs(&a.trailing_comment)
            ),
            Entry::MapEntry(m) => format!(
                "m({},{},{},{},{},{})",
                pos(m.pos),
                qs(&m.key),
                bit(m.quoted),
                value(Some(&m.value)),
                comments(&m.leading_comments),
                qs(&m.trailing_comment)
            ),
            Entry::Block(b) => format!(
                "b({},{},{},{},{},{})",
                pos(b.pos),
                qs(&b.name),
                bit(b.name_quoted),
                entries(&b.entries),
                comments(&b.leading_comments),
                comments(&b.trailing_comments)
            ),
            Entry::TypeEntry(t) => format!(
                "ty({},{},{},{})",
                pos(t.pos),
                qs(&t.url),
                comments(&t.leading_comments),
                qs(&t.trailing_comment)
            ),
        })
        .collect();
    format!("[{}]", parts.join(","))
}

fn strs(ss: &[String]) -> String {
    let parts: Vec<String> = ss.iter().map(|s| qs(s)).collect();
    format!("[{}]", parts.join(","))
}

fn row(r: &DatasetRow) -> String {
    let cells: Vec<String> = r.cells.iter().map(|c| value(c.as_ref())).collect();
    format!(
        "r({},[{}],{})",
        pos(r.pos),
        cells.join(","),
        comments(&r.leading_comments)
    )
}

fn dump(d: &Document) -> String {
    let dirs: Vec<String> = d
        .directives
        .iter()
        .map(|x| {
            let body = if x.has_body {
                q(&x.body)
            } else {
                "-".to_string()
            };
            format!(
                "d({},{},{},{},{},{})",
                pos(x.pos),
                qs(&x.name),
                strs(&x.prefixes),
                qs(&x.r#type),
                body,
                comments(&x.leading_comments)
            )
        })
        .collect();
    let sets: Vec<String> = d
        .datasets
        .iter()
        .map(|x| {
            let rows: Vec<String> = x.rows.iter().map(row).collect();
            format!(
                "t({},{},{},[{}],{})",
                pos(x.pos),
                qs(&x.r#type),
                strs(&x.columns),
                rows.join(","),
                comments(&x.leading_comments)
            )
        })
        .collect();
    let protos: Vec<String> = d
        .protos
        .iter()
        .map(|x| {
            let shape = match x.shape {
                ProtoShape::Anonymous => "anonymous",
                ProtoShape::Named => "named",
                ProtoShape::Source => "source",
                ProtoShape::Descriptor => "descriptor",
            };
            format!(
                "p({},{},{},{},{})",
                pos(x.pos),
                shape,
                qs(&x.type_name),
                q(&x.body),
                comments(&x.leading_comments)
            )
        })
        .collect();
    format!(
        "doc({},{},{},[{}],[{}],[{}],{},{})",
        qs(&d.type_url),
        d.body_offset,
        comments(&d.leading_comments),
        dirs.join(","),
        sets.join(","),
        protos.join(","),
        entries(&d.entries),
        comments(&d.trailing_comments)
    )
}

fn errors(errs: &[PxfError]) -> String {
    if errs.is_empty() {
        return "NONE".to_string();
    }
    errs.iter()
        .map(|e| format!("{}|{}", pos(e.pos), e.msg))
        .collect::<Vec<_>>()
        .join(" ; ")
}

/// The document a case names: inline, a file under `testdata/`, or a prefix
/// of one.
fn document(case: &str) -> String {
    let testdata = Path::new(env!("CARGO_MANIFEST_DIR")).join("testdata");
    if let Some(path) = case.strip_prefix("file ") {
        return std::fs::read_to_string(testdata.join(path)).expect(path);
    }
    if let Some(rest) = case.strip_prefix("prefixes ") {
        let (path, n) = rest.rsplit_once(' ').expect(rest);
        let data = std::fs::read(testdata.join(path)).expect(path);
        let n: usize = n.parse().expect(n);
        // A prefix cut inside a multi-byte character is not a str; the
        // cases' documents are ASCII, so none is.
        return String::from_utf8(data[..n].to_vec()).expect("an ASCII document");
    }
    unescape(case)
}

/// A string whose escapes give bytes that are not UTF-8 (#170). protowire-go
/// keeps the bytes; this port's lexer refuses the string, so `parse` fails
/// and `parse_tolerant` records that error and a placeholder. Such a row is
/// checked for exactly that, and the count is pinned, so the test fails the
/// day #170 is fixed.
fn waits_on_170(case: &str, got: &str) -> bool {
    case.contains("\\\\xff")
        && (got.contains("string contains invalid UTF-8") || got.contains("bad("))
}

#[derive(Debug, Default, PartialEq)]
struct Marker {
    cases: usize,
    strict_ok: usize,
    with_errors: usize,
    errors: usize,
    placeholders: usize,
    waits_on_170: usize,
}

#[test]
fn every_case_meets_the_reference() {
    let mut seen = Marker::default();
    let mut failures = Vec::new();
    for line in ORACLE.lines() {
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let c: Vec<&str> = line.split('\t').collect();
        assert_eq!(c.len(), 4, "{line}");
        let doc = document(c[0]);
        seen.cases += 1;
        let parsed = parse(&doc);
        let (tree, errs) = parse_tolerant(&doc);
        let strict = match &parsed {
            Ok(strict_tree) => {
                seen.strict_ok += 1;
                // A document `parse` reads gives the same tree and no error.
                assert_eq!(&tree, strict_tree, "{}", c[0]);
                assert!(errs.is_empty(), "{}: {errs:?}", c[0]);
                "OK".to_string()
            }
            Err(e) => format!("ERR {e}"),
        };
        if !errs.is_empty() {
            seen.with_errors += 1;
        }
        seen.errors += errs.len();
        let tree = dump(&tree);
        seen.placeholders += tree.matches("bad(").count();
        for (what, got, want) in [
            ("parse", strict, c[1]),
            ("errors", errors(&errs), c[2]),
            ("tree", tree, c[3]),
        ] {
            let want = unescape(want);
            if got != want && waits_on_170(c[0], &got) {
                seen.waits_on_170 += 1;
                continue;
            }
            if got != want {
                failures.push(format!(
                    "{} [{what}] {:?}\n   got: {got}\n  want: {want}",
                    c[0], doc
                ));
            }
        }
    }
    assert!(
        failures.is_empty(),
        "{} differ from protowire-go:\n{}",
        failures.len(),
        failures.join("\n")
    );
    // This port's counts. The oracle file gives protowire-go's: 282 read by
    // `parse`, 469 with errors, 878 errors and 189 placeholders. The two
    // #170 rows make the difference: the first is refused by `parse`, and
    // each holds one more error and one more placeholder here.
    assert_eq!(
        seen,
        Marker {
            cases: 751,
            strict_ok: 281,
            with_errors: 470,
            errors: 880,
            placeholders: 191,
            waits_on_170: 5,
        }
    );
}

/// An input past `max_message_size` gives an empty document and that one
/// error, as protowire-go's `ParseOptions.ParseTolerant` does.
#[test]
fn an_input_past_the_size_limit_is_one_error() {
    let limits = protowire_pxf::Limits {
        max_message_size: 4,
        ..Default::default()
    };
    let (doc, errs) = protowire_pxf::parse_tolerant_with_limits("a = 1\n", limits);
    assert_eq!(doc, Document::default());
    assert_eq!(errs.len(), 1);
    assert_eq!(
        errs[0].to_string(),
        "1:1: input of 6 bytes exceeds MaxMessageSize=4"
    );
    // Within the limit, the document is read as `parse_tolerant` reads it:
    // protowire-go's verdict on `port =` in the oracle.
    let (_, errs) = protowire_pxf::parse_tolerant_with_limits("a =", limits);
    assert_eq!(errs.len(), 1, "{errs:?}");
    assert_eq!(errs[0].to_string(), "1:4: expected value, got EOF (\"\")");
}
