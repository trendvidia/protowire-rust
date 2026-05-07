// SPDX-License-Identifier: MIT
// Copyright (c) 2026 TrendVidia, LLC.
//! Schema-driven binary marshal/unmarshal via the [`Message`] trait.
//!
//! Each Rust message type implements `Message` to encode/decode its own
//! fields one at a time over the wire primitives in [`crate::wire`].
//! Helpers for nested-message blobs live here so impls stay short.
//!
//! Wire-format choices match proto3 semantics:
//!
//! - `int32` / `int64`: plain varint, with negative values sign-extended
//!   to a 10-byte uint64.
//! - `sint32` / `sint64`: zigzag varint; more compact for negative values.
//! - `uint32` / `uint64`: plain varint.
//! - `bool`: varint (0 / 1).
//! - `float`: fixed32; `double`: fixed64.
//! - `string` and `bytes`: length-delimited.
//! - nested messages: length-delimited.
//! - repeated fields: one tag+value per element (non-packed).
//! - maps: each entry is a length-delimited `MapEntry { key=1; value=2 }`.

use crate::wire::{Error, Reader, Result, WireType, Writer, MAX_NESTING_DEPTH};

/// A message with self-contained encode/decode. Mirrors the role of
/// `prost::Message` for our trait-based codec.
pub trait Message: Sized + Default {
    /// Append the message's fields to `w`. Caller is responsible for
    /// the surrounding length prefix when used as a nested message.
    fn encode_to(&self, w: &mut Writer);

    /// Merge a single field (already-decoded tag) into `self`.
    /// Implementations should call `r.skip(wire_type)` for unknown numbers.
    fn merge_field(
        &mut self,
        field_number: u32,
        wire_type: WireType,
        r: &mut Reader<'_>,
    ) -> Result<()>;
}

/// Encode a message to a byte vector.
pub fn marshal<M: Message>(value: &M) -> Vec<u8> {
    let mut w = Writer::new();
    value.encode_to(&mut w);
    w.finish()
}

/// Decode a message from a byte slice.
pub fn unmarshal<M: Message>(data: &[u8]) -> Result<M> {
    let mut r = Reader::new(data);
    let mut msg = M::default();
    while !r.eof() {
        let (num, wt) = r.tag()?;
        msg.merge_field(num, wt, &mut r)?;
    }
    Ok(msg)
}

/// Write a nested message at `field_number` as a length-delimited blob.
pub fn write_message<M: Message>(w: &mut Writer, field_number: u32, msg: &M) {
    let mut inner = Writer::new();
    msg.encode_to(&mut inner);
    let bytes = inner.finish();
    w.tag(field_number, WireType::LengthDelimited);
    w.varint(bytes.len() as u64);
    w.raw(&bytes);
}

/// Read a length-delimited nested message. The reader's tag is already consumed.
///
/// Increments `r.depth` for the duration of the inner decode and rejects with
/// [`Error::DepthExceeded`] before recursing past [`MAX_NESTING_DEPTH`]. Per
/// HARDENING.md §Recursion, the counter must persist across `merge_field` →
/// `read_message` re-entry; that's why it lives on the `Reader`, not as a
/// thread-local or function argument.
///
/// The length-prefix bounds check uses `checked_add` so that a maximum-value
/// varint length (2^64 - 1) cannot wrap `pos + len` past a naive comparison
/// and trip a slice-indexing panic — HARDENING.md §API contract item 3.
pub fn read_message<M: Message>(r: &mut Reader<'_>) -> Result<M> {
    let len = r.varint()?;
    let len = usize::try_from(len).map_err(|_| Error::NestedExceedsBuffer)?;
    let end = r.pos.checked_add(len).ok_or(Error::NestedExceedsBuffer)?;
    if end > r.data().len() {
        return Err(Error::NestedExceedsBuffer);
    }
    if r.depth >= MAX_NESTING_DEPTH {
        return Err(Error::DepthExceeded(MAX_NESTING_DEPTH));
    }
    r.depth += 1;
    let result = (|| -> Result<M> {
        let mut msg = M::default();
        while r.pos < end {
            let (num, wt) = r.tag()?;
            msg.merge_field(num, wt, r)?;
        }
        if r.pos != end {
            return Err(Error::Overrun { pos: r.pos, end });
        }
        Ok(msg)
    })();
    r.depth -= 1;
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;

    // --- Test message types ---
    //
    // Mirror `Inner` / `Outer` from the TS port's pb/codec.test.ts.

    #[derive(Debug, Default, Clone, PartialEq)]
    struct Inner {
        name: String,
        value: i32,
    }

    impl Message for Inner {
        fn encode_to(&self, w: &mut Writer) {
            if !self.name.is_empty() {
                w.tag(1, WireType::LengthDelimited);
                w.string(&self.name);
            }
            if self.value != 0 {
                w.tag(2, WireType::Varint);
                w.varint_i32(self.value);
            }
        }

        fn merge_field(&mut self, num: u32, wt: WireType, r: &mut Reader<'_>) -> Result<()> {
            match num {
                1 => self.name = r.string()?,
                2 => self.value = r.varint()? as i32,
                _ => r.skip(wt)?,
            }
            Ok(())
        }
    }

    #[derive(Debug, Default, Clone, PartialEq)]
    struct Outer {
        title: String,
        count: u32,
        score: f64,
        active: bool,
        data: Vec<u8>,
        items: Vec<Inner>,
        signed: i64,
        small_f: f32,
    }

    impl Message for Outer {
        fn encode_to(&self, w: &mut Writer) {
            if !self.title.is_empty() {
                w.tag(1, WireType::LengthDelimited);
                w.string(&self.title);
            }
            if self.count != 0 {
                w.tag(2, WireType::Varint);
                w.varint(self.count as u64);
            }
            if self.score != 0.0 {
                w.tag(3, WireType::Fixed64);
                w.double(self.score);
            }
            if self.active {
                w.tag(4, WireType::Varint);
                w.varint(1);
            }
            if !self.data.is_empty() {
                w.tag(5, WireType::LengthDelimited);
                w.bytes(&self.data);
            }
            for item in &self.items {
                write_message(w, 6, item);
            }
            if self.signed != 0 {
                w.tag(8, WireType::Varint);
                w.varint_i64(self.signed);
            }
            if self.small_f != 0.0 {
                w.tag(9, WireType::Fixed32);
                w.float(self.small_f);
            }
        }

        fn merge_field(&mut self, num: u32, wt: WireType, r: &mut Reader<'_>) -> Result<()> {
            match num {
                1 => self.title = r.string()?,
                2 => self.count = r.varint()? as u32,
                3 => self.score = r.double()?,
                4 => self.active = r.varint()? != 0,
                5 => self.data = r.bytes()?,
                6 => self.items.push(read_message(r)?),
                8 => self.signed = r.varint()? as i64,
                9 => self.small_f = r.float()?,
                _ => r.skip(wt)?,
            }
            Ok(())
        }
    }

    #[test]
    fn populated_message_round_trip() {
        let orig = Outer {
            title: "hello".into(),
            count: 42,
            score: 3.125,
            active: true,
            data: vec![0xde, 0xad],
            items: vec![
                Inner { name: "a".into(), value: 1 },
                Inner { name: "b".into(), value: -7 },
            ],
            signed: -12345,
            small_f: 2.5,
        };
        let bytes = marshal(&orig);
        let got: Outer = unmarshal(&bytes).unwrap();
        assert_eq!(got, orig);
    }

    #[test]
    fn all_zero_message_marshals_to_empty_bytes() {
        let bytes = marshal(&Outer::default());
        assert!(bytes.is_empty());
    }

    #[test]
    fn empty_bytes_unmarshal_to_default() {
        let got: Outer = unmarshal(&[]).unwrap();
        assert_eq!(got, Outer::default());
    }

    #[test]
    fn unknown_fields_are_skipped() {
        #[derive(Debug, Default, PartialEq)]
        struct Big {
            a: String,
            b: String,
            c: String,
        }
        impl Message for Big {
            fn encode_to(&self, w: &mut Writer) {
                if !self.a.is_empty() {
                    w.tag(1, WireType::LengthDelimited);
                    w.string(&self.a);
                }
                if !self.b.is_empty() {
                    w.tag(2, WireType::LengthDelimited);
                    w.string(&self.b);
                }
                if !self.c.is_empty() {
                    w.tag(3, WireType::LengthDelimited);
                    w.string(&self.c);
                }
            }
            fn merge_field(&mut self, num: u32, wt: WireType, r: &mut Reader<'_>) -> Result<()> {
                match num {
                    1 => self.a = r.string()?,
                    2 => self.b = r.string()?,
                    3 => self.c = r.string()?,
                    _ => r.skip(wt)?,
                }
                Ok(())
            }
        }
        #[derive(Debug, Default, PartialEq)]
        struct Small {
            a: String,
        }
        impl Message for Small {
            fn encode_to(&self, w: &mut Writer) {
                if !self.a.is_empty() {
                    w.tag(1, WireType::LengthDelimited);
                    w.string(&self.a);
                }
            }
            fn merge_field(&mut self, num: u32, wt: WireType, r: &mut Reader<'_>) -> Result<()> {
                match num {
                    1 => self.a = r.string()?,
                    _ => r.skip(wt)?,
                }
                Ok(())
            }
        }

        let bytes = marshal(&Big {
            a: "aa".into(),
            b: "bb".into(),
            c: "cc".into(),
        });
        let got: Small = unmarshal(&bytes).unwrap();
        assert_eq!(got.a, "aa");
    }

    #[derive(Debug, Default, Clone, PartialEq)]
    struct Wrap {
        inner: Option<Inner>,
    }

    impl Message for Wrap {
        fn encode_to(&self, w: &mut Writer) {
            if let Some(ref i) = self.inner {
                write_message(w, 1, i);
            }
        }
        fn merge_field(&mut self, num: u32, wt: WireType, r: &mut Reader<'_>) -> Result<()> {
            match num {
                1 => self.inner = Some(read_message(r)?),
                _ => r.skip(wt)?,
            }
            Ok(())
        }
    }

    #[test]
    fn singular_nested_none_omits_tag() {
        let bytes = marshal(&Wrap { inner: None });
        assert!(bytes.is_empty());
        let got: Wrap = unmarshal(&bytes).unwrap();
        assert!(got.inner.is_none());
    }

    #[test]
    fn singular_nested_populated_round_trips() {
        let bytes = marshal(&Wrap {
            inner: Some(Inner { name: "x".into(), value: 9 }),
        });
        let got: Wrap = unmarshal(&bytes).unwrap();
        assert_eq!(got.inner, Some(Inner { name: "x".into(), value: 9 }));
    }

    #[test]
    fn singular_nested_empty_emits_zero_length_blob() {
        // Some(Inner::default()) should still emit tag(1, LengthDelim) + len 0.
        let bytes = marshal(&Wrap {
            inner: Some(Inner::default()),
        });
        assert_eq!(bytes, vec![0x0a, 0x00]);
        let got: Wrap = unmarshal(&bytes).unwrap();
        assert_eq!(got.inner, Some(Inner::default()));
    }

    #[derive(Debug, Default, Clone, PartialEq)]
    struct WithStringMap {
        meta: BTreeMap<String, String>,
    }

    impl Message for WithStringMap {
        fn encode_to(&self, w: &mut Writer) {
            for (k, v) in &self.meta {
                let mut inner = Writer::new();
                if !k.is_empty() {
                    inner.tag(1, WireType::LengthDelimited);
                    inner.string(k);
                }
                if !v.is_empty() {
                    inner.tag(2, WireType::LengthDelimited);
                    inner.string(v);
                }
                let bytes = inner.finish();
                w.tag(1, WireType::LengthDelimited);
                w.varint(bytes.len() as u64);
                w.raw(&bytes);
            }
        }
        fn merge_field(&mut self, num: u32, wt: WireType, r: &mut Reader<'_>) -> Result<()> {
            match num {
                1 => {
                    let len = r.varint()? as usize;
                    let end = r.pos + len;
                    let mut k = String::new();
                    let mut v = String::new();
                    while r.pos < end {
                        let (n, w) = r.tag()?;
                        match n {
                            1 => k = r.string()?,
                            2 => v = r.string()?,
                            _ => r.skip(w)?,
                        }
                    }
                    self.meta.insert(k, v);
                }
                _ => r.skip(wt)?,
            }
            Ok(())
        }
    }

    #[test]
    fn map_string_string_round_trips() {
        let mut meta = BTreeMap::new();
        meta.insert("a".into(), "1".into());
        meta.insert("b".into(), "2".into());
        meta.insert("key with space".into(), "v".into());
        let bytes = marshal(&WithStringMap { meta: meta.clone() });
        let got: WithStringMap = unmarshal(&bytes).unwrap();
        assert_eq!(got.meta, meta);
    }

    #[test]
    fn map_string_string_empty_produces_empty_bytes() {
        let bytes = marshal(&WithStringMap::default());
        assert!(bytes.is_empty());
        let got: WithStringMap = unmarshal(&bytes).unwrap();
        assert!(got.meta.is_empty());
    }

    #[derive(Debug, Default, Clone, PartialEq)]
    struct WithIntMap {
        codes: BTreeMap<i32, String>,
    }

    impl Message for WithIntMap {
        fn encode_to(&self, w: &mut Writer) {
            for (k, v) in &self.codes {
                let mut inner = Writer::new();
                if *k != 0 {
                    inner.tag(1, WireType::Varint);
                    inner.varint_i32(*k);
                }
                if !v.is_empty() {
                    inner.tag(2, WireType::LengthDelimited);
                    inner.string(v);
                }
                let bytes = inner.finish();
                w.tag(1, WireType::LengthDelimited);
                w.varint(bytes.len() as u64);
                w.raw(&bytes);
            }
        }
        fn merge_field(&mut self, num: u32, wt: WireType, r: &mut Reader<'_>) -> Result<()> {
            match num {
                1 => {
                    let len = r.varint()? as usize;
                    let end = r.pos + len;
                    let mut k: i32 = 0;
                    let mut v = String::new();
                    while r.pos < end {
                        let (n, w) = r.tag()?;
                        match n {
                            1 => k = r.varint()? as i32,
                            2 => v = r.string()?,
                            _ => r.skip(w)?,
                        }
                    }
                    self.codes.insert(k, v);
                }
                _ => r.skip(wt)?,
            }
            Ok(())
        }
    }

    #[test]
    fn map_int32_string_round_trips() {
        let mut codes = BTreeMap::new();
        codes.insert(404, "Not Found".into());
        codes.insert(500, "Internal".into());
        let bytes = marshal(&WithIntMap { codes: codes.clone() });
        let got: WithIntMap = unmarshal(&bytes).unwrap();
        assert_eq!(got.codes, codes);
    }

    // --- Cross-port wire-contract specifics ---
    //
    // The two TS-only schema-validation tests (duplicate field number,
    // repeated+map) don't apply to a trait-based codec — those are
    // compile-time invariants here. Replace them with two tests that
    // pin down the wire-format invariants the cross-port script
    // depends on.

    #[derive(Debug, Default, Clone, PartialEq)]
    struct SignedI32 {
        v: i32,
    }
    impl Message for SignedI32 {
        fn encode_to(&self, w: &mut Writer) {
            if self.v != 0 {
                w.tag(1, WireType::Varint);
                w.varint_i32(self.v);
            }
        }
        fn merge_field(&mut self, num: u32, wt: WireType, r: &mut Reader<'_>) -> Result<()> {
            match num {
                1 => self.v = r.varint()? as i32,
                _ => r.skip(wt)?,
            }
            Ok(())
        }
    }

    #[test]
    fn proto3_int32_negative_sign_extends_to_10_byte_varint() {
        // Cross-port contract: -1 as proto3 int32 emits FF FF FF FF FF FF FF FF FF 01
        // (sign-extended uint64). Required for envelope parity with Go/C++/TS/Java.
        let bytes = marshal(&SignedI32 { v: -1 });
        assert_eq!(
            bytes,
            vec![0x08, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0x01]
        );
        let got: SignedI32 = unmarshal(&bytes).unwrap();
        assert_eq!(got.v, -1);
    }

    #[derive(Debug, Default, Clone, PartialEq)]
    struct ZigzagI32 {
        v: i32,
    }
    impl Message for ZigzagI32 {
        fn encode_to(&self, w: &mut Writer) {
            if self.v != 0 {
                w.tag(1, WireType::Varint);
                w.zigzag32(self.v);
            }
        }
        fn merge_field(&mut self, num: u32, wt: WireType, r: &mut Reader<'_>) -> Result<()> {
            match num {
                1 => self.v = r.zigzag32()?,
                _ => r.skip(wt)?,
            }
            Ok(())
        }
    }

    // --- HARDENING.md §Recursion -----------------------------------------
    //
    // `read_message` must reject before recursing past MAX_NESTING_DEPTH and
    // must not crash on adversarial deep-nesting input. The cap matches the
    // cross-port HARDENING.md default of 100.

    /// Self-recursive PB message — mirrors `adversarial.v1.Tree` in the
    /// shared corpus. A tower of `Tree`s is the canonical adversarial
    /// fixture for depth-cap testing.
    #[derive(Default, Debug)]
    struct Tree {
        child: Option<Box<Tree>>,
    }

    impl Message for Tree {
        fn encode_to(&self, w: &mut Writer) {
            if let Some(c) = &self.child {
                write_message(w, 1, c.as_ref());
            }
        }
        fn merge_field(&mut self, num: u32, wt: WireType, r: &mut Reader<'_>) -> Result<()> {
            match num {
                1 => self.child = Some(Box::new(read_message(r)?)),
                _ => r.skip(wt)?,
            }
            Ok(())
        }
    }

    /// Build wire bytes for a Tree of N nested `child` levels.
    fn build_tree_bytes(depth: usize) -> Vec<u8> {
        let mut payload: Vec<u8> = Vec::new(); // empty leaf
        for _ in 0..depth {
            let mut framed = Vec::new();
            framed.push(0x0a); // tag(1, LengthDelimited)
            let mut len = payload.len() as u64;
            while len >= 0x80 {
                framed.push(((len & 0x7f) as u8) | 0x80);
                len >>= 7;
            }
            framed.push(len as u8);
            framed.extend_from_slice(&payload);
            payload = framed;
        }
        payload
    }

    #[test]
    fn deep_submessage_at_limit_is_accepted() {
        // 100 nested children → root + 100 read_message calls. Cap is the
        // increment count, so 100 levels of read_message reach depth=100
        // without exceeding it.
        let bytes = build_tree_bytes(100);
        let _: Tree = unmarshal(&bytes).unwrap();
    }

    #[test]
    fn deep_submessage_past_limit_returns_depth_exceeded() {
        // 200 levels must reject cleanly, not crash.
        let bytes = build_tree_bytes(200);
        let res: Result<Tree> = unmarshal(&bytes);
        assert!(matches!(res, Err(Error::DepthExceeded(100))), "got {:?}", res);
    }

    #[test]
    fn deep_submessage_at_extreme_depth_rejects_without_stack_overflow() {
        // 100k-deep is the SIGABRT case from issue #1. The cap must trip
        // before native stack exhaustion.
        let bytes = build_tree_bytes(100_000);
        let res: Result<Tree> = unmarshal(&bytes);
        assert!(matches!(res, Err(Error::DepthExceeded(100))), "got {:?}", res);
    }

    #[test]
    fn sint32_zigzag_is_compact_for_negative_values() {
        // -1 as sint32 emits a single byte (0x01) instead of the 10-byte
        // sign-extended int32 form. This is the wire-format choice the
        // `zigzag` opt-in selects.
        let bytes = marshal(&ZigzagI32 { v: -1 });
        assert_eq!(bytes, vec![0x08, 0x01]);
        let got: ZigzagI32 = unmarshal(&bytes).unwrap();
        assert_eq!(got.v, -1);
    }
}
