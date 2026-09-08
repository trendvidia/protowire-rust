// SPDX-License-Identifier: MIT
// Copyright (c) 2026 TrendVidia, LLC.
//! Arbitrary-precision literal forms: `pxf.BigInt` and `pxf.Decimal`
//! (protowire-rust#34). Mirrors the cases of protowire-go's
//! `encoding/pxf/bignum_test.go`, plus the digit cap at its bound.
//!
//! `pxf.BigFloat`'s literal form is not implemented in this port; the
//! test at the bottom pins that as a documented gap (see the tracking
//! issue it names) rather than letting it drift silently.

use prost::Message as _;
use prost_reflect::{
    DescriptorPool, DynamicMessage, MapKey, MessageDescriptor, ReflectMessage, Value,
};
use protowire_pxf::{
    marshal, unmarshal, unmarshal_full, MarshalOptions, UnmarshalOptions,
    MAX_NUMERIC_LITERAL_DIGITS,
};

const FDS: &[u8] = include_bytes!("../testdata/bignum-test.binpb");

fn desc(name: &str) -> MessageDescriptor {
    DescriptorPool::decode(FDS)
        .expect("decode bignum-test.binpb")
        .get_message_by_name(name)
        .unwrap_or_else(|| panic!("missing {name}"))
}

fn demo() -> MessageDescriptor {
    desc("bignum_test.v1.BigNumDemo")
}

fn decode(input: &str, d: &MessageDescriptor) -> DynamicMessage {
    unmarshal(input, d, UnmarshalOptions::default()).unwrap_or_else(|e| panic!("{input:?}: {e}"))
}

fn sub(msg: &DynamicMessage, field: &str) -> DynamicMessage {
    let fd = msg.descriptor().get_field_by_name(field).unwrap();
    match msg.get_field(&fd).into_owned() {
        Value::Message(m) => m,
        other => panic!("{field} is {other:?}"),
    }
}

/// (abs, negative) of a pxf.BigInt message; `abs` empty means zero.
fn big_int(m: &DynamicMessage) -> (Vec<u8>, bool) {
    let abs = match m.get_field_by_name("abs").map(|v| v.into_owned()) {
        Some(Value::Bytes(b)) => b.to_vec(),
        _ => Vec::new(),
    };
    let neg = matches!(
        m.get_field_by_name("negative").map(|v| v.into_owned()),
        Some(Value::Bool(true))
    );
    (abs, neg)
}

/// (unscaled, scale, negative) of a pxf.Decimal message.
fn decimal(m: &DynamicMessage) -> (Vec<u8>, i32, bool) {
    let unscaled = match m.get_field_by_name("unscaled").map(|v| v.into_owned()) {
        Some(Value::Bytes(b)) => b.to_vec(),
        _ => Vec::new(),
    };
    let scale = match m.get_field_by_name("scale").map(|v| v.into_owned()) {
        Some(Value::I32(n)) => n,
        _ => 0,
    };
    let neg = matches!(
        m.get_field_by_name("negative").map(|v| v.into_owned()),
        Some(Value::Bool(true))
    );
    (unscaled, scale, neg)
}

const TWO_POW_256_MINUS_1: &str =
    "115792089237316195423570985008687907853269984665640564039457584007913129639935";

// ---------------- BigInt ----------------

#[test]
fn big_int_basic() {
    let m = decode(&format!("big_int_field = {TWO_POW_256_MINUS_1}"), &demo());
    assert_eq!(big_int(&sub(&m, "big_int_field")), (vec![0xff; 32], false));
}

#[test]
fn big_int_negative_and_zero() {
    let m = decode("big_int_field = -42", &demo());
    assert_eq!(big_int(&sub(&m, "big_int_field")), (vec![42], true));

    // Zero has no magnitude bytes and is never negative.
    for doc in [
        "big_int_field = 0",
        "big_int_field = -0",
        "big_int_field = 000",
    ] {
        let m = decode(doc, &demo());
        let s = sub(&m, "big_int_field");
        assert_eq!(big_int(&s), (vec![], false), "{doc}");
        assert!(!s.has_field_by_name("abs"), "{doc}: abs must be unset");
        assert!(
            !s.has_field_by_name("negative"),
            "{doc}: negative must be unset"
        );
    }
    // Leading zeros are not significant.
    let m = decode("big_int_field = 007", &demo());
    assert_eq!(big_int(&sub(&m, "big_int_field")), (vec![7], false));
}

/// The wire layout the reference's `setBigIntFields` produces, byte for
/// byte: `abs` at 1, `negative` at 2, each only when non-zero.
#[test]
fn big_int_pb_bytes() {
    let m = decode("big_int_field = 255", &demo());
    assert_eq!(m.encode_to_vec(), vec![0x0a, 0x03, 0x0a, 0x01, 0xff]);
    let m = decode("big_int_field = -255", &demo());
    assert_eq!(
        m.encode_to_vec(),
        vec![0x0a, 0x05, 0x0a, 0x01, 0xff, 0x10, 0x01]
    );
}

#[test]
fn big_int_block_form_still_reads() {
    // base64 of 0xff is "/w=="
    let m = decode(
        "big_int_field { abs = b\"/w==\"\n negative = true }",
        &demo(),
    );
    assert_eq!(big_int(&sub(&m, "big_int_field")), (vec![0xff], true));
}

#[test]
fn big_int_rejects_a_non_integer_literal() {
    let err = unmarshal("big_int_field = 1.5", &demo(), UnmarshalOptions::default())
        .expect_err("a float is not a BigInt literal");
    assert!(
        err.msg
            .contains("expected '{' for message field \"big_int_field\""),
        "{}",
        err.msg
    );
}

// ---------------- Decimal ----------------

#[test]
fn decimal_basic() {
    // 2^96 - 1 with 24 fraction digits.
    let m = decode("decimal_field = 79228.162514264337593543950335", &demo());
    assert_eq!(
        decimal(&sub(&m, "decimal_field")),
        (vec![0xff; 12], 24, false)
    );
}

#[test]
fn decimal_preserves_scale() {
    let m = decode("decimal_field = 1.0", &demo());
    assert_eq!(decimal(&sub(&m, "decimal_field")), (vec![10], 1, false));
    let m = decode("decimal_field = 1.00", &demo());
    assert_eq!(decimal(&sub(&m, "decimal_field")), (vec![100], 2, false));
}

#[test]
fn decimal_integer_and_negative() {
    let m = decode("decimal_field = 42", &demo());
    let s = sub(&m, "decimal_field");
    assert_eq!(decimal(&s), (vec![42], 0, false));
    assert!(!s.has_field_by_name("scale"), "scale 0 must be unset");

    let m = decode("decimal_field = -123.45", &demo());
    assert_eq!(
        decimal(&sub(&m, "decimal_field")),
        (vec![0x30, 0x39], 2, true)
    );
}

#[test]
fn decimal_pb_bytes() {
    // unscaled 15 → 0a 01 0f; scale 1 → 10 01; negative → 18 01.
    let m = decode("decimal_field = -1.5", &demo());
    assert_eq!(
        m.encode_to_vec(),
        vec![0x12, 0x07, 0x0a, 0x01, 0x0f, 0x10, 0x01, 0x18, 0x01]
    );
    // unscaled 100, scale 2 → 1.00 keeps its trailing zeros on the wire.
    let m = decode("decimal_field = 1.00", &demo());
    assert_eq!(
        m.encode_to_vec(),
        vec![0x12, 0x05, 0x0a, 0x01, 0x64, 0x10, 0x02]
    );
}

#[test]
fn decimal_rejects_an_exponent() {
    let err = unmarshal("decimal_field = 1e5", &demo(), UnmarshalOptions::default())
        .expect_err("an exponent is not a decimal literal");
    assert!(err.msg.contains("invalid decimal: 1e5"), "{}", err.msg);
}

// ---------------- Repeated and map positions ----------------

#[test]
fn repeated_big_int_and_decimal() {
    let m = decode(
        &format!("repeated_big_int = [1, -2, {TWO_POW_256_MINUS_1}]\nrepeated_decimal = [1.0, 2.50, -0.001]"),
        &demo(),
    );
    let fd = m
        .descriptor()
        .get_field_by_name("repeated_big_int")
        .unwrap();
    let Value::List(items) = m.get_field(&fd).into_owned() else {
        panic!("not a list")
    };
    let got: Vec<(Vec<u8>, bool)> = items
        .iter()
        .map(|v| match v {
            Value::Message(sub) => big_int(sub),
            other => panic!("{other:?}"),
        })
        .collect();
    assert_eq!(
        got,
        vec![(vec![1], false), (vec![2], true), (vec![0xff; 32], false)]
    );

    let fd = m
        .descriptor()
        .get_field_by_name("repeated_decimal")
        .unwrap();
    let Value::List(items) = m.get_field(&fd).into_owned() else {
        panic!("not a list")
    };
    let got: Vec<(Vec<u8>, i32, bool)> = items
        .iter()
        .map(|v| match v {
            Value::Message(sub) => decimal(sub),
            other => panic!("{other:?}"),
        })
        .collect();
    assert_eq!(
        got,
        vec![
            (vec![10], 1, false),
            (vec![250], 2, false),
            (vec![1], 3, true)
        ]
    );
}

#[test]
fn map_values_take_the_literal_form_both_ways() {
    let d = desc("bignum_test.v1.BigNumMaps");
    let m = decode(
        "weights = {\n  alpha: 100\n  beta: -7\n}\nrates = {\n  x: 0.25\n  y: -3\n}",
        &d,
    );
    let fd = d.get_field_by_name("weights").unwrap();
    let Value::Map(w) = m.get_field(&fd).into_owned() else {
        panic!("not a map")
    };
    assert_eq!(w.len(), 2);
    let read = |k: &str| match &w[&MapKey::String(k.into())] {
        Value::Message(sub) => big_int(sub),
        other => panic!("{other:?}"),
    };
    assert_eq!(read("alpha"), (vec![100], false));
    assert_eq!(read("beta"), (vec![7], true));

    // Marshal writes the shorthand back, not a block.
    let out = marshal(&m, &d, MarshalOptions::default());
    assert!(out.contains("alpha: 100\n"), "{out}");
    assert!(out.contains("beta: -7\n"), "{out}");
    assert!(out.contains("x: 0.25\n"), "{out}");
    assert!(out.contains("y: -3\n"), "{out}");
    assert!(!out.contains("alpha: {"), "{out}");
}

// ---------------- (pxf.default) ----------------

#[test]
fn defaults_apply_to_big_int_and_decimal_fields() {
    let d = desc("bignum_test.v1.BigNumDefaults");
    let (m, _) = unmarshal_full("", &d, UnmarshalOptions::default()).expect("defaults apply");
    assert_eq!(big_int(&sub(&m, "int_with_default")), (vec![42], false));
    assert_eq!(
        decimal(&sub(&m, "dec_with_default")),
        (vec![0x01, 0x3a], 2, false)
    ); // 314
}

// ---------------- Round trips ----------------

#[test]
fn big_int_and_decimal_round_trip_through_marshal() {
    let d = demo();
    for (doc, want) in [
        (
            format!("big_int_field = -{TWO_POW_256_MINUS_1}"),
            format!("big_int_field = -{TWO_POW_256_MINUS_1}\n"),
        ),
        (
            "decimal_field = -123.456789".to_string(),
            "decimal_field = -123.456789\n".to_string(),
        ),
        (
            "decimal_field = 1.00".to_string(),
            "decimal_field = 1.00\n".to_string(),
        ),
        (
            "decimal_field = 0.05".to_string(),
            "decimal_field = 0.05\n".to_string(),
        ),
        (
            "big_int_field = 0".to_string(),
            "big_int_field = 0\n".to_string(),
        ),
    ] {
        let m = decode(&doc, &d);
        let out = marshal(&m, &d, MarshalOptions::default());
        assert_eq!(out, want, "{doc}");
        let back = decode(&out, &d);
        assert_eq!(
            back.encode_to_vec(),
            m.encode_to_vec(),
            "{doc}: pb bytes after re-read"
        );
    }
}

// ---------------- MaxNumericLiteralDigits ----------------

/// A literal of exactly MaxNumericLiteralDigits digits is within the
/// limit; one more is rejected naming it (HARDENING § Mandatory limits;
/// protowire#279's `pxf/long-numeric-4096.pxf` and `long-numeric.pxf`).
#[test]
fn digit_cap_is_inclusive() {
    let d = demo();
    let at: String = (0..MAX_NUMERIC_LITERAL_DIGITS)
        .map(|i| char::from(b'1' + (i % 9) as u8))
        .collect();
    assert_eq!(at.len(), 4096);
    let m = decode(&format!("big_int_field = {at}"), &d);
    // Decodes to that value: it reads back as the same digits.
    assert_eq!(
        marshal(&m, &d, MarshalOptions::default()),
        format!("big_int_field = {at}\n")
    );
    // The sign and the point do not count.
    decode(&format!("big_int_field = -{at}"), &d);
    decode(&format!("decimal_field = {}.{}", &at[..1], &at[1..]), &d);

    let over = format!("{at}1");
    for doc in [
        format!("big_int_field = {over}"),
        format!("decimal_field = {over}"),
        format!("decimal_field = {at}.1"),
        format!("repeated_big_int = [{over}]"),
    ] {
        let err = unmarshal(&doc, &d, UnmarshalOptions::default())
            .expect_err("4097 digits must be rejected");
        assert!(
            err.msg
                .contains("numeric literal has 4097 digits; MaxNumericLiteralDigits=4096"),
            "{}",
            err.msg
        );
    }
    let five_thousand = "9".repeat(5000);
    let err = unmarshal(
        &format!("big_int_field = {five_thousand}"),
        &d,
        UnmarshalOptions::default(),
    )
    .expect_err("5000 digits must be rejected");
    assert!(
        err.msg.contains("MaxNumericLiteralDigits=4096"),
        "{}",
        err.msg
    );
}

// ---------------- Pinned gap ----------------

/// `pxf.BigFloat` has no literal form in this port yet: matching the
/// reference's mantissa/exponent bytes for a decimal literal means
/// reproducing `big.Float`'s 256-bit rounding. Pinned so the gap is
/// visible; the block form is unaffected. Tracking issue: #39.
#[test]
fn big_float_literal_form_is_not_yet_supported() {
    let err = unmarshal("big_float_field = 42", &demo(), UnmarshalOptions::default())
        .expect_err("BigFloat literal form is a documented gap");
    assert!(
        err.msg
            .contains("expected '{' for message field \"big_float_field\""),
        "{}",
        err.msg
    );
    // The block form reads.
    let m = decode(
        "big_float_field { mantissa = b\"AQ==\"\n exponent = 3\n prec = 64 }",
        &demo(),
    );
    let s = sub(&m, "big_float_field");
    assert_eq!(
        s.get_field_by_name("exponent").unwrap().into_owned(),
        Value::I32(3)
    );
}
