// SPDX-License-Identifier: MIT
// Copyright (c) 2026 TrendVidia, LLC.
//! Tests for `Presence::directives()` / `Presence::tables()` — PR 3 of
//! the v0.72-v0.75 Rust catch-up. The direct decoder populates the
//! directive vectors on Presence during `unmarshal_full`, so consumers
//! (chameleon's @header reader, table binders, etc.) can read the
//! document-root directives after a decode call.

use prost_reflect::{DescriptorPool, MessageDescriptor};
use protowire_pxf::ast::Value;
use protowire_pxf::{unmarshal, unmarshal_full, UnmarshalOptions};

const TEST_FDS: &[u8] = include_bytes!("../testdata/test.binpb");

fn pool() -> DescriptorPool {
    DescriptorPool::decode(TEST_FDS).expect("decode test.binpb")
}

fn all_types() -> MessageDescriptor {
    pool()
        .get_message_by_name("test.v1.AllTypes")
        .expect("missing test.v1.AllTypes")
}

#[test]
fn empty_document_has_empty_accessors() {
    let (_msg, p) = unmarshal_full(
        "string_field = \"x\"\n",
        &all_types(),
        UnmarshalOptions::default(),
    )
    .expect("decode");
    assert!(p.directives().is_empty());
    assert!(p.tables().is_empty());
}

#[test]
fn bare_directive_recorded() {
    let (_msg, p) = unmarshal_full(
        "@frob\nstring_field = \"x\"\n",
        &all_types(),
        UnmarshalOptions::default(),
    )
    .expect("decode");
    let dirs = p.directives();
    assert_eq!(dirs.len(), 1);
    assert_eq!(dirs[0].name, "frob");
    assert!(dirs[0].prefixes.is_empty());
    assert!(!dirs[0].has_body);
    assert!(dirs[0].r#type.is_empty());
}

#[test]
fn single_prefix_populates_legacy_type() {
    let (_msg, p) = unmarshal_full(
        "@header pkg.Hdr { id = \"h\" }\nstring_field = \"x\"\n",
        &all_types(),
        UnmarshalOptions::default(),
    )
    .expect("decode");
    let d = &p.directives()[0];
    assert_eq!(d.name, "header");
    assert_eq!(d.prefixes, vec!["pkg.Hdr"]);
    assert_eq!(d.r#type, "pkg.Hdr");
    assert!(d.has_body);
    let body = std::str::from_utf8(&d.body).unwrap();
    assert!(body.contains("id = \"h\""));
}

#[test]
fn two_prefixes_leave_legacy_type_empty() {
    let (_msg, p) = unmarshal_full(
        "@entry mylabel pkg.MsgType\nstring_field = \"x\"\n",
        &all_types(),
        UnmarshalOptions::default(),
    )
    .expect("decode");
    let d = &p.directives()[0];
    assert_eq!(d.prefixes, vec!["mylabel", "pkg.MsgType"]);
    assert_eq!(d.r#type, "");
}

#[test]
fn multiple_directives_in_source_order() {
    let src = "@header pkg.Hdr { id = \"h\" }\n\
               @frob alpha beta\n\
               @meta\n\
               string_field = \"x\"\n";
    let (_msg, p) = unmarshal_full(src, &all_types(), UnmarshalOptions::default()).expect("decode");
    let names: Vec<&str> = p.directives().iter().map(|d| d.name.as_str()).collect();
    assert_eq!(names, vec!["header", "frob", "meta"]);
    assert_eq!(p.directives()[1].prefixes, vec!["alpha", "beta"]);
    assert!(p.directives()[2].prefixes.is_empty());
}

#[test]
fn nested_block_body_preserved() {
    let (_msg, p) = unmarshal_full(
        "@h T { inner { a = 1 nested { b = \"x\" } } }\nstring_field = \"y\"\n",
        &all_types(),
        UnmarshalOptions::default(),
    )
    .expect("decode");
    let body = std::str::from_utf8(&p.directives()[0].body).unwrap();
    assert!(body.contains("inner {"));
    assert!(body.contains("nested {"));
    assert!(body.contains("b = \"x\""));
}

#[test]
fn at_type_does_not_leak_into_directives() {
    let (_msg, p) = unmarshal_full(
        "@type test.v1.AllTypes\n@frob alpha\nstring_field = \"x\"\n",
        &all_types(),
        UnmarshalOptions::default(),
    )
    .expect("decode");
    assert_eq!(p.directives().len(), 1);
    assert_eq!(p.directives()[0].name, "frob");
}

// ---- @table ----

#[test]
fn table_recorded_with_columns_and_rows() {
    let (_msg, p) = unmarshal_full(
        "@table trades.v1.Trade ( px, qty )\n( 100, 5 )\n( 101, 7 )\n",
        &all_types(),
        UnmarshalOptions::default(),
    )
    .expect("decode");
    assert_eq!(p.tables().len(), 1);
    let t = &p.tables()[0];
    assert_eq!(t.r#type, "trades.v1.Trade");
    assert_eq!(t.columns, vec!["px", "qty"]);
    assert_eq!(t.rows.len(), 2);
    assert_eq!(t.rows[0].cells.len(), 2);
}

#[test]
fn table_cells_carry_actual_values() {
    let (_msg, p) = unmarshal_full(
        "@table x.Row ( a, b, c )\n( 42, \"hello\", true )\n",
        &all_types(),
        UnmarshalOptions::default(),
    )
    .expect("decode");
    let row = &p.tables()[0].rows[0];
    match row.cells[0].as_ref().expect("a present") {
        Value::Int(v) => assert_eq!(v.raw, "42"),
        other => panic!("expected Int, got {:?}", other),
    }
    match row.cells[1].as_ref().expect("b present") {
        Value::String(v) => assert_eq!(v.value, "hello"),
        other => panic!("expected String, got {:?}", other),
    }
    match row.cells[2].as_ref().expect("c present") {
        Value::Bool(v) => assert!(v.value),
        other => panic!("expected Bool, got {:?}", other),
    }
}

#[test]
fn three_state_cells() {
    let (_msg, p) = unmarshal_full(
        "@table x.Row ( a, b, c )\n( 1, , null )\n",
        &all_types(),
        UnmarshalOptions::default(),
    )
    .expect("decode");
    let row = &p.tables()[0].rows[0];
    assert!(row.cells[0].is_some()); // set
    assert!(row.cells[1].is_none()); // absent
    assert!(matches!(row.cells[2], Some(Value::Null(_)))); // present-null
}

#[test]
fn multiple_tables_in_order() {
    let (_msg, p) = unmarshal_full(
        "@table a.Row ( x )\n( 1 )\n@table b.Row ( y, z )\n( \"p\", \"q\" )\n",
        &all_types(),
        UnmarshalOptions::default(),
    )
    .expect("decode");
    let types: Vec<&str> = p.tables().iter().map(|t| t.r#type.as_str()).collect();
    assert_eq!(types, vec!["a.Row", "b.Row"]);
}

#[test]
fn directives_and_tables_coexist() {
    let (_msg, p) = unmarshal_full(
        "@header pkg.Hdr { id = \"h\" }\n@table x.Row ( a )\n( 1 )\n",
        &all_types(),
        UnmarshalOptions::default(),
    )
    .expect("decode");
    assert_eq!(p.directives().len(), 1);
    assert_eq!(p.tables().len(), 1);
    assert_eq!(p.directives()[0].name, "header");
    assert_eq!(p.tables()[0].r#type, "x.Row");
}

#[test]
fn unmarshal_without_presence_still_succeeds() {
    // Regression check: the presence-null branch must not regress.
    let msg = unmarshal(
        "@header pkg.Hdr { id = \"h\" }\n@frob alpha beta\nstring_field = \"x\"\n",
        &all_types(),
        UnmarshalOptions::default(),
    )
    .expect("decode");
    let _ = msg;
}
