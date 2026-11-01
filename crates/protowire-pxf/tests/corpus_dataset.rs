// SPDX-License-Identifier: MIT
// Copyright (c) 2026 TrendVidia, LLC.
//! `@dataset` value cells, column paths and the end of a row sequence
//! (draft -01 §3.4.4; protowire#341, #342, #336). `testdata/dataset/` is
//! vendored from trendvidia/protowire at e9e0a17.

mod common;

use common::{fixtures, message, pool};
use prost_reflect::{DescriptorPool, DynamicMessage, MessageDescriptor, ReflectMessage};
use protowire_pxf::{
    bind_row, parse, unmarshal, unmarshal_full, DatasetDirective, DatasetReader, UnmarshalOptions,
};

const TEST: &[u8] = include_bytes!("../testdata/test.binpb");
const PATHS: &[u8] = include_bytes!("../testdata/dataset/paths.binpb");

/// Every fixture: `""` must be accepted, otherwise a substring its rejection
/// must contain, so a fixture can't pass by failing for some other reason.
/// A fixture missing from this table fails the test.
const VERDICTS: &[(&str, &str)] = &[
    ("empty-rows", ""),
    ("rows-end-at-directive", ""),
    ("list-cell", ""),
    ("block-cell", ""),
    ("nested-cells", ""),
    ("cell-framing", ""),
    ("arity-long", "3 cells, expected 2"),
    ("arity-short", "2 cells, expected 3"),
    ("junk-between-rows", "top-level field entries"),
    ("truncated-row-eof", "expected ',' or ')'"),
    ("truncated-row-then-rows", "expected ',' or ')'"),
    ("with-body", "top-level field entries"),
    ("with-type", "@type"),
    (
        "block-on-scalar",
        "expected integer for field \"int32_field\"",
    ),
    ("string-newline-in-cell", "string"),
    (
        "null-on-scalar-cell",
        "null is not allowed on scalar field \"int32_field\"",
    ),
    ("null-on-optional-cell", ""),
    ("dotted-column", ""),
    ("dotted-grouping", ""),
    ("dotted-null-leaf", ""),
    ("dotted-oneof", ""),
    ("dotted-required", ""),
    ("dotted-with-value-cells", ""),
    (
        "dotted-oneof-two-members",
        "conflicts with already-set field",
    ),
    ("dotted-through-repeated", "\"picks\" is a repeated field"),
    ("dotted-through-map", "\"by_name\" is a map field"),
    (
        "dotted-into-timestamp",
        "\"ts\" is a google.protobuf.Timestamp",
    ),
    (
        "dotted-into-wrapper",
        "\"wrap\" is a google.protobuf.Int32Value",
    ),
    (
        "dotted-overlap",
        "column \"desc.energy\" overlaps column \"desc\"",
    ),
    ("duplicate-column", "column \"string_field\" appears twice"),
    (
        "dotted-required-missing",
        "required field \"req.n\" is absent",
    ),
    ("dotted-empty-segment", "a column path has an empty segment"),
    ("dotted-trailing-dot", "a column path has an empty segment"),
    ("dotted-unknown-segment", "has no field \"nope\""),
    ("dotted-through-scalar", "\"plain\" is not a message field"),
    (
        "dotted-into-duration",
        "\"dur\" is a google.protobuf.Duration",
    ),
    ("dotted-into-bignum", "\"big\" is a pxf.BigInt"),
    (
        "dotted-overlap-deep",
        "column \"desc.tempo.min_bpm\" overlaps column \"desc.tempo\"",
    ),
    (
        "dotted-bad-header-empty-row",
        "\"picks\" is a repeated field",
    ),
    ("dotted-into-secret", ""),
    ("dotted-into-any", ""),
    ("dotted-bad-header-no-rows", ""),
];

fn row_type(doc: &str, test: &DescriptorPool, paths: &DescriptorPool) -> MessageDescriptor {
    let name = doc
        .lines()
        .find_map(|l| l.strip_prefix("@dataset "))
        .and_then(|l| l.split_whitespace().next())
        .expect("a @dataset header");
    test.get_message_by_name(name)
        .or_else(|| paths.get_message_by_name(name))
        .unwrap_or_else(|| panic!("no row type {name}"))
}

fn bind_all(ds: &[DatasetDirective], d: &MessageDescriptor) -> Result<Vec<DynamicMessage>, String> {
    let mut out = Vec::new();
    for t in ds {
        for r in &t.rows {
            let m = bind_row(d, &t.columns, r, UnmarshalOptions::default())
                .map_err(|e| e.to_string())?;
            out.push(m);
        }
    }
    Ok(out)
}

fn via_parse(doc: &str, d: &MessageDescriptor) -> Result<Vec<DynamicMessage>, String> {
    bind_all(&parse(doc).map_err(|e| e.to_string())?.datasets, d)
}

fn via_full(doc: &str, d: &MessageDescriptor) -> Result<Vec<DynamicMessage>, String> {
    let (_, p) = unmarshal_full(doc, d, UnmarshalOptions::default()).map_err(|e| e.to_string())?;
    bind_all(p.datasets(), d)
}

/// Streams every dataset in the document, chaining a new reader on the
/// tail of the last, as a document with several datasets requires.
fn via_stream(doc: &str, d: &MessageDescriptor) -> Result<Vec<DynamicMessage>, String> {
    let mut out = Vec::new();
    let mut readers = 0;
    let mut src: Box<dyn std::io::Read> = Box::new(std::io::Cursor::new(doc.to_string()));
    loop {
        let mut r = match DatasetReader::new(src) {
            Ok(r) => r,
            Err(e) if e.to_string().contains("no @dataset directive") && readers > 0 => {
                return Ok(out);
            }
            Err(e) => return Err(e.to_string()),
        };
        readers += 1;
        while let Some(m) = r
            .scan_one(d, UnmarshalOptions::default())
            .map_err(|e| e.to_string())?
        {
            out.push(m);
        }
        src = Box::new(r.tail());
    }
}

#[test]
fn every_fixture_meets_its_verdict_on_every_path() {
    let (test, paths) = (pool(TEST), pool(PATHS));
    let files = fixtures("dataset");
    assert_eq!(files.len(), VERDICTS.len(), "a fixture has no verdict");
    let mut problems = Vec::new();
    for (stem, doc) in &files {
        let want = VERDICTS
            .iter()
            .find(|(s, _)| s == stem)
            .unwrap_or_else(|| panic!("{stem} has no verdict"))
            .1;
        let d = row_type(doc, &test, &paths);
        let results = [
            ("parse", via_parse(doc, &d)),
            ("full", via_full(doc, &d)),
            ("stream", via_stream(doc, &d)),
        ];
        for (path, got) in &results {
            match (got, want) {
                (Ok(_), "") => {}
                (Ok(_), w) => problems.push(format!("{stem}/{path}: accepted, want {w:?}")),
                (Err(e), "") => problems.push(format!("{stem}/{path}: {e}")),
                (Err(e), w) if !e.contains(w) => {
                    problems.push(format!("{stem}/{path}: {e}, want {w:?}"))
                }
                _ => {}
            }
        }
        if want.is_empty() {
            let [(_, a), (_, b), (_, c)] = &results;
            assert_eq!(a, b, "{stem}: parse and full disagree");
            assert_eq!(a, c, "{stem}: the stream's rows differ");
        }
    }
    assert!(problems.is_empty(), "\n{}", problems.join("\n"));
}

/// The header rules that need no schema reject at `parse`, at the column.
#[test]
fn header_rules_are_decided_without_a_schema_at_the_offending_column() {
    let fixture = |stem: &str| {
        fixtures("dataset")
            .into_iter()
            .find(|(s, _)| s == stem)
            .unwrap()
            .1
    };
    for stem in [
        "duplicate-column",
        "dotted-overlap",
        "dotted-overlap-deep",
        "dotted-empty-segment",
        "dotted-trailing-dot",
    ] {
        assert!(parse(&fixture(stem)).is_err(), "{stem}");
        let line = fixture(stem)
            .lines()
            .position(|l| l.starts_with("@dataset"))
            .unwrap()
            + 1;
        assert_eq!(parse(&fixture(stem)).unwrap_err().pos.line, line, "{stem}");
    }
    let e = parse("@dataset t.T (a, b,\n  a)").unwrap_err();
    assert_eq!((e.pos.line, e.pos.column), (2, 3));
    // A prefix of the name, not of the path, is no overlap.
    parse("@dataset t.T (ab, a.b, a.c, abc.d)").unwrap();
}

/// A header is met before any limit but MaxMessageSize, so its check must
/// not be quadratic in the number of columns or in one path's segments.
#[test]
fn a_huge_header_is_checked_in_linear_time() {
    let many: Vec<String> = (0..200_000).map(|i| format!("c{i}")).collect();
    let doc = format!("@dataset t.T ({})", many.join(","));
    let start = std::time::Instant::now();
    parse(&doc).unwrap();
    let deep = format!("@dataset t.T ({})", vec!["a"; 500_000].join("."));
    parse(&deep).unwrap();
    assert!(start.elapsed().as_secs() < 10, "{:?}", start.elapsed());
}

fn event(doc: &str) -> DynamicMessage {
    let d = message(&pool(PATHS), "dataset.v1.Event");
    via_parse(doc, &d).unwrap().remove(0)
}

#[test]
fn cells_sharing_a_prefix_land_in_one_block() {
    let m = event(
        "@dataset dataset.v1.Event (desc.energy, id, desc.tempo.min_bpm, desc.tempo.max_bpm)\n(0.5, \"x\", 60, 90)\n",
    );
    let desc = m.get_field_by_name("desc").unwrap();
    let desc = desc.as_message().unwrap();
    assert_eq!(
        desc.get_field_by_name("energy").unwrap().as_f64(),
        Some(0.5)
    );
    let tempo = desc.get_field_by_name("tempo").unwrap();
    let tempo = tempo.as_message().unwrap();
    assert_eq!(
        tempo.get_field_by_name("min_bpm").unwrap().as_i32(),
        Some(60)
    );
    assert_eq!(
        tempo.get_field_by_name("max_bpm").unwrap().as_i32(),
        Some(90)
    );
}

#[test]
fn a_prefix_with_no_cell_stays_absent() {
    let m = event("@dataset dataset.v1.Event (id, desc.energy, desc.tempo.min_bpm)\n(\"x\", , )\n");
    let d = m.descriptor();
    assert!(!m.has_field(&d.get_field_by_name("desc").unwrap()));
}

#[test]
fn value_cells_bind_as_their_body_entry() {
    let d = message(&pool(TEST), "test.v1.AllTypes");
    let doc = "@dataset test.v1.AllTypes (nested_field, repeated_string, string_field)\n({ name = \"n\" }, [\"a\", \"b\"], \"s\")\n";
    let bound = via_parse(doc, &d).unwrap().remove(0);
    let body =
        "nested_field = { name = \"n\" }\nrepeated_string = [\"a\", \"b\"]\nstring_field = \"s\"\n";
    assert_eq!(
        bound,
        unmarshal(body, &d, UnmarshalOptions::default()).unwrap()
    );

    let doc = "@dataset test.v1.AllTypes (nested_field, string_field)\n({}, )\n";
    let bound = via_parse(doc, &d).unwrap().remove(0);
    assert!(bound.has_field(&d.get_field_by_name("nested_field").unwrap()));
    assert!(!bound.has_field(&d.get_field_by_name("string_field").unwrap()));
}

/// A row's body is built by recursing over a path, and a recursive row
/// type admits a path of any length, so the depth is refused before that.
#[test]
fn a_path_past_the_nesting_limit_fails_cleanly() {
    let fds = include_bytes!("../testdata/hardening-test.binpb");
    let d = message(&pool(fds), "hardening_test.v1.Tree");
    let path = format!("{}label", "child.".repeat(200_000));
    let doc = format!("@dataset hardening_test.v1.Tree ({path})\n(\"x\")\n");
    let e = via_parse(&doc, &d).unwrap_err();
    assert!(e.contains("MaxNestingDepth"), "{e}");
}

/// A cell's error names its column and is reported at the row.
#[test]
fn a_bind_error_names_the_column() {
    let d = message(&pool(TEST), "test.v1.AllTypes");
    let doc =
        "@dataset test.v1.AllTypes (string_field, int32_field)\n(\"a\", 1)\n(\"b\", \"nope\")\n";
    let e = via_parse(doc, &d).unwrap_err();
    assert!(e.starts_with("3:1: column int32_field: "), "{e}");
}
