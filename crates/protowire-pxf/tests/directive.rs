// SPDX-License-Identifier: MIT
// Copyright (c) 2026 TrendVidia, LLC.
//! Parser-tier tests for the v0.72-v0.75 directive grammar:
//!   - `@<name> *(<prefix>) [{ ... }]`   (draft §3.4.2)
//!   - `@entry  *(<prefix>) [{ ... }]`   (draft §3.4.3)
//!   - `@table  <type> ( cols ) row*`    (draft §3.4.4)
//!
//! Exercises `parse()` directly and asserts on AST shape. Decode-tier
//! wiring (Presence accessors, TableReader, bind_row) arrives in later
//! PRs of the v0.72-v0.75 catch-up.

use protowire_pxf::ast::Value;
use protowire_pxf::parse;

#[test]
fn bare_directive_no_prefix_no_body() {
    let doc = parse("@frob\nname = \"x\"\n").expect("parse");
    assert_eq!(doc.directives.len(), 1);
    let d = &doc.directives[0];
    assert_eq!(d.name, "frob");
    assert!(d.prefixes.is_empty());
    assert!(!d.has_body);
    assert!(d.r#type.is_empty());
    assert_eq!(doc.entries.len(), 1);
}

#[test]
fn single_prefix_populates_legacy_type() {
    // v0.72.0-era chameleon shape.
    let doc =
        parse("@header chameleon.v1.LayerHeader { id = \"x\" }\nbody = \"z\"\n").expect("parse");
    let d = &doc.directives[0];
    assert_eq!(d.name, "header");
    assert_eq!(d.prefixes, vec!["chameleon.v1.LayerHeader"]);
    assert_eq!(d.r#type, "chameleon.v1.LayerHeader");
    assert!(d.has_body);
    let body = std::str::from_utf8(&d.body).unwrap();
    assert!(body.contains("id = \"x\""));
}

#[test]
fn two_prefixes_leave_type_empty() {
    let doc = parse("@entry mylabel pkg.MsgType { x = 1 }\nname = \"z\"\n").expect("parse");
    let d = &doc.directives[0];
    assert_eq!(d.prefixes, vec!["mylabel", "pkg.MsgType"]);
    assert_eq!(d.r#type, "");
}

#[test]
fn prefix_lookahead_stops_at_body_key() {
    let doc = parse("@foo BarType\nbody_key = \"x\"\n").expect("parse");
    let d = &doc.directives[0];
    assert_eq!(d.prefixes, vec!["BarType"]);
    assert_eq!(doc.entries.len(), 1);
}

#[test]
fn multiple_directives_in_source_order() {
    let src = "@type some.MsgType\n\
               @header pkg.Header { id = \"h1\" }\n\
               @frob alpha beta\n\
               name = \"z\"\n";
    let doc = parse(src).expect("parse");
    assert_eq!(doc.type_url, "some.MsgType");
    let names: Vec<&str> = doc.directives.iter().map(|d| d.name.as_str()).collect();
    assert_eq!(names, vec!["header", "frob"]);
    assert_eq!(doc.directives[1].prefixes, vec!["alpha", "beta"]);
    assert!(doc.body_offset > 0);
}

#[test]
fn body_offset_matches_end_of_last_directive() {
    let doc = parse("@frob alpha\nname = 1\n").expect("parse");
    // "alpha" starts at offset 6 (after "@frob ") and is length 5, so end = 11.
    assert_eq!(doc.body_offset, 11);
}

#[test]
fn block_body_preserves_raw_bytes() {
    let doc = parse("@hdr T { a = 1\n b = \"x\" }\nrest = 0\n").expect("parse");
    let d = &doc.directives[0];
    assert!(d.has_body);
    let body = std::str::from_utf8(&d.body).unwrap();
    assert!(body.contains("a = 1"));
    assert!(body.contains("b = \"x\""));
    assert!(!body.contains('}'));
}

#[test]
fn nested_braces_in_body() {
    let doc = parse("@nested T { inner { a = 1 } }\n").expect("parse");
    let body = std::str::from_utf8(&doc.directives[0].body).unwrap();
    assert!(body.contains("inner { a = 1 }"));
}

#[test]
fn braces_inside_strings_not_counted() {
    let doc = parse("@s T { a = \"}{\" }\n").expect("parse");
    assert!(doc.directives[0].has_body);
}

#[test]
fn line_comment_inside_body() {
    let doc = parse("@h T { a = 1 # trailing } comment\n  b = 2\n}\n").expect("parse");
    assert!(doc.directives[0].has_body);
}

#[test]
fn block_comment_inside_body() {
    let doc = parse("@h T { a = 1 /* not a } close */ b = 2 }\n").expect("parse");
    assert!(doc.directives[0].has_body);
}

#[test]
fn at_type_without_ident_rejected() {
    let err = parse("@type =\n").unwrap_err();
    assert!(err.to_string().contains("expected type name after @type"));
}

#[test]
fn bare_at_is_illegal() {
    parse("@\n").unwrap_err();
}

// ---- Table ----

#[test]
fn table_basic_two_columns_two_rows() {
    let src = "@table trades.v1.Trade ( px, qty )\n( 100, 5 )\n( 101, 7 )\n";
    let doc = parse(src).expect("parse");
    assert_eq!(doc.tables.len(), 1);
    let t = &doc.tables[0];
    assert_eq!(t.r#type, "trades.v1.Trade");
    assert_eq!(t.columns, vec!["px", "qty"]);
    assert_eq!(t.rows.len(), 2);
    assert_eq!(t.rows[0].cells.len(), 2);
}

#[test]
fn table_empty_cell_means_absent() {
    let doc = parse("@table x.Row ( a, b, c )\n( 1, , 3 )\n").expect("parse");
    let row = &doc.tables[0].rows[0];
    assert!(row.cells[0].is_some());
    assert!(row.cells[1].is_none()); // absent
    assert!(row.cells[2].is_some());
}

#[test]
fn table_null_cell_means_present_null() {
    let doc = parse("@table x.Row ( a, b )\n( 1, null )\n").expect("parse");
    let row = &doc.tables[0].rows[0];
    assert!(matches!(row.cells[1], Some(Value::Null(_))));
}

#[test]
fn table_zero_rows_valid() {
    let doc = parse("@table x.Row ( a, b )\n").expect("parse");
    assert_eq!(doc.tables.len(), 1);
    assert!(doc.tables[0].rows.is_empty());
}

#[test]
fn table_arity_mismatch_rejected() {
    let err = parse("@table x.Row ( a, b )\n( 1, 2, 3 )\n").unwrap_err();
    assert!(err.to_string().contains("3 cells, expected 2"));
}

#[test]
fn table_dotted_column_rejected() {
    let err = parse("@table x.Row ( a.b )\n").unwrap_err();
    assert!(err.to_string().contains("dotted column"));
}

#[test]
fn table_list_cell_rejected() {
    let err = parse("@table x.Row ( a )\n( [1, 2] )\n").unwrap_err();
    assert!(err.to_string().contains("list values"));
}

#[test]
fn table_block_cell_rejected() {
    let err = parse("@table x.Row ( a )\n( { x = 1 } )\n").unwrap_err();
    assert!(err.to_string().contains("block values"));
}

#[test]
fn standalone_rejects_coexisting_at_type_before() {
    let err = parse("@type other\n@table x.Row ( a )\n( 1 )\n").unwrap_err();
    assert!(err.to_string().contains("cannot coexist with @type"));
}

#[test]
fn standalone_rejects_at_type_after_table() {
    let err = parse("@table x.Row ( a )\n@type other\n").unwrap_err();
    assert!(err.to_string().contains("cannot coexist with @type"));
}

#[test]
fn standalone_rejects_coexisting_body_entries() {
    let err = parse("@table x.Row ( a )\n( 1 )\nextra = 5\n").unwrap_err();
    assert!(err
        .to_string()
        .contains("cannot coexist with top-level field entries"));
}

#[test]
fn missing_type_after_at_table_rejected() {
    let err = parse("@table ( a )\n").unwrap_err();
    assert!(err
        .to_string()
        .contains("expected row message type after @table"));
}

#[test]
fn missing_lparen_rejected() {
    let err = parse("@table x.Row a, b\n").unwrap_err();
    assert!(err.to_string().contains("expected '(' to start"));
}

#[test]
fn empty_column_list_rejected() {
    let err = parse("@table x.Row ( )\n").unwrap_err();
    assert!(err.to_string().contains("at least one field name"));
}

#[test]
fn bad_column_token_rejected() {
    let err = parse("@table x.Row ( a, 123 )\n").unwrap_err();
    assert!(err.to_string().contains("expected column field name"));
}

#[test]
fn missing_comma_in_column_list_rejected() {
    let err = parse("@table x.Row ( a b )\n").unwrap_err();
    assert!(err
        .to_string()
        .contains("expected ',' or ')' in @table column list"));
}

#[test]
fn missing_comma_in_row_rejected() {
    let err = parse("@table x.Row ( a, b )\n( 1 2 )\n").unwrap_err();
    assert!(err
        .to_string()
        .contains("expected ',' or ')' in @table row"));
}
