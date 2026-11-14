// SPDX-License-Identifier: MIT
// Copyright (c) 2026 TrendVidia, LLC.
//! The `Rewriter` (#131) where `rewrite_oracle.rs`, which holds it to
//! protowire-go on protowire-go's own cases, cannot reach: what a Rust
//! caller sees of the API.

use std::error::Error as _;

use protowire_pxf::{format_value, Entry, Position, PxfError, Rewriter, StringVal, Value};

fn string(s: &str) -> Value {
    Value::String(StringVal {
        pos: Position::default(),
        end: Position::default(),
        value: s.into(),
    })
}

/// protowire-go's `TestFormatValueMatchesRewriter`: `format_value` writes
/// the literal the Rewriter splices for the same value.
#[test]
fn format_value_matches_the_rewriter() {
    let mut r = Rewriter::new("a = 0\n").unwrap();
    r.set("a", &string("x\ny")).unwrap();
    let want = format!("a = {}\n", format_value(&string("x\ny")));
    assert_eq!(String::from_utf8(r.bytes().unwrap()).unwrap(), want);
}

/// A node held from `document()` stays usable while the Rewriter is
/// edited, so one of several siblings with the same key can be replaced:
/// the API decided on #131.
#[test]
fn a_node_held_from_the_document_can_be_edited() {
    let mut r = Rewriter::new("w { n = 1 }\nw { n = 2 }\n").unwrap();
    let doc = r.document();
    for (i, e) in doc.entries.iter().enumerate() {
        let Entry::Block(b) = e else { panic!("{e:?}") };
        let Entry::Assignment(a) = &b.entries[0] else {
            panic!("{:?}", b.entries[0])
        };
        if i == 1 {
            r.replace_value(&a.value, &string("second")).unwrap();
        }
    }
    r.append_entry(
        match &doc.entries[0] {
            Entry::Block(b) => b,
            e => panic!("{e:?}"),
        },
        &protowire_pxf::parse("k = true").unwrap().entries[0],
    )
    .unwrap();
    assert_eq!(
        String::from_utf8(r.bytes().unwrap()).unwrap(),
        "w { n = 1 k = true }\nw { n = \"second\" }\n"
    );
}

/// A rewrite that gives invalid PXF is refused with protowire-go's text, and
/// the parse error is its source, as Go wraps it with `%w`.
#[test]
fn an_invalid_rewrite_keeps_the_parse_error_as_its_source() {
    let mut r = Rewriter::new("a = 1\n").unwrap();
    r.set_span(4, 5, b"}").unwrap();
    let e = r.bytes().unwrap_err();
    assert_eq!(
        e.to_string(),
        "pxf: rewrite produced an invalid document: 1:5: expected value, got } (\"}\")"
    );
    let source = e.source().expect("a source").downcast_ref::<PxfError>();
    assert_eq!(
        source.map(|s| s.to_string()).as_deref(),
        Some("1:5: expected value, got } (\"}\")")
    );
}
