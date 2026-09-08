// SPDX-License-Identifier: MIT
// Copyright (c) 2026 TrendVidia, LLC.
//! Every `google.protobuf.Duration` the encoder writes must read back
//! (protowire-rust#26). The encoder mirrors Go's `time.Duration.String()`,
//! which writes fractions and `µs` for any value that is not a whole
//! multiple of its largest unit — i.e. every measured latency — and until
//! #26 the lexer could read neither, so this port could not read its own
//! output. Mirrors protowire-go's `TestMarshalDurationReadsBack`.

use prost_reflect::{DescriptorPool, DynamicMessage, MessageDescriptor, ReflectMessage, Value};
use protowire_pxf::{marshal, unmarshal, MarshalOptions, UnmarshalOptions};

const TEST_FDS: &[u8] = include_bytes!("../testdata/test.binpb");

fn all_types() -> MessageDescriptor {
    DescriptorPool::decode(TEST_FDS)
        .expect("decode test.binpb")
        .get_message_by_name("test.v1.AllTypes")
        .expect("test.v1.AllTypes")
}

const NANOS_PER_SECOND: i128 = 1_000_000_000;

/// (total nanoseconds, what `time.Duration.String()` prints for it).
const CASES: &[(i128, &str)] = &[
    (0, "0s"),
    (1, "1ns"),
    (999, "999ns"),
    (1_000, "1µs"),
    (1_234, "1.234µs"),
    (1_500, "1.5µs"),
    (312_500, "312.5µs"),
    (1_234_567, "1.234567ms"),
    (1_500_000, "1.5ms"),
    (250_000_000, "250ms"),
    (1_000_000_000, "1s"),
    (1_500_000_000, "1.5s"),
    (90 * 60 * NANOS_PER_SECOND, "1h30m0s"),
    (90 * 60 * NANOS_PER_SECOND + 500_000_000, "1h30m0.5s"),
    (
        (3600 + 30 * 60 + 45) * NANOS_PER_SECOND + 123_456_789,
        "1h30m45.123456789s",
    ),
    (100 * 3600 * NANOS_PER_SECOND, "100h0m0s"),
    (-1, "-1ns"),
    (-1_500, "-1.5µs"),
    (-312_500, "-312.5µs"),
    (-1_500_000_000, "-1.5s"),
    (-(90 * 60 * NANOS_PER_SECOND + 500_000_000), "-1h30m0.5s"),
    (i64::MAX as i128, "2562047h47m16.854775807s"),
    (i64::MIN as i128, "-2562047h47m16.854775808s"),
];

#[test]
fn marshal_duration_reads_back() {
    let desc = all_types();
    let dur_fd = desc.get_field_by_name("dur_field").unwrap();
    let dur_desc = match dur_fd.kind() {
        prost_reflect::Kind::Message(m) => m,
        _ => unreachable!(),
    };
    let secs_fd = dur_desc.get_field_by_name("seconds").unwrap();
    let nanos_fd = dur_desc.get_field_by_name("nanos").unwrap();

    for &(total, want_text) in CASES {
        // Same sign on both fields, per google.protobuf.Duration.
        let secs = (total / NANOS_PER_SECOND) as i64;
        let nanos = (total % NANOS_PER_SECOND) as i32;

        let mut sub = DynamicMessage::new(dur_desc.clone());
        sub.set_field(&secs_fd, Value::I64(secs));
        sub.set_field(&nanos_fd, Value::I32(nanos));
        let mut msg = DynamicMessage::new(desc.clone());
        msg.set_field(&dur_fd, Value::Message(sub));

        let text = marshal(&msg, &desc, MarshalOptions::default());
        assert!(
            text.contains(&format!("dur_field = {want_text}\n")),
            "{total}: encoder wrote {text:?}, want {want_text:?}"
        );

        let back = unmarshal(&text, &desc, UnmarshalOptions::default())
            .unwrap_or_else(|e| panic!("{total}: reading back {text:?}: {e}"));
        let got = match back.get_field(&dur_fd).into_owned() {
            Value::Message(m) => m,
            other => panic!("{total}: dur_field is {other:?}"),
        };
        assert_eq!(
            got.get_field(&secs_fd).into_owned(),
            Value::I64(secs),
            "{total}: seconds"
        );
        assert_eq!(
            got.get_field(&nanos_fd).into_owned(),
            Value::I32(nanos),
            "{total}: nanos"
        );
    }
}

/// The decoder's own duration parser takes the `µs` unit and a fraction in
/// any segment, in the forms other ports' encoders write.
#[test]
fn decoder_reads_micro_and_fractional_segments() {
    let desc = all_types();
    let dur_fd = desc.get_field_by_name("dur_field").unwrap();
    for (doc, secs, nanos) in [
        ("dur_field = 2µs", 0i64, 2_000i32),
        ("dur_field = 312.5µs", 0, 312_500),
        ("dur_field = 1.5ms", 0, 1_500_000),
        ("dur_field = 1.5h", 5400, 0),
        ("dur_field = 1h30m0.5s", 5400, 500_000_000),
        ("dur_field = -2.5s", -2, -500_000_000),
    ] {
        let msg = unmarshal(doc, &desc, UnmarshalOptions::default())
            .unwrap_or_else(|e| panic!("{doc}: {e}"));
        let got = match msg.get_field(&dur_fd).into_owned() {
            Value::Message(m) => m,
            other => panic!("{doc}: {other:?}"),
        };
        let s = got
            .get_field(&got.descriptor().get_field_by_name("seconds").unwrap())
            .into_owned();
        let n = got
            .get_field(&got.descriptor().get_field_by_name("nanos").unwrap())
            .into_owned();
        assert_eq!((s, n), (Value::I64(secs), Value::I32(nanos)), "{doc}");
    }
}
