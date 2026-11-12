// SPDX-License-Identifier: MIT
// Copyright (c) 2026 TrendVidia, LLC.
//! A `pxf.BigFloat` whose value, rounded to its precision, is past
//! `MaxBigFloatExponent` has no literal a reader takes, so the producer
//! fails (#111, protowire-go#261, draft -01 § Mandatory Limits). The spec's
//! `pb/bigfloat-rounds-past-bound.binpb`: a 54-bit all-ones mantissa at
//! exponent 16330 is at the bound (16384) as stored, and at prec 3 rounds
//! to 2^54, exponent 16385. protowire-go v1.17.0's check-decode refuses its
//! `pb-render` with the text pinned below.

use prost_reflect::{DescriptorPool, DynamicMessage, MessageDescriptor, Value};
use protowire_pxf::bigfloat::format_big_float;
use protowire_pxf::{marshal, unmarshal, MarshalOptions, UnmarshalOptions};

/// 2^54 - 1, big-endian.
const ALL_ONES_54: [u8; 7] = [0x3f, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff];

fn demo() -> MessageDescriptor {
    DescriptorPool::decode(&include_bytes!("../testdata/bignum-test.binpb")[..])
        .expect("bignum-test.binpb")
        .get_message_by_name("bignum_test.v1.BigNumDemo")
        .expect("BigNumDemo")
}

fn holder(exponent: i32) -> (MessageDescriptor, DynamicMessage) {
    let d = demo();
    let fd = d.get_field_by_name("big_float_field").unwrap();
    let mut bf = DynamicMessage::new(fd.kind().as_message().unwrap().clone());
    bf.set_field_by_name("mantissa", Value::Bytes(ALL_ONES_54.to_vec().into()));
    bf.set_field_by_name("exponent", Value::I32(exponent));
    bf.set_field_by_name("prec", Value::U32(3));
    let mut m = DynamicMessage::new(d.clone());
    m.set_field(&fd, Value::Message(bf));
    (d, m)
}

#[test]
fn a_value_rounded_past_the_bound_has_no_literal() {
    let want = "pxf.BigFloat binary exponent 16385 after rounding to prec 3 exceeds MaxBigFloatExponent=16384";
    assert_eq!(
        format_big_float(&ALL_ONES_54, 16330, 3, false),
        Err(want.to_string())
    );
    let (d, m) = holder(16330);
    let e = marshal(&m, &d, MarshalOptions::default()).expect_err("marshal wrote it");
    assert_eq!(e.to_string(), want);
}

/// One below, the rounded value is at the bound, 16384, and its literal reads
/// back.
#[test]
fn a_value_rounded_to_the_bound_is_written() {
    let (d, m) = holder(16329);
    let out = marshal(&m, &d, MarshalOptions::default()).expect("marshal");
    unmarshal(&out, &d, UnmarshalOptions::default()).unwrap_or_else(|e| panic!("{out}: {e}"));
}
