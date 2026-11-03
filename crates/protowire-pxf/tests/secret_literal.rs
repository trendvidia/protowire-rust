// SPDX-License-Identifier: MIT
// Copyright (c) 2026 TrendVidia, LLC.
//! Draft -01 § pxf.Secret (#97): a `pxf.Secret` is written either as a
//! string literal, which sets `value` alone (`pw = "x"`), or as a block. Each
//! element of a list, and each value of a map, may take either form. The
//! encoder writes the literal unless a `hint` or `fingerprint` is set. The
//! bytes are protowire-go's for the spec's text cases `WellKnown.secret`
//! (`7a 04 0a 02 70 77`, there at field 15).

use prost::Message as _;
use prost_reflect::{DescriptorPool, DynamicMessage, MessageDescriptor};
use protowire_pxf::{marshal, unmarshal, MarshalOptions, UnmarshalOptions};

const FDS: &[u8] = include_bytes!("../testdata/secret-literal-test.binpb");

fn desc() -> MessageDescriptor {
    DescriptorPool::decode(FDS)
        .unwrap()
        .get_message_by_name("secretlit.v1.S")
        .unwrap()
}

fn decode(doc: &str) -> DynamicMessage {
    unmarshal(doc, &desc(), UnmarshalOptions::default()).unwrap_or_else(|e| panic!("{e}\n{doc}"))
}

fn hex(b: &[u8]) -> String {
    b.iter().map(|x| format!("{x:02x}")).collect()
}

#[test]
fn a_string_literal_sets_value_alone() {
    // pw is field 1: 0a 04 | 0a 02 "pw".
    assert_eq!(hex(&decode(r#"pw = "pw""#).encode_to_vec()), "0a040a027077");
    // The block form gives the same bytes.
    assert_eq!(
        hex(&decode(r#"pw { value = "pw" }"#).encode_to_vec()),
        "0a040a027077"
    );
}

#[test]
fn either_form_in_a_list_and_a_map() {
    let lit = decode(r#"pws = ["a", { value = "b" hint = "h" }]"#);
    let block = decode(r#"pws = [{ value = "a" }, { value = "b" hint = "h" }]"#);
    assert_eq!(lit.encode_to_vec(), block.encode_to_vec());
    // One entry per map: prost-reflect writes a map in hash order.
    for (lit, block) in [
        (
            r#"by_tenant = { acme: "k" }"#,
            r#"by_tenant = { acme: { value = "k" } }"#,
        ),
        (
            r#"by_tenant = { "a b": "" }"#,
            r#"by_tenant = { "a b": { value = "" } }"#,
        ),
    ] {
        assert_eq!(
            decode(lit).encode_to_vec(),
            decode(block).encode_to_vec(),
            "{lit}"
        );
    }
}

#[test]
fn another_token_is_refused() {
    let e = unmarshal("pw = 5", &desc(), UnmarshalOptions::default()).unwrap_err();
    assert!(e.to_string().contains("expected"), "{e}");
}

#[test]
fn marshal_writes_the_literal_unless_there_is_metadata() {
    let d = desc();
    let out = marshal(&decode(r#"pw = "pw""#), &d, MarshalOptions::default()).unwrap();
    assert_eq!(out, "pw = \"pw\"\n");
    let with_hint = decode(r#"pw { value = "pw" hint = "h" fingerprint = "f" }"#);
    let out = marshal(&with_hint, &d, MarshalOptions::default()).unwrap();
    assert_eq!(
        out,
        "pw {\n  value = \"pw\"\n  hint = \"h\"\n  fingerprint = \"f\"\n}\n"
    );

    // Both read back to the same message.
    for doc in [
        r#"pw = "pw" pws = ["a", "b"] by_tenant = { x: "y" }"#,
        r#"pw { value = "v" hint = "h" }"#,
    ] {
        let m = decode(doc);
        let text = marshal(&m, &d, MarshalOptions::default()).unwrap();
        assert_eq!(decode(&text).encode_to_vec(), m.encode_to_vec(), "{text}");
    }
}
