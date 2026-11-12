// SPDX-License-Identifier: MIT
// Copyright (c) 2026 TrendVidia, LLC.
//! `marshal` and an unset annotated field (#116, protowire-go#259). A reader
//! applies `(pxf.default)` to an absent field and checks `(pxf.required)`,
//! so leaving an annotated field out would not read back as the message;
//! `marshal` writes such a field whenever text can say what it holds. The
//! draft requires decode_PXF(encode_PXF(v)) == v (Common Schema Layer).
//! Ports protowire-go's `marshal_unset_annotated_test.go`; every expected
//! text is what that test expects of protowire-go.

use prost::Message as _;
use prost_reflect::{DescriptorPool, DynamicMessage, MessageDescriptor, ReflectMessage, Value};
use protowire_pxf::{marshal, unmarshal_full, MarshalOptions, UnmarshalOptions};

const FDS: &[u8] = include_bytes!("../testdata/unset-annotated-test.binpb");

fn desc(name: &str) -> MessageDescriptor {
    DescriptorPool::decode(FDS)
        .unwrap()
        .get_message_by_name(&format!("unsetann.v1.{name}"))
        .unwrap()
}

/// Marshals `m` and reads the text back with defaults applied and required
/// fields checked.
fn full_read_back(m: &DynamicMessage) -> (String, DynamicMessage) {
    let out = marshal(m, &m.descriptor(), MarshalOptions::default()).unwrap();
    let (back, _) = unmarshal_full(&out, &m.descriptor(), UnmarshalOptions::default())
        .unwrap_or_else(|e| panic!("{out}: {e}"));
    (out, back)
}

/// Equality by encoded bytes: `assert_eq!` on a `DynamicMessage` can
/// recurse without end in `Debug` for self-referential descriptors.
fn same(a: &DynamicMessage, b: &DynamicMessage) -> bool {
    a.encode_to_vec() == b.encode_to_vec()
}

fn set_i32(m: &mut DynamicMessage, name: &str, v: i32) {
    m.set_field_by_name(name, Value::I32(v));
}

// The issue's case: a zero proto3 scalar with a default reads back zero.
#[test]
fn zero_defaulted_scalar_reads_back() {
    let mut m = DynamicMessage::new(desc("Sub"));
    set_i32(&mut m, "plain", 1);
    let (out, back) = full_read_back(&m);
    assert_eq!(out, "k = 0\nplain = 1\n");
    assert!(same(&m, &back), "{out}");
}

// Every unset annotated shape at once, top level and one level down, with
// nothing but plain fields set.
#[test]
fn unset_annotated_fields_read_back() {
    let d = desc("M");
    let mut m = DynamicMessage::new(d.clone());
    set_i32(&mut m, "plain", 1);
    let mut sub = DynamicMessage::new(desc("Sub"));
    set_i32(&mut sub, "plain", 2);
    m.set_field_by_name("sub", Value::Message(sub));

    let (out, mut back) = full_read_back(&m);
    assert_eq!(
        out,
        "n = 0\ns = \"\"\nb = false\nreq = 0\nrl = []\nrm = {}\nw = null\nts = null\nplain = 1\nsub {\n  k = 0\n  plain = 2\n}\n"
    );
    // The oneof member with a default is the one field that reads back as
    // its default: marshal leaves it out rather than write null
    // (protowire-go#267, owner's choice).
    let a = d.get_field_by_name("a").unwrap();
    assert_eq!(back.get_field(&a).as_i32(), Some(8));
    back.clear_field(&a);
    assert!(same(&m, &back), "{out}");
}

// A set oneof member keeps its sibling's default off, so the sibling is not
// written.
#[test]
fn oneof_sibling_keeps_default_off() {
    let mut m = DynamicMessage::new(desc("M"));
    m.set_field_by_name("other", Value::String("x".into()));
    let (out, back) = full_read_back(&m);
    assert!(!out.contains("a = "), "{out}");
    assert!(same(&m, &back), "{out}");
}

// An unset `optional` scalar with a default is left out and reads back as
// its default (protowire-go#267); `o = null` is the text that keeps it unset.
#[test]
fn unset_optional_defaulted_reads_back_default() {
    let d = desc("Opt");
    let (out, back) = full_read_back(&DynamicMessage::new(d.clone()));
    assert_eq!(out, "");
    assert_eq!(back.get_field_by_name("o").unwrap().as_i32(), Some(6));

    let (back, _) = unmarshal_full("o = null\n", &d, UnmarshalOptions::default()).unwrap();
    assert!(
        !back.has_field_by_name("o"),
        "null is the text that keeps it unset"
    );
}

// Pinned limit (protowire-go#267): in a message with a _null mask, every
// null read is recorded in the mask, so an unset defaulted field has no text
// that reads back as "unset and not null". No writer can avoid this.
#[test]
fn unset_defaulted_in_null_mask_message_has_no_text() {
    let d = desc("N");
    let mut m = DynamicMessage::new(d.clone());
    set_i32(&mut m, "plain", 1);

    let (out, back) = full_read_back(&m);
    assert_eq!(out, "w = null\nplain = 1\n");
    assert!(!back.has_field_by_name("w"));
    let mask = back.get_field_by_name("_null").unwrap();
    let paths = mask
        .as_message()
        .unwrap()
        .get_field_by_name("paths")
        .unwrap();
    assert_eq!(paths.as_list().unwrap(), &[Value::String("w".into())][..]);
    assert!(!same(&m, &back), "the mask names w");

    let (back, _) = unmarshal_full("plain = 1\n", &d, UnmarshalOptions::default()).unwrap();
    let w = back.get_field_by_name("w").unwrap();
    assert_eq!(
        w.as_message()
            .unwrap()
            .get_field_by_name("value")
            .unwrap()
            .as_i32(),
        Some(7)
    );
    assert!(!back.has_field_by_name("_null"));
}

// An unset required field with presence has no text a reader accepts:
// absent is rejected, and so is null. So marshal refuses it. With
// emit_defaults it is written as zero, which a reader accepts.
#[test]
fn unset_required_optional_refused() {
    let d = desc("ReqOpt");
    let mut m = DynamicMessage::new(d.clone());
    set_i32(&mut m, "plain", 1);

    // protowire-go's text after its "pxf: " prefix. This port prints every
    // marshal error after a 0:0 position instead (#138).
    let err = marshal(&m, &d, MarshalOptions::default()).unwrap_err();
    assert_eq!(
        err.to_string(),
        "0:0: unsetann.v1.ReqOpt.o is unset, and the field is (pxf.required)"
    );

    let out = marshal(
        &m,
        &d,
        MarshalOptions {
            emit_defaults: true,
            ..Default::default()
        },
    )
    .unwrap();
    assert_eq!(out, "o = 0\nplain = 1\n");

    set_i32(&mut m, "o", 0);
    let (_, back) = full_read_back(&m);
    assert!(same(&m, &back));
}

// Not in protowire-go's file: the same rule in a list element and a map
// value, which protowire-go's encodeMessage reaches the same way.
#[test]
fn unset_annotated_fields_in_elements_read_back() {
    let d = desc("Holder");
    let mut m = DynamicMessage::new(d.clone());
    let mut sub = DynamicMessage::new(desc("Sub"));
    set_i32(&mut sub, "plain", 1);
    m.set_field_by_name("subs", Value::List(vec![Value::Message(sub.clone())]));
    let mut map = std::collections::HashMap::new();
    map.insert(
        prost_reflect::MapKey::String("x".into()),
        Value::Message(sub),
    );
    m.set_field_by_name("by_name", Value::Map(map));

    let (out, back) = full_read_back(&m);
    assert_eq!(
        out,
        "subs = [\n  {\n    k = 0\n    plain = 1\n  }\n]\nby_name = {\n  x: {\n    k = 0\n    plain = 1\n  }\n}\n"
    );
    assert!(same(&m, &back), "{out}");
}
