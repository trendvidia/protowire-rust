// SPDX-License-Identifier: MIT
// Copyright (c) 2026 TrendVidia, LLC.
//! A struct field is a language value, so a NaN is written as the canonical
//! quiet NaN whatever its sign and payload (draft -01 § Numeric Literals,
//! protowire-go#241, #125): `0x7FF8000000000000` for a double and
//! `0x7FC00000` for a float. Go's `math.NaN()` is `0x7FF8000000000001`, and
//! the same struct in another language would hold another NaN, so writing
//! the bits as given would make the ports disagree on one value.
//!
//! The bytes are protowire-go v1.17.0's: its `TestMarshalWritesNaNCanonically`,
//! and the same struct with a map field of each, run for four NaNs.

use protowire_pb::wire::{Reader, Result, WireType, Writer};
use protowire_pb::{marshal, write_map_entry, Message};

/// Go's `nanHolder`, with protowire-go's probe's two map fields.
#[derive(Default)]
struct NanHolder {
    d: f64,
    f: f32,
    ds: Vec<f64>,
    fs: Vec<f32>,
    m: Vec<(String, f64)>,
    mf: Vec<(String, f32)>,
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

impl Message for NanHolder {
    fn encode_to(&self, w: &mut Writer) {
        if self.d != 0.0 {
            w.tag(1, WireType::Fixed64);
            w.double(self.d);
        }
        if self.f != 0.0 {
            w.tag(2, WireType::Fixed32);
            w.float(self.f);
        }
        packed(w, 3, |p| self.ds.iter().for_each(|&v| p.double(v)));
        packed(w, 4, |p| self.fs.iter().for_each(|&v| p.float(v)));
        for (k, v) in &self.m {
            write_map_entry(w, 5, k.as_str(), v);
        }
        for (k, v) in &self.mf {
            write_map_entry(w, 6, k.as_str(), v);
        }
    }

    fn merge_field(&mut self, _: u32, wt: WireType, r: &mut Reader<'_>) -> Result<()> {
        r.skip(wt)
    }
}

fn hex(b: &[u8]) -> String {
    b.iter().map(|b| format!("{b:02x}")).collect()
}

/// protowire-go's `TestMarshalWritesNaNCanonically`, byte for byte.
#[test]
fn nan_is_written_canonically_as_go_does() {
    let payload64 = f64::from_bits(0x7FF8_0000_0000_0001);
    let payload32 = f32::from_bits(0x7FC0_0001);
    let go_nan = f64::from_bits(0x7FF8_0000_0000_0001);
    let msg = NanHolder {
        d: payload64,
        f: payload32,
        ds: vec![go_nan, 1.0],
        fs: vec![payload32],
        ..Default::default()
    };
    let want = "09000000000000f87f".to_string()
        + "150000c07f"
        + "1a10"
        + "000000000000f87f"
        + "000000000000f03f"
        + "2204"
        + "0000c07f";
    assert_eq!(hex(&marshal(&msg).unwrap()), want);
}

/// A NaN with a payload, a negative one, a signalling one and one with
/// every bit set: protowire-go writes each the same, singular, packed and
/// as a map value.
#[test]
fn every_nan_is_written_alike_as_go_does() {
    let want = "09000000000000f87f150000c07f1a08000000000000f87f22040000c07f\
                2a0c0a016b11000000000000f87f32080a016b150000c07f";
    for (name, d, f) in [
        ("payload", 0x7FF8_0000_0000_0001_u64, 0x7FC0_0001_u32),
        ("negative", 0xFFF8_0000_0000_0000, 0xFFC0_0000),
        ("signalling", 0x7FF0_0000_0000_0001, 0x7F80_0001),
        ("every bit", 0xFFFF_FFFF_FFFF_FFFF, 0xFFFF_FFFF),
    ] {
        let (d, f) = (f64::from_bits(d), f32::from_bits(f));
        assert!(d.is_nan() && f.is_nan(), "{name}");
        let msg = NanHolder {
            d,
            f,
            ds: vec![d],
            fs: vec![f],
            m: vec![("k".into(), d)],
            mf: vec![("k".into(), f)],
        };
        assert_eq!(hex(&marshal(&msg).unwrap()), want, "{name}");
    }
}

/// `fixed64` and `fixed32` stay the raw-bits writers, as protowire-go's
/// `AppendFixed64` is: only a float or double value is canonicalized.
#[test]
fn fixed_bits_are_written_as_given() {
    let mut w = Writer::new();
    w.fixed64(0x7FF8_0000_0000_0001);
    w.fixed32(0x7FC0_0001);
    assert_eq!(hex(&w.finish()), "010000000000f87f0100c07f");
}
