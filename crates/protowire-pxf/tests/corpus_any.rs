// SPDX-License-Identifier: MIT
// Copyright (c) 2026 TrendVidia, LLC.
//! The type entry of a `google.protobuf.Any` block (draft -01 "Any Blocks";
//! protowire#354). `testdata/any/` is vendored from trendvidia/protowire at
//! e9e0a17.

mod common;

use common::{fixtures, message, must_reject, pool};
use prost::Message as _;
use prost_reflect::{DescriptorPool, DynamicMessage, Value};
use protowire_pxf::{format, parse, unmarshal, unmarshal_full, PoolResolver, UnmarshalOptions};

const FDS: &[u8] = include_bytes!("../testdata/any/any.binpb");
const D4: &[u8] = include_bytes!("../testdata/d4-test.binpb");

/// The fixtures that a parser has to accept because only a schema can
/// refuse them, per the corpus README.
const PARSE_ACCEPTS: [&str; 2] = ["err-not-any", "err-unresolved"];

fn opts(pool: &DescriptorPool) -> UnmarshalOptions<'_> {
    // Leaked so the options, which borrow a `dyn TypeResolver`, can be
    // returned; a test process is short.
    let resolver: &'static PoolResolver<'static> =
        Box::leak(Box::new(PoolResolver(Box::leak(Box::new(pool.clone())))));
    UnmarshalOptions {
        type_resolver: Some(resolver),
        ..Default::default()
    }
}

fn leaf_of(any: &DynamicMessage, p: &DescriptorPool) -> DynamicMessage {
    let bytes = any.get_field_by_name("value").unwrap();
    let mut leaf = DynamicMessage::new(message(p, "any.v1.Leaf"));
    leaf.merge(bytes.as_bytes().unwrap().clone()).unwrap();
    leaf
}

#[test]
fn every_fixture_meets_its_verdict() {
    let p = pool(FDS);
    let d = message(&p, "any.v1.Holder");
    let files = fixtures("any");
    assert_eq!(files.len(), 11, "the README lists eleven fixtures");
    for (stem, doc) in files {
        let parsed = parse(&doc);
        let decoded = unmarshal(&doc, &d, opts(&p));
        if must_reject(&stem) {
            assert!(decoded.is_err(), "{stem}: decode must reject");
            assert_eq!(
                parsed.is_ok(),
                PARSE_ACCEPTS.contains(&stem.as_str()),
                "{stem}: parse"
            );
        } else {
            parsed.unwrap_or_else(|e| panic!("{stem}: parse: {e}"));
            decoded.unwrap_or_else(|e| panic!("{stem}: decode: {e}"));
        }
    }
}

fn fixture(stem: &str) -> String {
    fixtures("any")
        .into_iter()
        .find(|(s, _)| s == stem)
        .unwrap()
        .1
}

#[test]
fn the_body_packs_as_the_named_type_and_the_url_is_kept_verbatim() {
    let p = pool(FDS);
    let d = message(&p, "any.v1.Holder");
    let any_of = |stem: &str, field: &str| -> DynamicMessage {
        let msg = unmarshal(&fixture(stem), &d, opts(&p)).unwrap();
        let v = msg.get_field_by_name(field).unwrap().into_owned();
        match v {
            Value::Message(m) => m,
            Value::List(l) => l[1].as_message().unwrap().clone(),
            Value::Map(m) => m
                .into_values()
                .next()
                .unwrap()
                .as_message()
                .unwrap()
                .clone(),
            other => panic!("{other:?}"),
        }
    };
    let block = any_of("block", "payload");
    assert_eq!(
        block.get_field_by_name("type_url").unwrap().as_str(),
        Some("any.v1.Leaf")
    );
    let leaf = leaf_of(&block, &p);
    assert_eq!(leaf.get_field_by_name("n").unwrap().as_i32(), Some(7));
    assert_eq!(
        any_of("url-verbatim", "payload")
            .get_field_by_name("type_url")
            .unwrap()
            .as_str(),
        Some("type.googleapis.com/any.v1.Leaf")
    );
    let second = leaf_of(&any_of("list-element", "anys"), &p);
    assert_eq!(second.get_field_by_name("n").unwrap().as_i32(), Some(2));
    let in_map = leaf_of(&any_of("map-value", "by_key"), &p);
    assert_eq!(in_map.get_field_by_name("tag").unwrap().as_str(), Some("t"));
    let plain = any_of("plain-fields", "payload");
    assert_eq!(
        leaf_of(&plain, &p)
            .get_field_by_name("tag")
            .unwrap()
            .as_str(),
        Some("t")
    );
}

#[test]
fn a_type_entry_without_a_resolver_is_an_error_not_a_skip() {
    let p = pool(FDS);
    let d = message(&p, "any.v1.Holder");
    let e = unmarshal(&fixture("block"), &d, UnmarshalOptions::default()).unwrap_err();
    assert!(e.msg.contains("cannot resolve Any type"), "{}", e.msg);
}

#[test]
fn format_keeps_a_type_entry_first_and_round_trips() {
    let doc = parse(&fixture("block")).unwrap();
    let out = format(&doc);
    assert!(out.contains("  @type = \"any.v1.Leaf\"\n"), "{out}");
    assert_eq!(format(&parse(&out).unwrap()), out);
}

/// Packed to bytes, an Any's body is out of reach of the post-decode pass
/// over the root, so it takes the pass before it is packed.
#[test]
fn required_and_default_apply_inside_an_any_body() {
    let p = pool(D4);
    let d = message(&p, "d4_test.v1.AnyHolder");
    let o = opts(&p);
    let doc = "payload {\n  @type = \"d4_test.v1.WithRequired\"\n  value = 1\n}\n";
    let e = unmarshal_full(doc, &d, o).unwrap_err();
    assert!(e.msg.contains("payload.name"), "{}", e.msg);

    let doc = "payload {\n  @type = \"d4_test.v1.WithDefault\"\n}\n";
    let (msg, _) = unmarshal_full(doc, &d, o).unwrap();
    let any = msg.get_field_by_name("payload").unwrap();
    let bytes = any
        .as_message()
        .unwrap()
        .get_field_by_name("value")
        .unwrap();
    let mut inner = DynamicMessage::new(message(&p, "d4_test.v1.WithDefault"));
    inner.merge(bytes.as_bytes().unwrap().clone()).unwrap();
    assert_eq!(
        inner.get_field_by_name("name").unwrap().as_str(),
        Some("anonymous")
    );
    assert!(!inner.encode_to_vec().is_empty());
}
