// SPDX-License-Identifier: MIT
// Copyright (c) 2026 TrendVidia, LLC.
//! Where `null` may bind and where a simple string may break a line
//! (draft -01 § Booleans, Null, and Identifier Values and § String
//! Literals; protowire#339, #379, #380, #350). `testdata/nulls/`,
//! `testdata/strings/` and `testdata/dataset/` are vendored from
//! trendvidia/protowire at e9e0a17.

mod common;

use common::{declared_type, fixtures, message, must_reject, pool};
use prost_reflect::{MessageDescriptor, ReflectMessage};
use protowire_pxf::{bind_row, parse, unmarshal, unmarshal_full, DatasetReader, UnmarshalOptions};

const NULLS: &[u8] = include_bytes!("../testdata/nulls/nulls.binpb");
const NULLS_P2: &[u8] = include_bytes!("../testdata/nulls/nulls_proto2.binpb");
const NULLS_ED: &[u8] = include_bytes!("../testdata/nulls/nulls_editions.binpb");
const STRINGS: &[u8] = include_bytes!("../testdata/strings/strings.binpb");
const TEST: &[u8] = include_bytes!("../testdata/test.binpb");
const PATHS: &[u8] = include_bytes!("../testdata/dataset/paths.binpb");

fn desc(name: &str) -> MessageDescriptor {
    let source = if name == "nulls.v1.Legacy" || name == "nulls.v1.Strict" {
        NULLS_P2
    } else if name.starts_with("nulls.ed.") {
        NULLS_ED
    } else {
        NULLS
    };
    message(&pool(source), name)
}

#[test]
fn every_null_fixture_meets_its_verdict() {
    let files = fixtures("nulls");
    assert_eq!(files.len(), 19, "the README lists nineteen fixtures");
    let mut ran = 0;
    for (stem, doc) in files {
        ran += 1;
        parse(&doc).unwrap_or_else(|e| panic!("{stem}: parse accepts every fixture: {e}"));
        let d = desc(declared_type(&doc).unwrap_or("nulls.v1.Holder"));
        let opts = UnmarshalOptions::default();
        if must_reject(&stem) {
            assert!(unmarshal(&doc, &d, opts).is_err(), "{stem}: unmarshal");
            assert!(unmarshal_full(&doc, &d, opts).is_err(), "{stem}: full");
            continue;
        }
        let msg = unmarshal(&doc, &d, opts).unwrap_or_else(|e| panic!("{stem}: {e}"));
        assert_eq!(msg.fields().count(), 0, "{stem}: leaves no field set");
        let (msg, presence) =
            unmarshal_full(&doc, &d, opts).unwrap_or_else(|e| panic!("{stem}: full: {e}"));
        assert_eq!(msg.fields().count(), 0, "{stem}: a default must not apply");
        assert_eq!(presence.null_paths().count(), 1, "{stem}: present-but-null");
    }
    assert_eq!(ran, 19);
}

#[test]
fn a_null_error_names_the_field() {
    let d = desc("nulls.v1.Holder");
    let opts = UnmarshalOptions::default();
    for (doc, want) in [
        ("n = null", "null is not allowed on scalar field \"n\""),
        ("l = null", "null is not allowed on repeated field \"l\""),
        ("m = null", "null is not allowed on map field \"m\""),
    ] {
        let e = unmarshal(doc, &d, opts).expect_err(doc);
        assert!(e.msg.contains(want), "{doc}: {}", e.msg);
    }
}

#[test]
fn every_string_fixture_meets_its_verdict() {
    let d = message(&pool(STRINGS), "strings.v1.Holder");
    let files = fixtures("strings");
    assert_eq!(files.len(), 4, "the README lists four fixtures");
    for (stem, doc) in files {
        let opts = UnmarshalOptions::default();
        if must_reject(&stem) {
            assert!(parse(&doc).is_err(), "{stem}: parse");
            assert!(unmarshal(&doc, &d, opts).is_err(), "{stem}: decode");
            continue;
        }
        parse(&doc).unwrap_or_else(|e| panic!("{stem}: parse: {e}"));
        let msg = unmarshal(&doc, &d, opts).unwrap_or_else(|e| panic!("{stem}: {e}"));
        let s = msg.get_field_by_name("s").unwrap();
        let want = if stem == "newline-after-string" {
            "x"
        } else {
            "x\ny"
        };
        assert_eq!(s.as_str(), Some(want), "{stem}");
    }
}

/// The verdict of a `@dataset` document on each path that can bind it:
/// parse then `bind_row`, `unmarshal_full` then `datasets()`, and the
/// streaming reader. Returns each path's success.
fn dataset_paths(doc: &str, d: &MessageDescriptor) -> [bool; 3] {
    let opts = UnmarshalOptions::default();
    let bind_all = |ds: &protowire_pxf::DatasetDirective| {
        ds.rows
            .iter()
            .all(|r| bind_row(d, &ds.columns, r, opts).is_ok())
    };
    let parsed = parse(doc).is_ok_and(|p| p.datasets.iter().all(bind_all));
    let full = unmarshal_full(doc, d, opts).is_ok_and(|(_, p)| p.datasets().iter().all(bind_all));
    let streamed = DatasetReader::new(doc.as_bytes()).is_ok_and(|mut r| loop {
        match r.scan_one(d, opts) {
            Ok(Some(_)) => {}
            Ok(None) => return true,
            Err(_) => return false,
        }
    });
    [parsed, full, streamed]
}

fn dataset_fixture(stem: &str) -> String {
    fixtures("dataset")
        .into_iter()
        .find(|(s, _)| s == stem)
        .unwrap_or_else(|| panic!("no fixture {stem}"))
        .1
}

#[test]
fn null_and_newline_rules_hold_in_a_dataset_cell() {
    let all_types = message(&pool(TEST), "test.v1.AllTypes");
    let event = message(&pool(PATHS), "dataset.v1.Event");
    let rejected = dataset_paths(&dataset_fixture("null-on-scalar-cell"), &all_types);
    assert_eq!(rejected, [false; 3], "null-on-scalar-cell");
    let rejected = dataset_paths(&dataset_fixture("string-newline-in-cell"), &all_types);
    assert_eq!(rejected, [false; 3], "string-newline-in-cell");
    let accepted = dataset_paths(&dataset_fixture("null-on-optional-cell"), &event);
    assert_eq!(accepted, [true; 3], "null-on-optional-cell");
}

/// Binding renders a cell back to text, so a line break that arrived
/// escaped has to leave escaped now that a raw one is an error.
#[test]
fn an_escaped_newline_in_a_cell_still_binds() {
    let all_types = message(&pool(TEST), "test.v1.AllTypes");
    let doc = "@dataset test.v1.AllTypes (string_field)\n(\"a\\nb\\t\\\"c\\\\\")\n";
    assert_eq!(dataset_paths(doc, &all_types), [true; 3]);
    let parsed = parse(doc).unwrap();
    let ds = &parsed.datasets[0];
    let msg = bind_row(
        &all_types,
        &ds.columns,
        &ds.rows[0],
        UnmarshalOptions::default(),
    )
    .unwrap();
    let got = msg.get_field_by_name("string_field").unwrap();
    assert_eq!(got.as_str(), Some("a\nb\t\"c\\"));
    assert_eq!(msg.descriptor().full_name(), "test.v1.AllTypes");
}
