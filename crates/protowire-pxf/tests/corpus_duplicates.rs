// SPDX-License-Identifier: MIT
// Copyright (c) 2026 TrendVidia, LLC.
//! A singular field and a map key are each bound at most once in one
//! message body (draft -01 § Entries and Keys; protowire#347, #396).
//! `testdata/duplicates/` is vendored from trendvidia/protowire at e9e0a17.

mod common;

use common::{fixtures, message, must_reject, pool};
use prost_reflect::{DynamicMessage, MapKey, MessageDescriptor, Value};
use protowire_pxf::{parse, unmarshal, unmarshal_full, UnmarshalOptions};

const FDS: &[u8] = include_bytes!("../testdata/duplicates/duplicates.binpb");

fn holder() -> MessageDescriptor {
    message(&pool(FDS), "dup.v1.Holder")
}

fn decode(doc: &str) -> DynamicMessage {
    unmarshal(doc, &holder(), UnmarshalOptions::default()).unwrap_or_else(|e| panic!("{doc}: {e}"))
}

#[test]
fn every_fixture_meets_its_verdict() {
    let d = holder();
    let opts = UnmarshalOptions::default();
    let files = fixtures("duplicates");
    assert_eq!(files.len(), 19, "the README lists nineteen fixtures");
    for (stem, doc) in files {
        // A second binding is legal syntax: only a schema can see it.
        parse(&doc).unwrap_or_else(|e| panic!("{stem}: parse: {e}"));
        if must_reject(&stem) {
            let e = unmarshal(&doc, &d, opts).expect_err(&stem);
            assert!(
                e.msg.contains("already"),
                "{stem}: rejected for another reason: {}",
                e.msg
            );
            assert!(unmarshal_full(&doc, &d, opts).is_err(), "{stem}: full");
        } else {
            unmarshal(&doc, &d, opts).unwrap_or_else(|e| panic!("{stem}: {e}"));
            unmarshal_full(&doc, &d, opts).unwrap_or_else(|e| panic!("{stem}: full: {e}"));
        }
    }
}

#[test]
fn two_map_bindings_with_distinct_keys_merge() {
    let msg = decode("labels = { a: \"1\" }\nlabels = { b: \"2\" }\n");
    let Value::Map(m) = msg.get_field_by_name("labels").unwrap().into_owned() else {
        panic!("labels is a map");
    };
    assert_eq!(m.len(), 2);
    assert_eq!(
        m.get(&MapKey::String("a".into())).and_then(Value::as_str),
        Some("1")
    );
    assert_eq!(
        m.get(&MapKey::String("b".into())).and_then(Value::as_str),
        Some("2")
    );
}

#[test]
fn a_repeated_field_bound_twice_concatenates_in_order() {
    let msg = decode("items = [{ a = 1 }]\nitems = [{ a = 2 }, { a = 3 }]\n");
    let list = msg.get_field_by_name("items").unwrap();
    assert_eq!(list.as_list().map(<[Value]>::len), Some(3));
}

#[test]
fn the_error_says_which_field_or_key() {
    let d = holder();
    let opts = UnmarshalOptions::default();
    let e = unmarshal("s = \"a\"\ns = \"b\"\n", &d, opts).unwrap_err();
    assert_eq!(e.msg, "field \"s\" is already set");
    assert_eq!(e.pos.line, 2);
    let e = unmarshal("nums = { 1: \"x\" \"1\": \"y\" }\n", &d, opts).unwrap_err();
    assert_eq!(e.msg, "map field \"nums\": key 1 is already set");
    let e = unmarshal("labels = { a: \"1\" a: \"2\" }\n", &d, opts).unwrap_err();
    assert_eq!(e.msg, "map field \"labels\": key \"a\" is already set");
}

/// A field `discard_unknown` drops binds nothing, so naming it twice is
/// not a second binding.
#[test]
fn a_discarded_field_may_repeat() {
    let opts = UnmarshalOptions {
        discard_unknown: true,
        ..Default::default()
    };
    unmarshal("nope = 1\nnope = 2\n", &holder(), opts).unwrap();
}
