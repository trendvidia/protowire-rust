// SPDX-License-Identifier: MIT
// Copyright (c) 2026 TrendVidia, LLC.
//! The struct-binding table (trendvidia/protowire#509): values written
//! through a port's struct API, with the bytes protowire-go writes for each.
//! This port's struct API is the [`Message`] trait, so each row's struct has
//! a twin here, written the way a `Message` impl writes it: scalars skipped
//! at zero, repeated scalars packed, list elements always written, map
//! entries through [`write_map_entry`], submessages through
//! [`write_message`].
//!
//! `testdata/values.tsv` is a byte copy of the spec repo's
//! `testdata/values/values.tsv` (at aee541c); CI compares the two.
//!
//! A `*big.Int`, `*big.Rat` or `*big.Float` field's twin holds a
//! [`BigInt`], [`Decimal`] or [`BigFloat`] (#142): a pointer field is an
//! `Option`, written with `write_big_int` and its kin; a list element or a
//! map value is written with `write_message` / `write_map_entry`, a null
//! one as the type's default, which is the empty message Go writes. Every
//! row has a twin, and a row without one fails.

use std::collections::BTreeMap;

use protowire_pb::wire::{Reader, Result, WireType, Writer};
use protowire_pb::{
    marshal, write_big_float, write_big_int, write_decimal, write_map_entry, write_message,
    BigFloat, BigInt, Decimal, Message,
};

const TABLE: &str = include_str!("../testdata/values.tsv");

/// A `Message` that is only ever written here.
macro_rules! write_only {
    ($t:ty, |$s:ident, $w:ident| $body:block) => {
        impl Message for $t {
            fn encode_to(&self, $w: &mut Writer) {
                let $s = self;
                $body
            }
            fn merge_field(&mut self, _: u32, wt: WireType, r: &mut Reader<'_>) -> Result<()> {
                r.skip(wt)
            }
        }
    };
}

/// A packed repeated scalar field: one length-delimited record.
fn packed(w: &mut Writer, num: u32, write: impl FnOnce(&mut Writer)) {
    let mut inner = Writer::new();
    write(&mut inner);
    let payload = inner.finish();
    if !payload.is_empty() {
        w.tag(num, WireType::LengthDelimited);
        w.bytes(&payload);
    }
}

#[derive(Default)]
struct Ints {
    a: i64,
    b: i64,
    c: i32,
    m: BTreeMap<String, i64>,
    mi: BTreeMap<String, i32>,
}
write_only!(Ints, |s, w| {
    for (num, v) in [(1, s.a), (2, s.b)] {
        if v != 0 {
            w.tag(num, WireType::Varint);
            w.varint_i64(v);
        }
    }
    if s.c != 0 {
        w.tag(3, WireType::Varint);
        w.varint_i32(s.c);
    }
    for (k, v) in &s.m {
        write_map_entry(w, 4, k.as_str(), v);
    }
    for (k, v) in &s.mi {
        write_map_entry(w, 5, k.as_str(), v);
    }
});

#[derive(Default)]
struct Age {
    age: i32,
}
write_only!(Age, |s, w| {
    if s.age != 0 {
        w.tag(1, WireType::Varint);
        w.varint_i32(s.age);
    }
});

#[derive(Default)]
struct MinLong {
    v: i64,
}
write_only!(MinLong, |s, w| {
    if s.v != 0 {
        w.tag(1, WireType::Varint);
        w.varint_i64(s.v);
    }
});

/// Both fields `sint`: zigzag.
#[derive(Default)]
struct Zigzag {
    d: i64,
    e: i32,
}
write_only!(Zigzag, |s, w| {
    if s.d != 0 {
        w.tag(1, WireType::Varint);
        w.zigzag64(s.d);
    }
    if s.e != 0 {
        w.tag(2, WireType::Varint);
        w.zigzag32(s.e);
    }
});

#[derive(Default)]
struct Packed {
    xs: Vec<i32>,
    ys: Vec<i64>,
    bs: Vec<bool>,
    ds: Vec<f64>,
    fs: Vec<f32>,
}
write_only!(Packed, |s, w| {
    packed(w, 1, |p| s.xs.iter().for_each(|&v| p.varint_i32(v)));
    packed(w, 2, |p| s.ys.iter().for_each(|&v| p.varint_i64(v)));
    packed(w, 3, |p| s.bs.iter().for_each(|&v| p.varint(u64::from(v))));
    packed(w, 4, |p| s.ds.iter().for_each(|&v| p.double(v)));
    packed(w, 5, |p| s.fs.iter().for_each(|&v| p.float(v)));
});

#[derive(Default)]
struct ZeroMapEntry {
    m: BTreeMap<String, String>,
    z: BTreeMap<String, i32>,
    k: BTreeMap<i32, String>,
}
write_only!(ZeroMapEntry, |s, w| {
    for (k, v) in &s.m {
        write_map_entry(w, 1, k.as_str(), v.as_str());
    }
    for (k, v) in &s.z {
        write_map_entry(w, 2, k.as_str(), v);
    }
    for (k, v) in &s.k {
        write_map_entry(w, 3, k, v.as_str());
    }
});

#[derive(Default)]
struct Leaf {
    name: String,
    n: i32,
}
write_only!(Leaf, |s, w| {
    if !s.name.is_empty() {
        w.tag(1, WireType::LengthDelimited);
        w.string(&s.name);
    }
    if s.n != 0 {
        w.tag(2, WireType::Varint);
        w.varint_i32(s.n);
    }
});

#[derive(Default)]
struct Nested {
    s: String,
    ls: Vec<Leaf>,
    ss: Vec<String>,
}
write_only!(Nested, |s, w| {
    if !s.s.is_empty() {
        w.tag(1, WireType::LengthDelimited);
        w.string(&s.s);
    }
    // A list element is written even when it is zero, so the list keeps
    // its length.
    for l in &s.ls {
        write_message(w, 2, l);
    }
    for e in &s.ss {
        w.tag(3, WireType::LengthDelimited);
        w.string(e);
    }
});

#[derive(Default)]
struct Zeros {
    i: i32,
    s: String,
    b: bool,
    d: f64,
    bs: Vec<u8>,
    u: u64,
}
write_only!(Zeros, |s, w| {
    if s.i != 0 {
        w.tag(1, WireType::Varint);
        w.varint_i32(s.i);
    }
    if !s.s.is_empty() {
        w.tag(2, WireType::LengthDelimited);
        w.string(&s.s);
    }
    if s.b {
        w.tag(3, WireType::Varint);
        w.varint(1);
    }
    if s.d.to_bits() != 0 {
        w.tag(4, WireType::Fixed64);
        w.double(s.d);
    }
    if !s.bs.is_empty() {
        w.tag(5, WireType::LengthDelimited);
        w.bytes(&s.bs);
    }
    if s.u != 0 {
        w.tag(6, WireType::Varint);
        w.varint(s.u);
    }
});

// ---- big numbers (#142) ----

/// The big-endian magnitude of `v`, with no leading zero byte.
fn be(v: u64) -> Vec<u8> {
    let b = v.to_be_bytes();
    let first = b.iter().position(|x| *x != 0).unwrap_or(b.len());
    b[first..].to_vec()
}

fn dec(unscaled: u64, scale: i32, negative: bool) -> Decimal {
    Decimal {
        unscaled: be(unscaled),
        scale,
        negative,
    }
}

fn int(abs: u64, negative: bool) -> BigInt {
    BigInt {
        abs: be(abs),
        negative,
    }
}

/// `v` as a BigFloat at `prec` bits: mantissa × 2^exponent, not yet
/// normalised, so the writer's rounding and widening are what is tested.
fn float(mantissa: u64, exponent: i32, prec: u32, negative: bool) -> BigFloat {
    BigFloat {
        mantissa: be(mantissa),
        exponent,
        prec,
        negative,
    }
}

#[derive(Default)]
struct VDecimal {
    d: Option<Decimal>,
}
write_only!(VDecimal, |s, w| {
    if let Some(d) = &s.d {
        write_decimal(w, 1, d);
    }
});

#[derive(Default)]
struct VDecimals {
    ds: Vec<Option<Decimal>>,
}
write_only!(VDecimals, |s, w| {
    for d in &s.ds {
        write_message(w, 1, &d.clone().unwrap_or_default());
    }
});

#[derive(Default)]
struct VBigNums {
    i: Option<BigInt>,
    d: Option<Decimal>,
    f: Option<BigFloat>,
}
write_only!(VBigNums, |s, w| {
    if let Some(i) = &s.i {
        write_big_int(w, 1, i);
    }
    if let Some(d) = &s.d {
        write_decimal(w, 2, d);
    }
    if let Some(f) = &s.f {
        write_big_float(w, 3, f);
    }
});

#[derive(Default)]
struct VBigInts {
    is: Vec<Option<BigInt>>,
}
write_only!(VBigInts, |s, w| {
    for i in &s.is {
        write_message(w, 1, &i.clone().unwrap_or_default());
    }
});

#[derive(Default)]
struct VBigFloatMap {
    m: BTreeMap<String, Option<BigFloat>>,
}
write_only!(VBigFloatMap, |s, w| {
    for (k, v) in &s.m {
        write_map_entry(w, 1, k.as_str(), &v.clone().unwrap_or_default());
    }
});

#[derive(Default)]
struct VBigFloat {
    f: Option<BigFloat>,
}
write_only!(VBigFloat, |s, w| {
    if let Some(f) = &s.f {
        write_big_float(w, 1, f);
    }
});

fn map<K: Ord, V>(entries: impl IntoIterator<Item = (K, V)>) -> BTreeMap<K, V> {
    entries.into_iter().collect()
}

/// Each covered row's bytes, built from the row's value column.
fn twin(case: &str) -> Option<Vec<u8>> {
    let bytes = match case {
        "Ints" => marshal(&Ints {
            a: -1,
            b: -1,
            c: -1,
            m: map([("k".to_string(), -1)]),
            mi: map([("k".to_string(), -1)]),
        }),
        "Age" => marshal(&Age { age: 30 }),
        "MinLong" => marshal(&MinLong { v: i64::MIN }),
        "Zigzag" => marshal(&Zigzag { d: -1, e: -2 }),
        "Packed" => marshal(&Packed {
            xs: vec![1, 0, -1],
            ys: vec![300],
            bs: vec![true, false],
            ds: vec![1.5],
            fs: vec![-2.5],
        }),
        "ZeroMapEntry" => marshal(&ZeroMapEntry {
            m: map([(String::new(), String::new())]),
            z: map([("zero".to_string(), 0)]),
            k: map([(0, "v".to_string())]),
        }),
        "Nested" => marshal(&Nested {
            s: "x".into(),
            ls: vec![
                Leaf::default(),
                Leaf {
                    name: "l".into(),
                    n: -3,
                },
            ],
            ss: vec![String::new(), "s".into()],
        }),
        "Zeros" => marshal(&Zeros::default()),
        // Each Decimal is given at the scale its value column spells, so the
        // writer's reduction to the smallest scale is what is tested.
        "Decimal" => marshal(&VDecimal {
            d: Some(dec(31415, 4, false)),
        }),
        "DecimalOne" => marshal(&VDecimal {
            d: Some(dec(100, 2, false)),
        }),
        "DecimalFiveE2" => marshal(&VDecimal {
            d: Some(dec(5, -2, false)),
        }),
        "DecimalFifteen" => marshal(&VDecimal {
            d: Some(dec(1500, 2, false)),
        }),
        "DecimalZero" => marshal(&VDecimal {
            d: Some(dec(0, 2, false)),
        }),
        "DecimalNegative" => marshal(&VDecimal {
            d: Some(dec(150, 2, true)),
        }),
        "Decimals" => marshal(&VDecimals {
            ds: vec![
                Some(dec(0, 2, false)),
                Some(dec(100, 2, false)),
                Some(dec(5, -2, false)),
            ],
        }),
        "BigNumsNull" => marshal(&VBigNums::default()),
        "BigNumsZero" => marshal(&VBigNums {
            i: Some(BigInt::default()),
            d: Some(Decimal::default()),
            f: Some(BigFloat::default()),
        }),
        "BigNumsNegative" => marshal(&VBigNums {
            i: Some(int(12345678901234567890, true)),
            d: Some(dec(5, 1, true)),
            f: Some(float(3, -1, 53, true)),
        }),
        "BigIntsZeroElement" => marshal(&VBigInts {
            is: vec![Some(BigInt::default()), Some(int(1, false))],
        }),
        "BigIntsNullElement" => marshal(&VBigInts {
            is: vec![Some(int(1, false)), None],
        }),
        "DecimalsNullElement" => marshal(&VDecimals {
            ds: vec![Some(dec(1, 0, false)), None],
        }),
        "BigFloatMapNullValue" => marshal(&VBigFloatMap {
            m: map([("k".to_string(), None)]),
        }),
        "BigFloatPrec64" => marshal(&VBigFloat {
            f: Some(float(3, -1, 64, false)),
        }),
        _ => return None,
    };
    Some(bytes.expect("marshal"))
}

fn hex(b: &[u8]) -> String {
    b.iter().map(|x| format!("{x:02x}")).collect()
}

#[test]
fn every_row_is_written_as_protowire_go_writes_it() {
    let mut covered = 0;
    let mut problems = Vec::new();
    for line in TABLE
        .lines()
        .filter(|l| !l.starts_with('#') && !l.is_empty())
    {
        let cols: Vec<&str> = line.split('\t').collect();
        let [case, want, value] = cols[..] else {
            panic!("malformed row: {line:?}");
        };
        match twin(case) {
            Some(got) => {
                covered += 1;
                if hex(&got) != want {
                    problems.push(format!(
                        "{case} ({value}):\n  go:   {want}\n  rust: {}",
                        hex(&got)
                    ));
                }
            }
            None => problems.push(format!("{case}: a row with no twin")),
        }
    }
    assert!(problems.is_empty(), "{}", problems.join("\n"));
    assert_eq!(covered, 23, "rows with a twin");
}
