// SPDX-License-Identifier: MIT
// Copyright (c) 2026 TrendVidia, LLC.
//! Draft -01 § Durations (#93): a duration literal holds a signed 64-bit
//! count of nanoseconds, about 292 years. An encoder writes a Duration that
//! does not fit as a block with its exact seconds and nanos, never a longer
//! literal, and a decoder refuses a longer literal, as protowire-go does.
//! The table is protowire-go's `TestDurationAroundTheLiteralLimit`, which
//! is protowire-java's too.

use prost_reflect::{DescriptorPool, DynamicMessage, Kind, MapKey, MessageDescriptor, Value};
use protowire_pxf::{marshal, unmarshal, MarshalOptions, UnmarshalOptions};

const FDS: &[u8] = include_bytes!("../testdata/duration-block-test.binpb");

fn holder() -> MessageDescriptor {
    DescriptorPool::decode(FDS)
        .unwrap()
        .get_message_by_name("durblock.Holder")
        .unwrap()
}

fn duration(d: &MessageDescriptor, seconds: i64, nanos: i32) -> Value {
    let Kind::Message(dd) = d.get_field_by_name("dur_field").unwrap().kind() else {
        unreachable!()
    };
    let mut m = DynamicMessage::new(dd);
    m.set_field_by_name("seconds", Value::I64(seconds));
    m.set_field_by_name("nanos", Value::I32(nanos));
    Value::Message(m)
}

fn seconds_nanos(v: &Value) -> (i64, i32) {
    let m = v.as_message().unwrap();
    (
        m.get_field_by_name("seconds").unwrap().as_i64().unwrap(),
        m.get_field_by_name("nanos").unwrap().as_i32().unwrap(),
    )
}

#[test]
fn around_the_literal_limit() {
    let d = holder();
    for (seconds, nanos, text) in [
        (
            9223372036,
            854775807,
            "dur_field = 2562047h47m16.854775807s\n",
        ),
        (
            -9223372036,
            -854775808,
            "dur_field = -2562047h47m16.854775808s\n",
        ),
        (
            9223372036,
            854775808,
            "dur_field {\n  seconds = 9223372036\n  nanos = 854775808\n}\n",
        ),
        (
            -9223372036,
            -854775809,
            "dur_field {\n  seconds = -9223372036\n  nanos = -854775809\n}\n",
        ),
        (9500000000, 0, "dur_field {\n  seconds = 9500000000\n}\n"),
        (
            -9500000000,
            -5,
            "dur_field {\n  seconds = -9500000000\n  nanos = -5\n}\n",
        ),
        (
            315576000000,
            999999999,
            "dur_field {\n  seconds = 315576000000\n  nanos = 999999999\n}\n",
        ),
    ] {
        let mut m = DynamicMessage::new(d.clone());
        m.set_field_by_name("dur_field", duration(&d, seconds, nanos));
        let out = marshal(&m, &d, MarshalOptions::default()).unwrap();
        assert_eq!(out, text);
        let back = unmarshal(&out, &d, UnmarshalOptions::default()).unwrap();
        assert_eq!(
            seconds_nanos(&back.get_field_by_name("dur_field").unwrap()),
            (seconds, nanos),
            "{text}"
        );
    }
}

#[test]
fn past_the_limit_in_a_list_and_a_map() {
    let d = holder();
    let big = duration(&d, 9500000000, 1);
    let small = duration(&d, 5400, 0);
    let mut m = DynamicMessage::new(d.clone());
    m.set_field_by_name("list", Value::List(vec![small.clone(), big.clone()]));
    m.set_field_by_name(
        "by_name",
        Value::Map(
            [
                (MapKey::String("big".into()), big),
                (MapKey::String("small".into()), small),
            ]
            .into_iter()
            .collect(),
        ),
    );
    let out = marshal(&m, &d, MarshalOptions::default()).unwrap();
    assert_eq!(
        out,
        "list = [\n  1h30m0s,\n  {\n    seconds = 9500000000\n    nanos = 1\n  }\n]\n\
         by_name = {\n  big: {\n    seconds = 9500000000\n    nanos = 1\n  }\n  small: 1h30m0s\n}\n"
    );
    let back = unmarshal(&out, &d, UnmarshalOptions::default()).unwrap();
    let list = back.get_field_by_name("list").unwrap();
    let list = list.as_list().unwrap();
    assert_eq!(seconds_nanos(&list[1]), (9500000000, 1));
    let map = back.get_field_by_name("by_name").unwrap();
    let big = &map.as_map().unwrap()[&MapKey::String("big".into())];
    assert_eq!(seconds_nanos(big), (9500000000, 1));
}

/// A longer literal is refused, as protowire-go's `time.ParseDuration`
/// refuses it, and an enormous one does not overflow. Go's lexer refuses
/// each, so the text is its `invalid duration` (#153).
#[test]
fn a_literal_past_the_limit_is_refused() {
    let d = holder();
    let read = |lit: &str| {
        unmarshal(
            &format!("dur_field = {lit}"),
            &d,
            UnmarshalOptions::default(),
        )
    };
    // The bounds themselves read.
    for lit in ["2562047h47m16.854775807s", "-2562047h47m16.854775808s"] {
        read(lit).unwrap_or_else(|e| panic!("{lit}: {e}"));
    }
    for lit in [
        "2562047h47m16.854775808s",
        "-2562047h47m16.854775809s",
        "2638888h53m20s",
        "99999999999999999999999999999h",
        "9223372036854775807h",
        "1h99999999999999999999999999999999999ns",
    ] {
        let e = read(lit).expect_err(lit).to_string();
        assert_eq!(e, format!("1:13: invalid duration: {lit}"));
    }
}
