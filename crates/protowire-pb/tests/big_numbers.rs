// SPDX-License-Identifier: MIT
// Copyright (c) 2026 TrendVidia, LLC.
//! `pxf.BigInt`, `pxf.Decimal` and `pxf.BigFloat` fields (#142), ported from
//! protowire-go's `encoding/pb` tests: `bigfloat_zero_test.go` and the
//! big-number half of `limits_test.go`. Every expected byte string and
//! error text is the one those tests expect of protowire-go.
//!
//! A value's binary exponent, Go's `MantExp`, is `exponent + bit length of
//! mantissa` here. Go's "infinite BigFloat" case has no counterpart: this
//! type cannot hold an infinity.

use protowire_pb::wire::{Limits, Reader, Result, WireType, Writer};
use protowire_pb::{
    marshal, read_message, unmarshal, unmarshal_with, write_big_float, write_decimal,
    write_message, BigFloat, Decimal, Message,
};

#[derive(Default, Debug)]
struct BigFloatHolder {
    f: Option<BigFloat>,
}
impl Message for BigFloatHolder {
    fn encode_to(&self, w: &mut Writer) {
        if let Some(f) = &self.f {
            write_big_float(w, 1, f);
        }
    }
    fn merge_field(&mut self, num: u32, wt: WireType, r: &mut Reader<'_>) -> Result<()> {
        match num {
            1 => self.f = Some(read_message(r)?),
            _ => r.skip(wt)?,
        }
        Ok(())
    }
}

#[derive(Default, Debug)]
struct BigFloatList {
    f: Vec<BigFloat>,
}
impl Message for BigFloatList {
    fn encode_to(&self, w: &mut Writer) {
        for f in &self.f {
            write_message(w, 1, f);
        }
    }
    fn merge_field(&mut self, num: u32, wt: WireType, r: &mut Reader<'_>) -> Result<()> {
        match num {
            1 => {
                let f = read_message(r)?;
                r.push_element(&mut self.f, f)?;
            }
            _ => r.skip(wt)?,
        }
        Ok(())
    }
}

#[derive(Default, Debug)]
struct DecimalHolder {
    d: Option<Decimal>,
}
impl Message for DecimalHolder {
    fn encode_to(&self, w: &mut Writer) {
        if let Some(d) = &self.d {
            write_decimal(w, 1, d);
        }
    }
    fn merge_field(&mut self, num: u32, wt: WireType, r: &mut Reader<'_>) -> Result<()> {
        match num {
            1 => self.d = Some(read_message(r)?),
            _ => r.skip(wt)?,
        }
        Ok(())
    }
}

fn varint(out: &mut Vec<u8>, mut v: u64) {
    while v >= 0x80 {
        out.push((v as u8) | 0x80);
        v >>= 7;
    }
    out.push(v as u8);
}

fn field_bytes(out: &mut Vec<u8>, num: u32, b: &[u8]) {
    varint(out, u64::from(num << 3 | 2));
    varint(out, b.len() as u64);
    out.extend_from_slice(b);
}

fn field_varint(out: &mut Vec<u8>, num: u32, v: u64) {
    varint(out, u64::from(num << 3));
    varint(out, v);
}

/// Go's `decimalWire`: unscaled 25 at `scale`, in field 1.
fn decimal_wire(scale: i32) -> Vec<u8> {
    let mut sub = Vec::new();
    field_bytes(&mut sub, 1, &[25]);
    field_varint(&mut sub, 2, i64::from(scale) as u64);
    let mut out = Vec::new();
    field_bytes(&mut out, 1, &sub);
    out
}

/// Go's `bigFloatWire`.
fn big_float_wire(mant: &[u8], exp: i32, prec: u32) -> Vec<u8> {
    let mut sub = Vec::new();
    field_bytes(&mut sub, 1, mant);
    field_varint(&mut sub, 2, i64::from(exp) as u64);
    field_varint(&mut sub, 3, u64::from(prec));
    let mut out = Vec::new();
    field_bytes(&mut out, 1, &sub);
    out
}

fn read_float(wire: &[u8]) -> std::result::Result<BigFloat, String> {
    read_float_with(wire, Limits::default())
}

fn read_float_with(wire: &[u8], limits: Limits) -> std::result::Result<BigFloat, String> {
    unmarshal_with::<BigFloatHolder>(wire, limits)
        .map(|h| h.f.expect("field set"))
        .map_err(|e| e.to_string())
}

fn bit_len(mag: &[u8]) -> i64 {
    let first = mag.iter().position(|b| *b != 0).unwrap_or(mag.len());
    let mag = &mag[first..];
    match mag.first() {
        None => 0,
        Some(top) => (mag.len() as i64 - 1) * 8 + i64::from(8 - top.leading_zeros()),
    }
}

/// Go's `MantExp`: the value's binary exponent.
fn mant_exp(f: &BigFloat) -> i64 {
    i64::from(f.exponent) + bit_len(&f.mantissa)
}

fn marshal_err<M: Message>(m: &M) -> String {
    match marshal(m) {
        Ok(b) => panic!("marshal wrote {b:02x?}, want an error"),
        Err(e) => e.to_string(),
    }
}

/// `5^n` as a big-endian magnitude: the unscaled value of 1/2^n.
fn pow5(n: u32) -> Vec<u8> {
    let mut mag = vec![1u8];
    for _ in 0..n {
        let mut carry = 0u16;
        for b in mag.iter_mut().rev() {
            let v = u16::from(*b) * 5 + carry;
            *b = v as u8;
            carry = v >> 8;
        }
        if carry != 0 {
            mag.insert(0, carry as u8);
        }
    }
    mag
}

const MAX_DIGITS: i32 = 4096;
const MAX_EXP: i32 = 16384;

// ---- bigfloat_zero_test.go ----

// A prec-0 BigFloat is ±0, and PXF writes the two apart, "0" and "-0"
// (protowire#410). -0 is {negative: true} with no precision.
#[test]
fn prec_zero_keeps_its_sign() {
    for (wire, neg) in [
        (vec![0x0a, 0x00], false),
        (vec![0x0a, 0x02, 0x20, 0x01], true),
    ] {
        let f = read_float(&wire).unwrap();
        assert_eq!(
            (f.prec, f.negative, f.is_zero()),
            (0, neg, true),
            "{wire:02x?}"
        );
    }
    let neg = BigFloatHolder {
        f: Some(BigFloat {
            negative: true,
            ..Default::default()
        }),
    };
    let out = marshal(&neg).unwrap();
    assert_eq!(out, [0x0a, 0x02, 0x20, 0x01]);
    let back = read_float(&out).unwrap();
    assert!(back.negative && back.prec == 0);
    // +0 at prec 0 stays the omitted field it was.
    let zero = BigFloatHolder {
        f: Some(BigFloat::default()),
    };
    assert_eq!(marshal(&zero).unwrap(), Vec::<u8>::new());
}

// A zero at any prec is written with an empty mantissa and exponent 0,
// keeping prec and negative (draft -01 § PXF Annotations).
#[test]
fn zero_has_one_encoding() {
    for (neg, want) in [
        (false, vec![0x0a, 0x02, 0x18, 0x40]),
        (true, vec![0x0a, 0x04, 0x18, 0x40, 0x20, 0x01]),
    ] {
        let z = BigFloat {
            mantissa: vec![0, 0],
            exponent: -64,
            prec: 64,
            negative: neg,
        };
        let out = marshal(&BigFloatHolder { f: Some(z) }).unwrap();
        assert_eq!(out, want, "negative {neg}");
        let back = read_float(&out).unwrap();
        assert_eq!((back.is_zero(), back.prec, back.negative), (true, 64, neg));
    }
    // A zero mantissa with any exponent still reads as zero.
    let old = [
        0x0a, 0x0f, 0x10, 0xc0, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0x01, 0x18, 0x40,
        0x20, 0x01,
    ];
    let f = read_float(&old).unwrap();
    assert_eq!(
        (f.is_zero(), f.prec, f.negative, f.exponent),
        (true, 64, true, 0)
    );
}

// A zero's prec is bounded as any BigFloat's is (protowire#499): refused at
// 2^32-1 bits on writing and on reading, round-tripped at 13603.
#[test]
fn zero_at_max_prec() {
    for neg in [false, true] {
        let at_max = BigFloat {
            prec: u32::MAX,
            negative: neg,
            ..Default::default()
        };
        let err = marshal_err(&BigFloatHolder { f: Some(at_max) });
        assert!(err.contains("MaxNumericLiteralDigits"), "{err}");
        let mut old = vec![0x0a, 0x06, 0x18, 0xff, 0xff, 0xff, 0xff, 0x0f];
        if neg {
            old = vec![0x0a, 0x08, 0x18, 0xff, 0xff, 0xff, 0xff, 0x0f, 0x20, 0x01];
        }
        let err = read_float(&old).unwrap_err();
        assert!(err.contains("MaxNumericLiteralDigits"), "{err}");

        let at_bound = BigFloat {
            prec: 13603,
            negative: neg,
            ..Default::default()
        };
        let out = marshal(&BigFloatHolder { f: Some(at_bound) }).unwrap();
        let back = read_float(&out).unwrap();
        assert_eq!(
            (back.is_zero(), back.prec, back.negative),
            (true, 13603, neg)
        );
    }
}

// ---- limits_test.go ----

#[test]
fn decimal_scale_bound() {
    for scale in [MAX_DIGITS, -MAX_DIGITS] {
        let h: DecimalHolder = unmarshal(&decimal_wire(scale)).unwrap();
        let d = h.d.unwrap();
        assert_eq!((d.unscaled, d.scale), (vec![25], scale));
    }
    for scale in [MAX_DIGITS + 1, -MAX_DIGITS - 1, i32::MAX, i32::MIN] {
        let err = unmarshal::<DecimalHolder>(&decimal_wire(scale))
            .unwrap_err()
            .to_string();
        assert_eq!(
            err,
            format!("pxf.Decimal scale {scale} exceeds MaxNumericLiteralDigits=4096")
        );
    }
}

#[test]
fn big_float_exponent_bound() {
    // 25 has 5 bits, so the value's binary exponent is exp + 5, and the
    // bound is on that.
    for exp in [-MAX_EXP - 5, -1, 0, 1, MAX_EXP - 5] {
        let f = read_float(&big_float_wire(&[25], exp, 64)).unwrap();
        assert_eq!(mant_exp(&f), i64::from(exp) + 5, "exponent {exp}");
    }
    for exp in [i32::MIN, -MAX_EXP - 6, MAX_EXP - 4, i32::MAX] {
        let err = read_float(&big_float_wire(&[25], exp, 64)).unwrap_err();
        assert!(err.contains("MaxBigFloatExponent"), "exponent {exp}: {err}");
    }
    // The bound is a per-call limit.
    let ten = Limits {
        max_big_float_exponent: 10,
        ..Limits::default()
    };
    let err = read_float_with(&big_float_wire(&[25], 6, 64), ten).unwrap_err();
    assert_eq!(
        err,
        "pxf.BigFloat binary exponent 11 exceeds MaxBigFloatExponent=10"
    );
    read_float_with(&big_float_wire(&[25], 5, 64), ten).unwrap();
}

#[test]
fn big_float_exponent_bound_edges() {
    // A zero mantissa is zero at any exponent.
    for mant in [&[][..], &[0, 0][..]] {
        for exp in [i32::MIN, i32::MAX] {
            let f = read_float(&big_float_wire(mant, exp, 64)).unwrap();
            assert!(f.is_zero(), "mantissa {mant:02x?}, exponent {exp}");
        }
    }
    // prec 0 is zero at any exponent.
    let f = read_float(&big_float_wire(&[1], i32::MIN, 0)).unwrap();
    assert!(f.is_zero());
    // Leading zero bytes are not mantissa bits.
    read_float(&big_float_wire(&[0, 0, 1], MAX_EXP - 1, 64)).unwrap();
    let err = read_float(&big_float_wire(&[0, 0, 1], MAX_EXP, 64)).unwrap_err();
    assert!(err.contains("MaxBigFloatExponent"), "{err}");
    // A bound raised past big.Float's range still refuses an infinity
    // (protowire#278).
    let raised = Limits {
        max_big_float_exponent: usize::MAX >> 1,
        ..Limits::default()
    };
    let err = read_float_with(&big_float_wire(&[25], i32::MAX, 64), raised).unwrap_err();
    assert!(err.contains("overflows"), "{err}");
    // Pinned divergence, as in protowire-go: the bound is on the wire's
    // mantissa and exponent (protowire#281), and rounding to prec can carry
    // one past it. 0xFF at one bit rounds up to 0x100. The value read is
    // one past the bound, and marshal refuses it. This fails the day the
    // reader checks the rounded value instead.
    let f = read_float(&big_float_wire(&[0xff], MAX_EXP - 8, 1)).unwrap();
    assert_eq!(mant_exp(&f), i64::from(MAX_EXP) + 1);
    let err = marshal_err(&BigFloatHolder { f: Some(f) });
    assert!(err.contains("MaxBigFloatExponent"), "{err}");
}

#[test]
fn big_float_prec_bound() {
    // 0x19 × 2^-3 = 3.125.
    let f = read_float(&big_float_wire(&[0x19], -3, 13603)).unwrap();
    assert_eq!(
        (f.mantissa.as_slice(), f.exponent, f.prec),
        (&[0x19][..], -3, 13603)
    );
    for prec in [13604, 1 << 27, i32::MAX as u32, u32::MAX] {
        let err = read_float(&big_float_wire(&[0x19], -3, prec)).unwrap_err();
        assert!(
            err.contains("MaxNumericLiteralDigits=4096"),
            "prec {prec}: {err}"
        );
    }
    // The bound is a per-call limit: 2^29 has 9 digits, so N is 10, and
    // 2^30 has 10.
    let lim = |n| Limits {
        max_numeric_literal_digits: n,
        ..Limits::default()
    };
    read_float_with(&big_float_wire(&[0x19], -3, 29), lim(10)).unwrap();
    let err = read_float_with(&big_float_wire(&[0x19], -3, 30), lim(10)).unwrap_err();
    assert!(err.contains("MaxNumericLiteralDigits=10"), "{err}");
    read_float_with(&big_float_wire(&[0x19], -3, 13604), lim(5000)).unwrap();
}

#[test]
fn marshal_refuses_what_unmarshal_rejects() {
    // Decimal scale past the limit: 1/2^4097 is 5^4097 × 10^-4097, which no
    // smaller scale holds.
    let past = DecimalHolder {
        d: Some(Decimal {
            unscaled: pow5(MAX_DIGITS as u32 + 1),
            scale: MAX_DIGITS + 1,
            negative: false,
        }),
    };
    assert_eq!(
        marshal_err(&past),
        "pxf.Decimal scale 4097 exceeds MaxNumericLiteralDigits=4096"
    );
    // At the limit it round-trips.
    let at = Decimal {
        unscaled: pow5(MAX_DIGITS as u32),
        scale: MAX_DIGITS,
        negative: false,
    };
    let data = marshal(&DecimalHolder {
        d: Some(at.clone()),
    })
    .unwrap();
    let back: DecimalHolder = unmarshal(&data).unwrap();
    assert_eq!(back.d.unwrap(), at);

    // A BigFloat precision too wide to scale is refused on reading, and a
    // varint past uint32 keeps its low 32 bits, so 1<<32 + 1<<31 is 1<<31.
    for prec in [
        1u64 << 31,
        (1 << 31) + 1,
        u64::from(u32::MAX),
        (1 << 32) + (1 << 31),
    ] {
        let mut sub = Vec::new();
        field_bytes(&mut sub, 1, &[0x19]);
        field_varint(&mut sub, 3, prec);
        let mut wire = Vec::new();
        field_bytes(&mut wire, 1, &sub);
        if prec == 1 << 31 {
            assert_eq!(
                wire,
                [0x0a, 0x09, 0x0a, 0x01, 0x19, 0x18, 0x80, 0x80, 0x80, 0x80, 0x08]
            );
        }
        let err = read_float(&wire).unwrap_err();
        assert!(
            err.contains("MaxNumericLiteralDigits"),
            "prec {prec}: {err}"
        );
    }
    // The same value built in memory, singular and in a list, and the one
    // past the bound: marshal refuses what unmarshal would.
    for prec in [1u32 << 31, 13604] {
        let neg = BigFloat {
            mantissa: vec![3],
            exponent: 0,
            prec,
            negative: true,
        };
        let err = marshal_err(&BigFloatHolder {
            f: Some(neg.clone()),
        });
        assert!(err.contains("MaxNumericLiteralDigits=4096"), "{err}");
        let err = marshal_err(&BigFloatList { f: vec![neg] });
        assert!(err.contains("MaxNumericLiteralDigits=4096"), "{err}");
    }
    // At the bound the value is written and reads back as the same number.
    let at = BigFloat {
        mantissa: vec![3],
        exponent: 0,
        prec: 13603,
        negative: true,
    };
    let out = marshal(&BigFloatHolder { f: Some(at) }).unwrap();
    let back = read_float(&out).unwrap();
    assert_eq!(
        (back.prec, back.negative, mant_exp(&back)),
        (13603, true, 2)
    );

    // The exponent leaves int32 after widening: 0.75 × 2^(MinInt32+3) at 53
    // bits is mantissa 3 × 2^(MinInt32+1).
    let low = BigFloat {
        mantissa: vec![3],
        exponent: i32::MIN + 1,
        prec: 53,
        negative: false,
    };
    let err = marshal_err(&BigFloatHolder { f: Some(low) });
    assert!(err.contains("does not fit int32"), "{err}");

    // 0.75 × 2^k has binary exponent k: refused past the bound, written at it.
    let past = BigFloat {
        mantissa: vec![3],
        exponent: -MAX_EXP - 1 - 2,
        prec: 53,
        negative: false,
    };
    let err = marshal_err(&BigFloatHolder { f: Some(past) });
    assert!(err.contains("MaxBigFloatExponent"), "{err}");
    let at = BigFloat {
        mantissa: vec![3],
        exponent: -MAX_EXP - 2,
        prec: 53,
        negative: false,
    };
    let out = marshal(&BigFloatHolder { f: Some(at) }).unwrap();
    assert_eq!(mant_exp(&read_float(&out).unwrap()), -i64::from(MAX_EXP));
}

// A BigInt reads back as a value: no leading zeros, and no -0.
#[test]
fn big_int_reads_as_a_value() {
    use protowire_pb::BigInt;
    let mut sub = Vec::new();
    field_bytes(&mut sub, 1, &[0, 0]);
    field_varint(&mut sub, 2, 1);
    let mut wire = Vec::new();
    field_bytes(&mut wire, 1, &sub);
    #[derive(Default)]
    struct H {
        i: Option<BigInt>,
    }
    impl Message for H {
        fn encode_to(&self, _: &mut Writer) {}
        fn merge_field(&mut self, num: u32, wt: WireType, r: &mut Reader<'_>) -> Result<()> {
            match num {
                1 => self.i = Some(read_message(r)?),
                _ => r.skip(wt)?,
            }
            Ok(())
        }
    }
    let h: H = unmarshal(&wire).unwrap();
    assert_eq!(h.i.unwrap(), BigInt::default());
}
