// SPDX-License-Identifier: MIT
// Copyright (c) 2026 TrendVidia, LLC.
//! Draft §3.4.4: streaming and materializing APIs MUST produce identical
//! row sequences for the same input. The stream yields exactly `parse`'s
//! rows, with `parse`'s positions and comments, or fails with exactly
//! `parse`'s error, however the input is cut into reads (#117). A port of
//! protowire-go v1.17.0's `dataset_stream_parity_test.go`, with its fuzz
//! seeds run through `check_stream_agrees`, and of
//! `TestDatasetReader_RowComments`.

use std::io::{Cursor, Read};

use protowire_pxf::{parse, parse_with_limits, DatasetReader, DatasetRow, Limits, PxfError};

/// A reader that returns one byte per read, as Go's `iotest.OneByteReader`.
struct OneByte<R>(R);

impl<R: Read> Read for OneByte<R> {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        if buf.is_empty() {
            return Ok(0);
        }
        self.0.read(&mut buf[..1])
    }
}

type Src = Box<dyn Read>;

fn whole(input: &str) -> Src {
    Box::new(Cursor::new(input.as_bytes().to_vec()))
}

fn one_byte(input: &str) -> Src {
    Box::new(OneByte(Cursor::new(input.as_bytes().to_vec())))
}

/// For every offset k inside `input`, a reader whose first read ends at k.
fn split_readers(input: &str) -> Vec<(String, Src)> {
    let b = input.as_bytes();
    (1..b.len())
        .map(|k| {
            let src: Src =
                Box::new(Cursor::new(b[..k].to_vec()).chain(Cursor::new(b[k..].to_vec())));
            (format!("split at {k}"), src)
        })
        .collect()
}

fn readers(input: &str) -> Vec<(String, Src)> {
    let mut rs = vec![
        ("whole".to_string(), whole(input)),
        ("one byte".to_string(), one_byte(input)),
    ];
    rs.extend(split_readers(input));
    rs
}

fn read_rows<R: Read>(tr: &mut DatasetReader<R>) -> Result<Vec<DatasetRow>, PxfError> {
    let mut rows = Vec::new();
    while let Some(row) = tr.next_row() {
        rows.push(row?);
    }
    Ok(rows)
}

fn read_all_rows(src: Src, limits: Limits) -> Result<Vec<DatasetRow>, PxfError> {
    let mut tr = DatasetReader::with_limits(src, limits)?;
    read_rows(&mut tr)
}

#[test]
fn agrees_with_parse() {
    const HEAD: &str = "@dataset T (min_bpm, max_bpm)\n(60, 90)\n";
    let h = |s: &str| format!("{HEAD}{s}");
    let cases: Vec<(&str, String, bool)> = vec![
        ("A row cut mid-number then valid rows", h("(80, 1\n(90, 120)\n(95, 125)\n"), true),
        ("B row cut at end of input", h("(80, 1"), true),
        ("C non-row content between rows", h("xyz\n(90, 120)\n"), true),
        ("D wrong arity", h("(90)\n(95, 125)\n"), true),
        ("open paren only", h("("), true),
        ("cut after a comma", h("(80,"), true),
        ("cut inside a string cell", h("(\"ab"), true),
        ("string between rows", h("\"s\"\n(90, 120)\n"), true),
        ("block value between rows", h("{ }\n"), true),
        ("unterminated block comment at end", h("/* never closed"), true),
        ("unclosed list cell", h("([1, 2)\n(3, 4)\n"), true),
        ("unclosed block cell", h("({ a = 1, 2)\n"), true),
        ("nested paren in a cell", h("((1), 2)\n"), true),
        ("cut bytes literal", h("(b\"QQ\nQQ\", 1)\n"), true),
        ("bare @ at end", h("@"), true),
        ("bare @ between rows", h("@\n(90, 120)\n"), true),
        ("@ then a space", h("@ x\n"), true),
        ("cut bytes literal in a leading entry", "x = b\"QQ\nQQ\"\n@dataset T (a, b)\n(1, 2)\n".into(), true),
        ("cut bytes literal in a leading directive", "@x b\"\n@dataset T (a, b)\n".into(), true),
        ("character no token starts with", h("(~, 1)\n"), true),
        ("@proto body cut by a newline in a string", "@proto { string a = 1 [default = \"x\n\"]; }\n@dataset T (a, b)\n(1, 2)\n".into(), true),
        ("@proto body never closed", "@proto { string a = 1;\n@dataset T (a, b)\n(1, 2)\n".into(), true),
        ("clean", h("(90, 120)\n"), false),
        ("no rows", "@dataset T (a, b)\n".into(), false),
        ("comments and blank lines", h("# c\n\n// d\n/* e\n */ (90, 120) # f\n"), false),
        ("comment inside a row", h("(90, /* x */ 120)\n"), false),
        ("empty and null cells", h("(, null)\n(1,)\n"), false),
        ("strings with parens and commas", "@dataset T (a, b)\n(\"(,)\", \"#\")\n".into(), false),
        ("multi-line triple string", "@dataset T (a, b)\n(\"\"\"\nx)\n\"\"\", 1)\n".into(), false),
        ("list cell", h("([1], 2)\n"), false),
        ("block and map cells", "@dataset T (a, b)\n({ name = \"x)\" n = [1, 2] }, { \"k,\": \"v)\" })\n(, [])\n".into(), false),
        ("multi-line block cell", "@dataset T (a, b)\n({\n  name = \"x\"  # c)\n}, 1)\n".into(), false),
        // protowire-go accepts this; Rust's parse refuses it (#153). The
        // stream must still agree with parse.
        ("duration with a fraction after a unit", h("(1h.5m, 1)\n"), true),
        ("micro sign duration", h("(1\u{b5}s, 1)\n"), false),
        ("-inf", h("(-inf, 1)\n"), false),
        ("timestamp ended by a comment", h("(2026-10-23T00:00:00Z// c\n, 1)\n"), false),
        ("string escapes", h("(\"\\x41\\101\\u00e9\\U0001F600\\\\\", 1)\n"), false),
        ("@proto body", "@proto { string a = 1; }\n@dataset T (a, b)\n(1, 2)\n".into(), false),
        ("named @proto body", "@proto T { string a = 1; map<string, int32> m = 2; }\n@dataset T (a, b)\n(1, 2)\n".into(), false),
        ("@proto name and body on later lines", "@proto\n# c\nT\n{ string a = 1; // )\n}\n@dataset T (a, b)\n(1, 2)\n".into(), false),
        ("@proto body with literals and comments", "@proto { string a = 1 [default = \"})\"]; /* } */ # }\n bytes b = 2 [default = b\"}\"]; }\n@dataset T (a, b)\n(1, 2)\n".into(), false),
        ("@proto source form", "@proto \"\"\"\nsyntax = \"proto3\";\n\"\"\"\n@dataset T (a, b)\n(1, 2)\n".into(), false),
    ];
    let mut failures = Vec::new();
    for (name, input, want_err) in &cases {
        let parsed = parse(input);
        assert_eq!(parsed.is_err(), *want_err, "{name}: parse gave {parsed:?}");
        for (rname, src) in readers(input) {
            let got = read_all_rows(src, Limits::default());
            let ok = match (&parsed, &got) {
                (Err(pe), Err(e)) => pe.to_string() == e.to_string(),
                (Ok(doc), Ok(rows)) => &doc.datasets[0].rows == rows,
                _ => false,
            };
            if !ok {
                failures.push(format!(
                    "{name} / {rname}:\n  parse  {:?}\n  stream {:?}",
                    parsed
                        .as_ref()
                        .map(|d| &d.datasets[0].rows)
                        .map_err(|e| e.to_string()),
                    got.as_ref().map_err(|e| e.to_string())
                ));
                break; // one reader per case is enough to report
            }
        }
    }
    assert!(
        failures.is_empty(),
        "{} of {} cases differ:\n{}",
        failures.len(),
        cases.len(),
        failures.join("\n")
    );
}

/// The stream reads the first table `parse` reads, or fails with `parse`'s
/// error, read one byte at a time and split in two at every offset. It may
/// stop early only at a directive.
fn check_stream_agrees(input: &str) -> Result<(), String> {
    let parsed = parse(input);
    let mut rs = vec![("one byte".to_string(), one_byte(input))];
    if input.len() <= 512 {
        rs.extend(split_readers(input));
    }
    for (rname, src) in rs {
        let mut tr = match DatasetReader::new(src) {
            Ok(tr) => tr,
            Err(e) => match &parsed {
                Err(pe) if pe.to_string() == e.to_string() => continue,
                _ => {
                    return Err(format!(
                        "{rname}: header error {e}, parse {:?}",
                        parsed.as_ref().err().map(|e| e.to_string())
                    ))
                }
            },
        };
        if let Ok(doc) = &parsed {
            if tr.type_name() != doc.datasets[0].r#type || tr.columns() != doc.datasets[0].columns {
                return Err(format!(
                    "{rname}: header {} {:?}",
                    tr.type_name(),
                    tr.columns()
                ));
            }
        }
        match read_rows(&mut tr) {
            Err(e) => match &parsed {
                Err(pe) if pe.to_string() == e.to_string() => continue,
                _ => {
                    return Err(format!(
                        "{rname}: stream error {e}, parse {:?}",
                        parsed.as_ref().err().map(|e| e.to_string())
                    ))
                }
            },
            Ok(rows) => {
                let mut rest = Vec::new();
                tr.tail().read_to_end(&mut rest).unwrap();
                let rest = String::from_utf8_lossy(&rest).to_string();
                let stop = input.len() - rest.len();
                if !rest.is_empty() && !starts_with_directive(&rest) {
                    return Err(format!(
                        "{rname}: stream ended at {rest:?}, not at a directive"
                    ));
                }
                match &parsed {
                    Err(pe) => {
                        if pe.pos.offset < stop {
                            return Err(format!("{rname}: stream ended cleanly at {stop}, parse failed before it: {pe}"));
                        }
                    }
                    Ok(doc) => {
                        if doc.datasets[0].rows != rows {
                            return Err(format!(
                                "{rname}: rows differ:\n  parse  {:?}\n  stream {:?}",
                                doc.datasets[0].rows, rows
                            ));
                        }
                    }
                }
            }
        }
    }
    Ok(())
}

/// Whether `s` starts with a directive token after newlines and comments.
fn starts_with_directive(s: &str) -> bool {
    let mut t = s;
    loop {
        let trimmed = t.trim_start();
        if let Some(r) = trimmed
            .strip_prefix('#')
            .or_else(|| trimmed.strip_prefix("//"))
        {
            t = r.split_once('\n').map(|(_, r)| r).unwrap_or("");
        } else if let Some(r) = trimmed.strip_prefix("/*") {
            t = r.split_once("*/").map(|(_, r)| r).unwrap_or("");
        } else {
            let mut c = trimmed.chars();
            return c.next() == Some('@')
                && c.next()
                    .is_some_and(|ch| ch.is_ascii_alphanumeric() || ch == '_' || ch == '.');
        }
    }
}

#[test]
fn row_seeds_agree() {
    let seeds = [
        "(1, 2)\n(3, 4)\n",
        "(80, 1\n(90, 120)\n",
        "(80, 1",
        "xyz\n(1, 2)\n",
        "(1)\n",
        "(\"a)\", b\"QQ==\")\n# c\n(, null)\n",
        "(\"\"\"\n)\n\"\"\", 1)\n/* x */",
        "(1, 2)\n@dataset U (z)\n(9)\n",
        "(1, [2])\n",
        "(1, {})\n",
        "(\"x\ny\", 1)\n",
        "(1, 2)\n@",
        "(1, 2)\n@ x\n",
        "(1h.5m, 1)\n",
        "(1\u{b5}s, 1)\n",
        "(-inf, 2)\n",
        "(~, 1)\n",
        "# c\n(1, 2)\n# d\n(3, 4)\n",
        "(1, # c\n2)\n(3, 4) # d\n",
        // TestDatasetReader_RowComments (format_comments_test.go).
        "# first\n(1, 2)\n# between\n/* b */\n(3, 4)\n# end\n",
        "(1, # one\n 2)\n(3, 4) # after\n(5, 6)\n",
        "(1, 2)\n# about U\n@dataset U (z)\n(9)\n",
    ];
    let errs: Vec<String> = seeds
        .iter()
        .filter_map(|b| {
            check_stream_agrees(&format!("@dataset T (a, b)\n{b}"))
                .err()
                .map(|e| format!("{b:?}: {e}"))
        })
        .collect();
    assert!(
        errs.is_empty(),
        "{} of {} seeds differ:\n{}",
        errs.len(),
        seeds.len(),
        errs.join("\n")
    );
}

#[test]
fn header_seeds_agree() {
    let seeds = [
        "",
        "# c",
        "@type T",
        "@x { k = 1 }",
        "@x a b { k = [1, 2] }",
        "@proto { string a = 1; }",
        "@proto T { string a = 1; }",
        "@proto\nT\n{ x }",
        "@proto { \"}\" }",
        "@proto { /* } */ }",
        "@proto { b\"}\" }",
        "@proto { a\n",
        "@proto \"\"\"syntax\"\"\"",
        "x = 1h.5m",
        "x = 1\u{b5}s",
        "@dataset X (q)\n(1)",
    ];
    let errs: Vec<String> = seeds
        .iter()
        .filter_map(|h| {
            check_stream_agrees(&format!("{h}\n@dataset T (a, b)\n(1, 2)\n"))
                .err()
                .map(|e| format!("{h:?}: {e}"))
        })
        .collect();
    assert!(
        errs.is_empty(),
        "{} of {} seeds differ:\n{}",
        errs.len(),
        seeds.len(),
        errs.join("\n")
    );
}

#[test]
fn honours_limits() {
    const HEAD: &str = "@dataset T (a)\n";
    let limits = Limits {
        max_bytes_literal_length: 4,
        max_nesting_depth: 2,
        ..Limits::default()
    };
    for (name, input, want_err) in [
        (
            "bytes literal in a row over the limit",
            format!("{HEAD}(b\"QUFBQUFBQUFBQUFB\")\n"),
            true,
        ),
        (
            "bytes literal in a row within the limit",
            format!("{HEAD}(b\"QUFB\")\n"),
            false,
        ),
        (
            "bytes literal in a leading directive over the limit",
            format!("@x {{ k = b\"QUFBQUFBQUFBQUFB\" }}\n{HEAD}"),
            true,
        ),
        (
            "nesting in a leading directive over the limit",
            format!("@x {{ a {{ b {{ c {{}} }} }} }}\n{HEAD}"),
            true,
        ),
    ] {
        let parsed = parse_with_limits(&input, limits);
        assert_eq!(parsed.is_err(), want_err, "{name}: {parsed:?}");
        let got = read_all_rows(whole(&input), limits);
        match (&parsed, &got) {
            (Err(pe), Err(e)) => assert_eq!(pe.to_string(), e.to_string(), "{name}"),
            (Ok(doc), Ok(rows)) => assert_eq!(&doc.datasets[0].rows, rows, "{name}"),
            _ => panic!("{name}: parse {parsed:?}, stream {got:?}"),
        }
    }
}

#[test]
fn directive_ends_rows_and_tail_reads_the_next_table() {
    let input = "@dataset A (x)\n(1)\n(2)\n# between\n@dataset B (y)\n(3)\n";
    let mut tr = DatasetReader::new(whole(input)).unwrap();
    for _ in 0..2 {
        tr.next_row().unwrap().unwrap();
    }
    assert!(tr.next_row().is_none());
    let mut tr2 = DatasetReader::from_tail(tr.tail()).unwrap();
    assert_eq!(tr2.type_name(), "B");
    let row = tr2.next_row().unwrap().unwrap();
    assert!(matches!(&row.cells[0], Some(protowire_pxf::Value::Int(i)) if i.raw == "3"));
    assert!(tr2.next_row().is_none());
}

#[test]
fn tail_keeps_directive_comments() {
    for input in [
        "@dataset A (x)\n(1) # a\n# b\n@x {}\n@dataset B (y)\n(3)\n",
        "@dataset A (x)\n(1)\n/* block\n */ // line\n@x {}\n@y { k = 1 }\n@dataset B (y)\n(3)\n",
        "@dataset A (x)\n(1)\n\n# only before the dataset\n@dataset B (y)\n(3)\n",
        "@dataset A (x)\n# no rows\n@x {}\n@dataset B (y)\n",
    ] {
        let want = parse(input).unwrap().directives;
        for (rname, src) in [("whole", whole(input)), ("one byte", one_byte(input))] {
            let mut tr = DatasetReader::new(src).unwrap();
            while let Some(r) = tr.next_row() {
                r.unwrap();
            }
            let tr2 = DatasetReader::from_tail(tr.tail())
                .unwrap_or_else(|e| panic!("{input:?} {rname}: {e}"));
            assert_eq!(tr2.type_name(), "B");
            assert_eq!(tr2.directives(), &want[..], "{input:?} {rname}");
        }
    }
}

/// A reader chained on a tail reports the next table's rows and errors at
/// their document positions, as `parse` does.
#[test]
fn tail_keeps_document_positions() {
    const TWO: &str =
        "@dataset A (x)\n(1)\n\n@x {}\n@proto { message M {} }\n@dataset B (y)\n(3)\n";
    for (name, input) in [("rows", TWO.to_string()), ("error", format!("{TWO}(4\n"))] {
        let parsed = parse(&input);
        for (rname, src) in [("whole", whole(&input)), ("one byte", one_byte(&input))] {
            let mut tr = DatasetReader::new(src).unwrap();
            while let Some(r) = tr.next_row() {
                r.unwrap();
            }
            let mut tr2 = DatasetReader::from_tail(tr.tail()).unwrap();
            let rows = read_rows(&mut tr2);
            match (&parsed, rows) {
                (Err(pe), Err(e)) => assert_eq!(pe.to_string(), e.to_string(), "{name} {rname}"),
                (Ok(doc), Ok(rows)) => {
                    assert_eq!(doc.datasets[1].rows, rows, "{name} {rname}");
                    assert_eq!(doc.directives, tr2.directives(), "{name} {rname}");
                    assert_eq!(doc.datasets[1].columns, tr2.columns(), "{name} {rname}");
                }
                (p, r) => panic!("{name} {rname}: parse {p:?}, stream {r:?}"),
            }
        }
    }
}

/// An error is sticky: the rows after a malformed one are never yielded.
#[test]
fn error_is_sticky() {
    let mut tr = DatasetReader::new(whole("@dataset T (a, b)\n(1, 2\n(3, 4)\n")).unwrap();
    let e1 = tr.next_row().unwrap().unwrap_err();
    let e2 = tr.next_row().unwrap().unwrap_err();
    assert_eq!(e1, e2);
}
