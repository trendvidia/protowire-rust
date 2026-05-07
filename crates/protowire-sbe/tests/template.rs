// SPDX-License-Identifier: MIT
// Copyright (c) 2026 TrendVidia, LLC.
//! Slice A tests for the SBE template + Codec builder. Mirrors the TS
//! port's `src/sbe/template.test.ts`.
//!
//! Drives the codec from the checked-in `sbe-test.binpb` fixture (built via
//! buf from `testdata/sbe-test.proto`), so the test is fully descriptor-
//! driven — no codegen.

use prost_reflect::DescriptorPool;
use protowire_sbe::Codec;

const SBE_FDS: &[u8] = include_bytes!("../testdata/sbe-test.binpb");

fn load_codec() -> Codec {
    let pool = DescriptorPool::decode(SBE_FDS).expect("decode sbe-test.binpb");
    let file = pool
        .get_file_by_name("sbe-test.proto")
        .expect("sbe-test.proto in pool");
    Codec::from_files(&[file]).expect("build codec")
}

// ---------------- registration ----------------

#[test]
fn registers_messages_with_template_id() {
    let codec = load_codec();
    assert!(codec.by_name().contains_key("test.v1.Order"));
    assert!(codec.by_name().contains_key("test.v1.Simple"));
    assert!(codec.by_name().contains_key("test.v1.WithComposite"));
    assert!(codec.by_name().contains_key("test.v1.WithNarrow"));
    // Inner has no template_id and is only referenced as a composite.
    assert!(!codec.by_name().contains_key("test.v1.Inner"));
}

// ---------------- Simple block length ----------------

#[test]
fn computes_simple_block_length() {
    let codec = load_codec();
    let t = codec.template("test.v1.Simple").unwrap();
    // id:uint32(4) + value:int32(4) = 8
    assert_eq!(t.block_length, 8);
    assert_eq!(t.template_id, 2);
    assert_eq!(t.schema_id, 1);
    assert_eq!(t.version, 0);
    assert!(t.groups.is_empty());
}

// ---------------- Order block length and group ----------------

#[test]
fn computes_order_block_length_and_group() {
    let codec = load_codec();
    let t = codec.template("test.v1.Order").unwrap();
    // order_id(8)+symbol(8)+price(8)+quantity(4)+side(1)+active(1)+weight(8)+score(4) = 42
    assert_eq!(t.block_length, 42);
    assert_eq!(t.groups.len(), 1);
    // fill_price(8)+fill_qty(4)+fill_id(8) = 20
    assert_eq!(t.groups[0].block_length, 20);
}

// ---------------- WithComposite block length ----------------

#[test]
fn computes_with_composite_block_length() {
    let codec = load_codec();
    let t = codec.template("test.v1.WithComposite").unwrap();
    // id(8) + inner:x(8)+y(8)=16 + code(4) = 28
    assert_eq!(t.block_length, 28);
    let inner = t
        .fields
        .iter()
        .find(|f| f.fd.name() == "inner")
        .expect("inner field");
    let names: Vec<&str> = inner.composite.iter().map(|c| c.fd.name()).collect();
    assert_eq!(names, vec!["x", "y"]);
}

// ---------------- (sbe.encoding) overrides ----------------

#[test]
fn respects_sbe_encoding_overrides() {
    let codec = load_codec();
    let t = codec.template("test.v1.WithNarrow").unwrap();
    // status:uint8(1) + port:uint16(2) + delta:int16(2) = 5
    assert_eq!(t.block_length, 5);
    let summary: Vec<(&str, &str, usize)> = t
        .fields
        .iter()
        .map(|f| (f.fd.name(), f.encoding.unwrap().name(), f.size))
        .collect();
    assert_eq!(
        summary,
        vec![
            ("status", "uint8", 1),
            ("port", "uint16", 2),
            ("delta", "int16", 2),
        ]
    );
}

// ---------------- lookup by template ID ----------------

#[test]
fn looks_up_by_template_id() {
    let codec = load_codec();
    assert_eq!(
        codec.template_by_id(1).unwrap().desc.full_name(),
        "test.v1.Order"
    );
    assert_eq!(
        codec.template_by_id(2).unwrap().desc.full_name(),
        "test.v1.Simple"
    );
    assert_eq!(
        codec.template_by_id(3).unwrap().desc.full_name(),
        "test.v1.WithComposite"
    );
    assert_eq!(
        codec.template_by_id(4).unwrap().desc.full_name(),
        "test.v1.WithNarrow"
    );
}
