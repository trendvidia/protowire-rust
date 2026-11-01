// SPDX-License-Identifier: MIT
// Copyright (c) 2026 TrendVidia, LLC.
//! An entry name of a keyed block populates the element's key field (draft
//! -01 §3.13), so it counts as the document supplying that field
//! (protowire-go#214). Mirrors the reference's
//! `keyed_entry_name_presence_test.go`, and pins the paths that report it:
//! each list element and map value has its own, and nothing marked inside
//! one lands on the parent.
//!
//! The port does not yet run the `(pxf.default)` / `(pxf.required)` pass
//! inside list elements, so the reference's tests of an anonymous element
//! taking a default have no counterpart here.

use prost_reflect::{DescriptorPool, DynamicMessage, MessageDescriptor, Value};
use protowire_pxf::{
    bind_row, marshal, parse, unmarshal, unmarshal_full, MarshalOptions, PoolResolver,
    UnmarshalOptions,
};

const FDS: &[u8] = include_bytes!("../testdata/keyed-entry-name-test.binpb");

fn pool() -> DescriptorPool {
    DescriptorPool::decode(FDS).expect("decode keyed-entry-name-test.binpb")
}

fn p() -> MessageDescriptor {
    pool().get_message_by_name("keyedname.v1.P").unwrap()
}

fn shadow() -> MessageDescriptor {
    pool().get_message_by_name("keyedname.v1.Shadow").unwrap()
}

fn elem(m: &DynamicMessage, field: &str, i: usize) -> DynamicMessage {
    match m.get_field_by_name(field).unwrap().as_ref() {
        Value::List(l) => match &l[i] {
            Value::Message(e) => e.clone(),
            other => panic!("{field}[{i}]: {other:?}"),
        },
        other => panic!("{field}: {other:?}"),
    }
}

fn text(m: &DynamicMessage, field: &str) -> String {
    match m.get_field_by_name(field).unwrap().as_ref() {
        Value::String(s) => s.clone(),
        other => panic!("{field}: {other:?}"),
    }
}

#[test]
fn entry_name_supplies_the_key_field() {
    type Get = fn(&DynamicMessage) -> DynamicMessage;
    let cases: &[(&str, Get, &str, &str)] = &[
        (
            "kd { alpha {} }",
            |m| elem(m, "kd", 0),
            "alpha",
            "kd[0].name",
        ),
        (
            r#"kd { "not ident" {} }"#,
            |m| elem(m, "kd", 0),
            "not ident",
            "kd[0].name",
        ),
        (
            "kd { alpha = {} }",
            |m| elem(m, "kd", 0),
            "alpha",
            "kd[0].name",
        ),
        (
            "kd = { alpha {} }",
            |m| elem(m, "kd", 0),
            "alpha",
            "kd[0].name",
        ),
        (
            "kd { a {} alpha {} }",
            |m| elem(m, "kd", 1),
            "alpha",
            "kd[1].name",
        ),
        (
            r#"kd { alpha { name = "alpha" } }"#,
            |m| elem(m, "kd", 0),
            "alpha",
            "kd[0].name",
        ),
        (
            "kr { alpha {} }",
            |m| elem(m, "kr", 0),
            "alpha",
            "kr[0].name",
        ),
        (
            "ko { alpha {} }",
            |m| elem(m, "ko", 0),
            "alpha",
            "ko[0].name",
        ),
        (
            "kn { a { inner { alpha {} } } }",
            |m| elem(&elem(m, "kn", 0), "inner", 0),
            "alpha",
            "kn[0].inner[0].name",
        ),
    ];
    for (doc, get, want, mark) in cases {
        let (m, presence) = unmarshal_full(doc, &p(), UnmarshalOptions::default())
            .unwrap_or_else(|e| panic!("{doc}: {e}"));
        assert_eq!(text(&get(&m), "name"), *want, "{doc}");
        assert!(presence.is_set(mark), "{doc}: {mark} not set");

        // Without presence tracking the key is set just the same.
        let m = unmarshal(doc, &p(), UnmarshalOptions::default()).unwrap();
        assert_eq!(text(&get(&m), "name"), *want, "{doc}: unmarshal");
    }
}

#[test]
fn entry_name_does_not_mark_the_parent() {
    let (_, presence) =
        unmarshal_full("kd { alpha {} }", &p(), UnmarshalOptions::default()).expect("decodes");
    assert!(presence.is_absent("name"));
}

/// Only an entry name counts as supplying the key.
#[test]
fn anonymous_element_does_not_mark_the_key() {
    let (_, presence) =
        unmarshal_full("kd = [{}]", &p(), UnmarshalOptions::default()).expect("decodes");
    assert!(presence.is_absent("kd[0].name"));
}

#[test]
fn oneof_member_key_stays_chosen() {
    let (m, _) =
        unmarshal_full("ko { alpha {} }", &p(), UnmarshalOptions::default()).expect("decodes");
    let e = elem(&m, "ko", 0);
    assert_eq!(text(&e, "name"), "alpha");
    assert!(!e.has_field_by_name("other"));
}

#[test]
fn inside_an_any_body() {
    let pool = pool();
    let resolver = PoolResolver(&pool);
    let opts = UnmarshalOptions {
        type_resolver: Some(&resolver),
        ..Default::default()
    };
    let doc = r#"payload { @type = "keyedname.v1.Nests" inner { alpha {} } }"#;
    let (m, presence) = unmarshal_full(doc, &p(), opts).expect("decodes");
    assert!(presence.is_set("payload.inner[0].name"));
    let any = match m.get_field_by_name("payload").unwrap().into_owned() {
        Value::Message(a) => a,
        other => panic!("{other:?}"),
    };
    let bytes = match any.get_field_by_name("value").unwrap().into_owned() {
        Value::Bytes(b) => b,
        other => panic!("{other:?}"),
    };
    let nests = pool.get_message_by_name("keyedname.v1.Nests").unwrap();
    let inner = DynamicMessage::decode(nests, bytes).expect("packed body");
    assert_eq!(text(&elem(&inner, "inner", 0), "name"), "alpha");
}

#[test]
fn in_a_dataset_cell() {
    let doc = parse("@dataset keyedname.v1.P (kd)\n({ alpha {} })\n").expect("parses");
    let ds = &doc.datasets[0];
    let m = bind_row(&p(), &ds.columns, &ds.rows[0], UnmarshalOptions::default()).expect("binds");
    assert_eq!(text(&elem(&m, "kd", 0), "name"), "alpha");
}

/// A mark made inside an element belongs to the element: it neither
/// satisfies the parent's required field nor keeps its default off.
#[test]
fn element_bodies_do_not_supply_the_parents_fields() {
    for doc in [
        r#"items = [{ name = "x" }]"#,
        "keyed { x {} }",
        r#"keyed { x { name = "x" } }"#,
        r#"by_id = { a: { name = "x" } }"#,
        r#"by_num = { 7: { name = "x" } }"#,
    ] {
        let err = unmarshal_full(doc, &shadow(), UnmarshalOptions::default())
            .expect_err(doc)
            .to_string();
        assert!(
            err.contains(r#"required field "name" is absent"#),
            "{doc}: {err}"
        );
    }
    for doc in [
        r#"name = "r" items = [{ label = "x" }]"#,
        r#"name = "r" keyed { x { label = "y" } }"#,
        r#"name = "r" by_id = { a: { label = "x" } }"#,
        r#"name = "r" by_num = { 7: { label = "x" } }"#,
    ] {
        let (m, presence) = unmarshal_full(doc, &shadow(), UnmarshalOptions::default()).expect(doc);
        assert_eq!(text(&m, "label"), "shadow-dflt", "{doc}");
        assert!(presence.is_absent("label"), "{doc}");
    }
}

#[test]
fn paths_carry_the_index_or_the_key() {
    let doc = r#"
        name = "r"
        items = [{ label = "a" }, { label = "b" }]
        items = [{ label = "c" }]
        keyed { x { label = "d" } }
        by_id = { a: { label = "e" } }
        by_num = { 7: { label = "f" } }
    "#;
    let (_, presence) = unmarshal_full(doc, &shadow(), UnmarshalOptions::default()).expect("ok");
    for path in [
        "items[0].label",
        "items[1].label",
        // A second binding continues the list, so the index does too.
        "items[2].label",
        "keyed[0].label",
        "keyed[0].name",
        r#"by_id["a"].label"#,
        "by_num[7].label",
    ] {
        assert!(presence.is_set(path), "{path}");
    }
}

/// A null inside an element belongs to that element alone. It stays out of
/// the root's `_null` mask, whose paths name fields, and the encoder finds
/// it under the same path the decoder recorded.
#[test]
fn a_null_in_an_element_stays_on_that_element() {
    let doc = r#"
        name = "r"
        items = [{ label = "a" note = null }, { label = "b" }]
        keyed { x { note = null } y {} }
        by_id = { a: { note = null } b: { label = "c" } }
        by_num = { 7: { note = null } 8: { label = "d" } }
    "#;
    let d = shadow();
    let (m, presence) = unmarshal_full(doc, &d, UnmarshalOptions::default()).expect("decodes");
    let mut nulls: Vec<&str> = presence.null_paths().collect();
    nulls.sort();
    assert_eq!(
        nulls,
        [
            r#"by_id["a"].note"#,
            "by_num[7].note",
            "items[0].note",
            "keyed[0].note",
        ]
    );
    assert!(!m.has_field_by_name("_null"));

    let text = marshal(
        &m,
        &d,
        MarshalOptions {
            null_fields: Some(&presence),
            ..Default::default()
        },
    )
    .expect("encodes");
    assert_eq!(text.matches("note = null").count(), 4, "{text}");
    let (_, again) = unmarshal_full(&text, &d, UnmarshalOptions::default()).expect("re-reads");
    let mut after: Vec<&str> = again.null_paths().collect();
    after.sort();
    assert_eq!(after, nulls, "{text}");
}
