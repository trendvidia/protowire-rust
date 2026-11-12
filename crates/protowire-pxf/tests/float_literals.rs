// SPDX-License-Identifier: MIT
// Copyright (c) 2026 TrendVidia, LLC.
//! Draft -01 § Scalar Values (#96): float and double fields accept literals
//! in the IEEE 754 range; a literal that rounds to an infinity is rejected
//! unless written as `inf`, `+inf` or `-inf`; and `nan` is stored as the
//! canonical quiet NaN, 0x7FF8000000000000 or 0x7FC00000. The byte
//! expectations are protowire-go's goldens for the spec's write cases
//! `Scalars.float-inf`, `float-nan`, `double-overflow`, `float-overflow`.

use prost::Message as _;
use prost_reflect::{DescriptorPool, DynamicMessage, MessageDescriptor};
use protowire_pxf::{marshal, unmarshal, unmarshal_full, MarshalOptions, UnmarshalOptions};

const FDS: &[u8] = include_bytes!("../testdata/float-literals-test.binpb");

fn desc(name: &str) -> MessageDescriptor {
    DescriptorPool::decode(FDS)
        .unwrap()
        .get_message_by_name(name)
        .unwrap()
}

fn decode(doc: &str) -> Result<DynamicMessage, String> {
    unmarshal(doc, &desc("floatlit.v1.F"), UnmarshalOptions::default()).map_err(|e| e.to_string())
}

fn f64_bits(m: &DynamicMessage, name: &str) -> u64 {
    m.get_field_by_name(name)
        .unwrap()
        .as_f64()
        .unwrap()
        .to_bits()
}

fn f32_bits(m: &DynamicMessage, name: &str) -> u32 {
    m.get_field_by_name(name)
        .unwrap()
        .as_f32()
        .unwrap()
        .to_bits()
}

fn hex(b: &[u8]) -> String {
    b.iter().map(|x| format!("{x:02x}")).collect()
}

#[test]
fn infinities_are_read() {
    // Scalars.float-inf: protowire-go writes 09 000000000000f07f 15 000080ff.
    let m = decode("d = inf f = -inf").unwrap();
    assert_eq!(hex(&m.encode_to_vec()), "09000000000000f07f15000080ff");
    let m = decode("d = +inf f = inf").unwrap();
    assert_eq!(f64_bits(&m, "d"), f64::INFINITY.to_bits());
    assert_eq!(f32_bits(&m, "f"), f32::INFINITY.to_bits());
    let m = decode("d = -inf").unwrap();
    assert_eq!(f64_bits(&m, "d"), f64::NEG_INFINITY.to_bits());
}

#[test]
fn nan_is_stored_canonically() {
    // Scalars.float-nan: protowire-go writes 09 000000000000f87f 15 0000c07f.
    let m = decode("d = nan f = nan").unwrap();
    assert_eq!(f64_bits(&m, "d"), 0x7FF8_0000_0000_0000);
    assert_eq!(f32_bits(&m, "f"), 0x7FC0_0000);
    assert_eq!(hex(&m.encode_to_vec()), "09000000000000f87f150000c07f");
}

#[test]
fn a_literal_that_rounds_to_infinity_is_rejected() {
    // Scalars.double-overflow and Scalars.float-overflow: protowire-go
    // rejects both with these messages.
    let e = decode("d = 1e400").unwrap_err();
    assert!(e.contains("invalid double: 1e400"), "{e}");
    let e = decode("f = 3.4028236e38").unwrap_err();
    assert!(e.contains("invalid float: 3.4028236e38"), "{e}");
    let e = decode("d = -1e400").unwrap_err();
    assert!(e.contains("invalid double: -1e400"), "{e}");
    // The largest finite values are in range.
    let m = decode("d = 1.7976931348623157e308 f = 3.4028235e38").unwrap();
    assert_eq!(f64_bits(&m, "d"), f64::MAX.to_bits());
    assert_eq!(f32_bits(&m, "f"), f32::MAX.to_bits());
}

#[test]
fn identifiers_in_lists_and_map_values() {
    let m = decode("ds = [inf, -inf, nan, +inf] fs = [nan] md = { a: -inf b: nan }").unwrap();
    let ds: Vec<u64> = m
        .get_field_by_name("ds")
        .unwrap()
        .as_list()
        .unwrap()
        .iter()
        .map(|v| v.as_f64().unwrap().to_bits())
        .collect();
    assert_eq!(
        ds,
        [
            f64::INFINITY.to_bits(),
            f64::NEG_INFINITY.to_bits(),
            0x7FF8_0000_0000_0000,
            f64::INFINITY.to_bits()
        ]
    );
    assert!(decode("ds = [1e400]").is_err());
    assert!(decode("md = { a: 1e999 }").is_err());
}

#[test]
fn a_sign_goes_only_on_a_number_or_inf() {
    // '+' is valid only in "+inf".
    assert!(decode("d = +1").is_err());
    assert!(decode("d = +nan").is_err());
    assert!(decode("d = -nan").is_err());
    // An identifier that starts with inf is not inf.
    assert!(decode("d = infinity").is_err());
    assert!(decode("d = -infx").is_err());
}

#[test]
fn what_marshal_writes_reads_back() {
    let d = desc("floatlit.v1.F");
    let m = decode("d = -inf f = nan ds = [inf, nan]").unwrap();
    let text = marshal(&m, &d, MarshalOptions::default()).unwrap();
    let back = unmarshal(&text, &d, UnmarshalOptions::default()).unwrap();
    assert_eq!(back.encode_to_vec(), m.encode_to_vec(), "{text}");
}

#[test]
fn defaults_take_the_same_literals() {
    let d = desc("floatlit.v1.Defaults");
    let (m, _) = unmarshal_full("", &d, UnmarshalOptions::default()).unwrap();
    assert_eq!(f64_bits(&m, "d_nan"), 0x7FF8_0000_0000_0000);
    assert_eq!(f32_bits(&m, "f_neg_inf"), f32::NEG_INFINITY.to_bits());
    assert_eq!(f64_bits(&m, "d_pos_inf"), f64::INFINITY.to_bits());

    let bad = desc("floatlit.v1.BadDefault");
    let set = unmarshal_full("d = 1", &bad, UnmarshalOptions::default()).map(|(m, _)| m);
    let unset = unmarshal_full("", &bad, UnmarshalOptions::default()).map(|(m, _)| m);
    // The default is refused where it is applied; before #96 it became
    // +Inf without a word.
    assert!(set.is_ok());
    let e = unset.unwrap_err().to_string();
    assert!(e.contains(r#"invalid default double "1e400""#), "{e}");
}

// The spec's text cases Scalars.float-negzero, float-large and float-small
// (testdata/text/cases): protowire-go writes `-0` for a negative zero, which
// protobuf-go counts as set, and an exponent where strconv's 'g' form has
// one. A negative zero read back keeps its sign.
#[test]
fn marshal_writes_floats_as_protowire_go_does() {
    let d = desc("floatlit.v1.F");
    let opts = MarshalOptions::default;
    let m = decode("d = -0 f = -0").unwrap();
    assert_eq!(marshal(&m, &d, opts()).unwrap(), "d = -0\nf = -0\n");
    let back = decode(&marshal(&m, &d, opts()).unwrap()).unwrap();
    assert_eq!(f64_bits(&back, "d"), (-0.0f64).to_bits());
    assert_eq!(f32_bits(&back, "f"), (-0.0f32).to_bits());

    let m = decode("d = 0 f = 0").unwrap();
    assert_eq!(marshal(&m, &d, opts()).unwrap(), "");

    let m = decode("d = 1e21 f = 1e10").unwrap();
    assert_eq!(marshal(&m, &d, opts()).unwrap(), "d = 1e+21\nf = 1e+10\n");
    let m = decode("d = 0.0000001 f = 0.000001").unwrap();
    assert_eq!(marshal(&m, &d, opts()).unwrap(), "d = 1e-07\nf = 1e-06\n");
    let back = decode("d = 1e-07 f = 1e-06").unwrap();
    assert_eq!(f64_bits(&back, "d"), 1e-7f64.to_bits());
    assert_eq!(f32_bits(&back, "f"), 1e-6f32.to_bits());
}
