// SPDX-License-Identifier: MIT
// Copyright (c) 2026 TrendVidia, LLC.
//! A tag's wire type, as protobuf handles it (#204, protowire#614).
//!
//! A decoder rejects wire type 6 or 7 on any field. It reads a known field
//! whose wire type does not match its type as an unknown field: skipped by
//! its own wire type, with the field left as it was, and refused when the
//! skip runs out of bytes. These are protowire-go's tests of the rule
//! (`encoding/pb/wire_type_test.go`), with the error texts and the
//! re-encoded values protowire-go v1.19.0 (a0a78c2) gives for each input.
//! The full matrix, every wire type at every field, is in
//! `crates/protowire/tests/pb_error_oracle.rs`.

use protowire_pb::{
    marshal, read_message, unmarshal, write_big_float, write_big_int, write_decimal, BigFloat,
    BigInt, Decimal, Message, Reader, Result, WireType, Writer,
};

fn unhex(s: &str) -> Vec<u8> {
    (0..s.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(&s[i..i + 2], 16).expect("hex"))
        .collect()
}

fn hex(b: &[u8]) -> String {
    b.iter().map(|b| format!("{b:02x}")).collect()
}

#[derive(Default, Debug)]
struct I64Holder {
    v: i64,
}

impl Message for I64Holder {
    fn encode_to(&self, _: &mut Writer) {}
    fn merge_field(&mut self, num: u32, wt: WireType, r: &mut Reader<'_>) -> Result<()> {
        match num {
            1 => self.v = r.int64()?,
            _ => r.skip(wt)?,
        }
        Ok(())
    }
    fn field_name(num: u32) -> Option<&'static str> {
        (num == 1).then_some("V")
    }
}

#[derive(Default, Debug)]
struct StrHolder {
    v: String,
}

impl Message for StrHolder {
    fn encode_to(&self, _: &mut Writer) {}
    fn merge_field(&mut self, num: u32, wt: WireType, r: &mut Reader<'_>) -> Result<()> {
        match num {
            1 => self.v = r.string()?,
            _ => r.skip(wt)?,
        }
        Ok(())
    }
    fn field_name(num: u32) -> Option<&'static str> {
        (num == 1).then_some("V")
    }
}

/// The rows of #204's table, and wire types 6 and 7 on a known field.
#[test]
fn issue_rows() {
    // (name, input, None for an accept, or protowire-go's error)
    let rows: &[(&str, &str, Option<&str>)] = &[
        (
            "int64, wire type 6",
            "0e01",
            Some("corrupt field 1: proto: cannot parse reserved wire type"),
        ),
        ("int64 sent as wire type 2", "0a0101", None),
        ("int64 sent as wire type 5", "0d01000000", None),
        (
            "int64 as wire type 2, cut short",
            "0a01",
            Some("corrupt field 1: unexpected EOF"),
        ),
        (
            "int64 as wire type 5, cut short",
            "0d01",
            Some("corrupt field 1: unexpected EOF"),
        ),
    ];
    for (name, input, want) in rows {
        let got = unmarshal::<I64Holder>(&unhex(input));
        match (want, got) {
            (None, Ok(m)) => assert_eq!(m.v, 0, "{name}: the field stays as it was"),
            (Some(want), Err(e)) => assert_eq!(e.to_string(), *want, "{name}"),
            (want, got) => panic!("{name}: want {want:?}, got {got:?}"),
        }
    }
    let rows: &[(&str, &str, Option<&str>)] = &[
        (
            "string, wire type 7",
            "0f0161",
            Some("corrupt field 1: proto: cannot parse reserved wire type"),
        ),
        (
            "string, wire type 6",
            "0e0161",
            Some("corrupt field 1: proto: cannot parse reserved wire type"),
        ),
        ("string sent as wire type 0", "0801", None),
    ];
    for (name, input, want) in rows {
        let got = unmarshal::<StrHolder>(&unhex(input));
        match (want, got) {
            (None, Ok(m)) => assert_eq!(m.v, "", "{name}: the field stays as it was"),
            (Some(want), Err(e)) => assert_eq!(e.to_string(), *want, "{name}"),
            (want, got) => panic!("{name}: want {want:?}, got {got:?}"),
        }
    }
}

/// A skipped field is not touched: an optional field stays unset and a
/// list gets no element, and the field after it is still read.
#[test]
fn mismatch_leaves_field_alone() {
    #[derive(Default, Debug)]
    struct Holder {
        p: Option<i64>,
        l: Vec<String>,
        kept: String,
    }
    impl Message for Holder {
        fn encode_to(&self, _: &mut Writer) {}
        fn merge_field(&mut self, num: u32, wt: WireType, r: &mut Reader<'_>) -> Result<()> {
            match num {
                1 => self.p = Some(r.int64()?),
                2 => {
                    let v = r.string()?;
                    r.push_element(&mut self.l, v)?;
                }
                3 => self.kept = r.string()?,
                _ => r.skip(wt)?,
            }
            Ok(())
        }
    }
    let m: Holder = unmarshal(&unhex(concat!("0d01000000", "1001", "1a0161"))).unwrap();
    assert_eq!(m.p, None);
    assert!(m.l.is_empty());
    assert_eq!(m.kept, "a");

    // A matching record after a mismatched one still sets the field, and a
    // mismatched one after a match leaves it.
    let m: I64Holder = unmarshal(&unhex("1007")).unwrap();
    assert_eq!(m.v, 0, "field 2 is unknown");
    let m: I64Holder = unmarshal(&unhex(concat!("0807", "0d01000000"))).unwrap();
    assert_eq!(m.v, 7);
    let m: I64Holder = unmarshal(&unhex(concat!("0a0101", "0807"))).unwrap();
    assert_eq!(m.v, 7);
}

#[derive(Default, Debug)]
struct Big {
    i: Option<BigInt>,
    r: Option<Decimal>,
    f: Option<BigFloat>,
}

impl Message for Big {
    fn encode_to(&self, w: &mut Writer) {
        if let Some(v) = &self.i {
            write_big_int(w, 1, v);
        }
        if let Some(v) = &self.r {
            write_decimal(w, 2, v);
        }
        if let Some(v) = &self.f {
            write_big_float(w, 3, v);
        }
    }
    fn merge_field(&mut self, num: u32, wt: WireType, r: &mut Reader<'_>) -> Result<()> {
        match num {
            1 => self.i = Some(read_message(r)?),
            2 => self.r = Some(read_message(r)?),
            3 => self.f = Some(read_message(r)?),
            _ => r.skip(wt)?,
        }
        Ok(())
    }
    fn field_name(num: u32) -> Option<&'static str> {
        ["I", "R", "F"].get(num.checked_sub(1)? as usize).copied()
    }
}

/// The big-number messages follow the same rule: a field whose wire type
/// does not match is skipped, and wire types 6 and 7 are refused. On an
/// accept, the value is checked by what it encodes to, which is what
/// protowire-go's `Marshal` writes for its decoded value.
#[test]
fn big_numbers() {
    let rows: &[(&str, &str, &str)] = &[
        // BigInt{abs: 5, negative sent as wire type 2}: negative is skipped.
        (
            "BigInt negative as bytes",
            "0a060a0105120101",
            "OK 0a030a0105",
        ),
        // BigInt{abs sent as a varint}: abs is skipped, the value is 0.
        ("BigInt abs as varint", "0a020805", "OK "),
        // Decimal{unscaled: 15, scale sent as fixed32}: scale stays 0.
        (
            "Decimal scale as fixed32",
            "12080a010f1501000000",
            "OK 12030a010f",
        ),
        // BigFloat{mantissa: 1, exponent sent as bytes, prec 53}.
        (
            "BigFloat exponent as bytes",
            "1a080a01011201011835",
            "OK 1a160a071000000000000010ccffffffffffffffff011835",
        ),
        (
            "BigInt, wire type 6",
            "0a020e01",
            "ERR field I: corrupt field 1 in BigInt: proto: cannot parse reserved wire type",
        ),
        (
            "Decimal, wire type 7",
            "12031f0101",
            "ERR field R: corrupt field 3 in Decimal: proto: cannot parse reserved wire type",
        ),
        (
            "BigFloat, wire type 6",
            "1a022601",
            "ERR field F: corrupt field 4 in BigFloat: proto: cannot parse reserved wire type",
        ),
        (
            "BigInt, a group ended by another field",
            "0a030b0c0c",
            "ERR field I: corrupt field 1 in BigInt: proto: mismatching end group marker",
        ),
    ];
    for (name, input, want) in rows {
        let got = match unmarshal::<Big>(&unhex(input)) {
            Ok(m) => format!("OK {}", hex(&marshal(&m).unwrap())),
            Err(e) => format!("ERR {e}"),
        };
        assert_eq!(got, *want, "{name}");
    }
}
