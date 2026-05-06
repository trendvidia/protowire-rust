//! Low-level protobuf wire-format primitives: varint, zigzag, fixed32/64,
//! length-delimited bytes, and tag (field-number + wire-type) encoding.
//!
//! Mirrors `google.golang.org/protobuf/encoding/protowire` at the call sites
//! used by the schema-free `pb` codec, and the TS port's `wire.ts`.

use thiserror::Error;

/// HARDENING.md `MaxNestingDepth` — applies to PB submessage / group / map-entry
/// nesting. Rejection happens before recursing into the inner message, so a
/// 100k-deep adversarial input becomes a clean `Err(DepthExceeded)` instead
/// of a stack-overflow abort.
pub const MAX_NESTING_DEPTH: usize = 100;

#[derive(Debug, Error)]
pub enum Error {
    #[error("truncated varint")]
    TruncatedVarint,
    #[error("varint exceeds 10 bytes")]
    VarintTooLong,
    #[error("truncated fixed32")]
    TruncatedFixed32,
    #[error("truncated fixed64")]
    TruncatedFixed64,
    #[error("truncated length-delimited")]
    TruncatedLengthDelim,
    #[error("invalid tag: field number 0 at offset {0}")]
    InvalidTag(usize),
    #[error("unknown wire type {0}")]
    UnknownWireType(u8),
    #[error("group wire types are not supported")]
    GroupNotSupported,
    #[error("invalid utf-8 in string field: {0}")]
    InvalidUtf8(#[from] std::string::FromUtf8Error),
    #[error("nested message exceeds buffer")]
    NestedExceedsBuffer,
    #[error("message overran (pos={pos}, end={end})")]
    Overrun { pos: usize, end: usize },
    #[error("nesting depth exceeds MaxNestingDepth ({0})")]
    DepthExceeded(usize),
}

pub type Result<T> = std::result::Result<T, Error>;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum WireType {
    Varint = 0,
    Fixed64 = 1,
    LengthDelimited = 2,
    StartGroup = 3,
    EndGroup = 4,
    Fixed32 = 5,
}

impl WireType {
    pub fn from_u8(v: u8) -> Result<Self> {
        match v {
            0 => Ok(Self::Varint),
            1 => Ok(Self::Fixed64),
            2 => Ok(Self::LengthDelimited),
            3 => Ok(Self::StartGroup),
            4 => Ok(Self::EndGroup),
            5 => Ok(Self::Fixed32),
            _ => Err(Error::UnknownWireType(v)),
        }
    }
}

#[derive(Debug, Default)]
pub struct Writer {
    buf: Vec<u8>,
}

impl Writer {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn with_capacity(cap: usize) -> Self {
        Self {
            buf: Vec::with_capacity(cap),
        }
    }

    pub fn finish(self) -> Vec<u8> {
        self.buf
    }

    pub fn len(&self) -> usize {
        self.buf.len()
    }

    pub fn is_empty(&self) -> bool {
        self.buf.is_empty()
    }

    /// Append raw bytes (no length prefix).
    pub fn raw(&mut self, b: &[u8]) {
        self.buf.extend_from_slice(b);
    }

    /// Write an unsigned varint.
    pub fn varint(&mut self, mut v: u64) {
        while v >= 0x80 {
            self.buf.push(((v & 0x7f) as u8) | 0x80);
            v >>= 7;
        }
        self.buf.push(v as u8);
    }

    /// Proto3 `int32`: plain varint, with negative values sign-extended to a
    /// 10-byte uint64.
    pub fn varint_i32(&mut self, v: i32) {
        self.varint(v as i64 as u64);
    }

    /// Proto3 `int64`: plain varint, two's-complement uint64 form.
    pub fn varint_i64(&mut self, v: i64) {
        self.varint(v as u64);
    }

    /// Zigzag-encoded signed varint (proto3 `sint32`).
    pub fn zigzag32(&mut self, v: i32) {
        let u = ((v << 1) ^ (v >> 31)) as u32;
        self.varint(u as u64);
    }

    /// Zigzag-encoded signed varint (proto3 `sint64`).
    pub fn zigzag64(&mut self, v: i64) {
        let u = ((v << 1) ^ (v >> 63)) as u64;
        self.varint(u);
    }

    /// Little-endian fixed 32-bit unsigned integer.
    pub fn fixed32(&mut self, v: u32) {
        self.buf.extend_from_slice(&v.to_le_bytes());
    }

    /// Little-endian fixed 64-bit unsigned integer.
    pub fn fixed64(&mut self, v: u64) {
        self.buf.extend_from_slice(&v.to_le_bytes());
    }

    /// IEEE 754 32-bit float, little-endian.
    pub fn float(&mut self, v: f32) {
        self.buf.extend_from_slice(&v.to_le_bytes());
    }

    /// IEEE 754 64-bit double, little-endian.
    pub fn double(&mut self, v: f64) {
        self.buf.extend_from_slice(&v.to_le_bytes());
    }

    /// UTF-8 length-prefixed string.
    pub fn string(&mut self, v: &str) {
        self.bytes(v.as_bytes());
    }

    /// Length-prefixed byte sequence.
    pub fn bytes(&mut self, v: &[u8]) {
        self.varint(v.len() as u64);
        self.raw(v);
    }

    /// Tag = (field_number << 3) | wire_type, encoded as a varint.
    ///
    /// Panics on out-of-range field numbers — that's a programmer error,
    /// not a wire-format failure.
    pub fn tag(&mut self, field_number: u32, wire_type: WireType) {
        assert!(
            (1..=0x1fff_ffff).contains(&field_number),
            "field number out of range: {field_number}"
        );
        self.varint(((field_number as u64) << 3) | (wire_type as u64));
    }
}

pub struct Reader<'a> {
    pub(crate) data: &'a [u8],
    pub pos: usize,
    /// Live recursion depth, incremented by `read_message` when entering a
    /// nested submessage and decremented on exit. The depth survives across
    /// `merge_field` calls so a `Message` impl that hands the same `Reader`
    /// to a fresh `read_message` cannot reset it to zero.
    pub(crate) depth: usize,
}

impl<'a> Reader<'a> {
    pub fn new(data: &'a [u8]) -> Self {
        Self { data, pos: 0, depth: 0 }
    }

    pub fn data(&self) -> &'a [u8] {
        self.data
    }

    pub fn eof(&self) -> bool {
        self.pos >= self.data.len()
    }

    pub fn remaining(&self) -> usize {
        self.data.len().saturating_sub(self.pos)
    }

    /// Read an unsigned varint, up to 10 bytes (uint64 range).
    pub fn varint(&mut self) -> Result<u64> {
        let mut result: u64 = 0;
        let mut shift = 0u32;
        for i in 0..10 {
            if self.pos >= self.data.len() {
                return Err(Error::TruncatedVarint);
            }
            let byte = self.data[self.pos];
            self.pos += 1;
            result |= ((byte & 0x7f) as u64) << shift;
            if byte & 0x80 == 0 {
                return Ok(result);
            }
            shift += 7;
            if i == 9 {
                return Err(Error::VarintTooLong);
            }
        }
        Err(Error::VarintTooLong)
    }

    /// Decode a zigzag varint as a 32-bit signed integer.
    pub fn zigzag32(&mut self) -> Result<i32> {
        let u = self.varint()? as u32;
        Ok(((u >> 1) as i32) ^ -((u & 1) as i32))
    }

    /// Decode a zigzag varint as a 64-bit signed integer.
    pub fn zigzag64(&mut self) -> Result<i64> {
        let u = self.varint()?;
        Ok(((u >> 1) as i64) ^ -((u & 1) as i64))
    }

    pub fn fixed32(&mut self) -> Result<u32> {
        if self.pos + 4 > self.data.len() {
            return Err(Error::TruncatedFixed32);
        }
        let v = u32::from_le_bytes(self.data[self.pos..self.pos + 4].try_into().unwrap());
        self.pos += 4;
        Ok(v)
    }

    pub fn fixed64(&mut self) -> Result<u64> {
        if self.pos + 8 > self.data.len() {
            return Err(Error::TruncatedFixed64);
        }
        let v = u64::from_le_bytes(self.data[self.pos..self.pos + 8].try_into().unwrap());
        self.pos += 8;
        Ok(v)
    }

    pub fn float(&mut self) -> Result<f32> {
        Ok(f32::from_bits(self.fixed32()?))
    }

    pub fn double(&mut self) -> Result<f64> {
        Ok(f64::from_bits(self.fixed64()?))
    }

    /// Length-prefixed bytes; returns a copy.
    pub fn bytes(&mut self) -> Result<Vec<u8>> {
        Ok(self.bytes_view()?.to_vec())
    }

    /// Length-prefixed bytes; returns a borrow into the underlying buffer.
    ///
    /// Guards against attacker-supplied length-prefix overflow per
    /// HARDENING.md §API contract item 3: a 10-byte varint of `2^64-1`
    /// would wrap `pos + len` to a small value and slip past a naive
    /// bounds check, then trip a slice-indexing panic. Compute the end
    /// offset with `checked_add` and reject before slicing.
    pub fn bytes_view(&mut self) -> Result<&'a [u8]> {
        let len = self.read_length()?;
        let end = self.pos + len;
        let view = &self.data[self.pos..end];
        self.pos = end;
        Ok(view)
    }

    /// Read a varint length and validate it fits in the remaining buffer.
    /// Returns the length as `usize` ready for slicing. Used by every
    /// length-delimited consumer (`bytes_view`, `skip`, `read_message`)
    /// so the overflow guard exists in exactly one place.
    fn read_length(&mut self) -> Result<usize> {
        let len = self.varint()?;
        let len = usize::try_from(len).map_err(|_| Error::TruncatedLengthDelim)?;
        let end = self.pos.checked_add(len).ok_or(Error::TruncatedLengthDelim)?;
        if end > self.data.len() {
            return Err(Error::TruncatedLengthDelim);
        }
        Ok(len)
    }

    /// UTF-8 length-prefixed string.
    pub fn string(&mut self) -> Result<String> {
        let bytes = self.bytes_view()?.to_vec();
        Ok(String::from_utf8(bytes)?)
    }

    /// Decode a tag varint into (field_number, wire_type).
    pub fn tag(&mut self) -> Result<(u32, WireType)> {
        let t = self.varint()?;
        let wire_type = WireType::from_u8((t & 0x7) as u8)?;
        let field_number = (t >> 3) as u32;
        if field_number == 0 {
            return Err(Error::InvalidTag(self.pos));
        }
        Ok((field_number, wire_type))
    }

    /// Skip the value of a field with the given wire type.
    pub fn skip(&mut self, wire_type: WireType) -> Result<()> {
        match wire_type {
            WireType::Varint => {
                self.varint()?;
            }
            WireType::Fixed64 => {
                if self.pos + 8 > self.data.len() {
                    return Err(Error::TruncatedFixed64);
                }
                self.pos += 8;
            }
            WireType::LengthDelimited => {
                let len = self.read_length()?;
                self.pos += len;
            }
            WireType::Fixed32 => {
                if self.pos + 4 > self.data.len() {
                    return Err(Error::TruncatedFixed32);
                }
                self.pos += 4;
            }
            WireType::StartGroup | WireType::EndGroup => {
                return Err(Error::GroupNotSupported);
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn round_trip_varint(v: u64) -> u64 {
        let mut w = Writer::new();
        w.varint(v);
        let bytes = w.finish();
        let mut r = Reader::new(&bytes);
        let out = r.varint().unwrap();
        assert!(r.eof());
        out
    }

    #[test]
    fn varint_encodes_zero_as_single_byte() {
        let mut w = Writer::new();
        w.varint(0);
        assert_eq!(w.finish(), vec![0]);
    }

    #[test]
    fn varint_round_trips_small_numbers() {
        for v in [0u64, 1, 127, 128, 255, 256, 16383, 16384] {
            assert_eq!(round_trip_varint(v), v);
        }
    }

    #[test]
    fn varint_round_trips_up_to_i64_max() {
        let v = i64::MAX as u64;
        assert_eq!(round_trip_varint(v), v);
    }

    #[test]
    fn varint_round_trips_full_uint64_range() {
        for v in [0u64, 1, 0x80, 0xff, 0xffff, 0xffff_ffff, u64::MAX] {
            assert_eq!(round_trip_varint(v), v);
        }
    }

    #[test]
    fn varint_encodes_150_as_canonical_proto_example() {
        let mut w = Writer::new();
        w.varint(150);
        assert_eq!(w.finish(), vec![0x96, 0x01]);
    }

    #[test]
    fn zigzag32_matches_proto3_spec() {
        let cases: &[(i32, u32)] = &[
            (0, 0),
            (-1, 1),
            (1, 2),
            (-2, 3),
            (2147483647, 4294967294),
            (-2147483648, 4294967295),
        ];
        for &(signed, encoded) in cases {
            let mut w = Writer::new();
            w.zigzag32(signed);
            let bytes = w.finish();
            let mut r = Reader::new(&bytes);
            assert_eq!(r.varint().unwrap() as u32, encoded);

            let mut r2 = Reader::new(&bytes);
            assert_eq!(r2.zigzag32().unwrap(), signed);
        }
    }

    #[test]
    fn zigzag64_round_trips_boundary_values() {
        for v in [0i64, -1, 1, -2, i64::MAX, i64::MIN] {
            let mut w = Writer::new();
            w.zigzag64(v);
            let bytes = w.finish();
        let mut r = Reader::new(&bytes);
            assert_eq!(r.zigzag64().unwrap(), v);
        }
    }

    #[test]
    fn fixed32_round_trips() {
        for v in [0u32, 1, 0x7fff_ffff, 0xffff_ffff] {
            let mut w = Writer::new();
            w.fixed32(v);
            let bytes = w.finish();
        let mut r = Reader::new(&bytes);
            assert_eq!(r.fixed32().unwrap(), v);
        }
    }

    #[test]
    fn fixed64_round_trips_uint64() {
        for v in [0u64, 1, 0xffff_ffff, u64::MAX] {
            let mut w = Writer::new();
            w.fixed64(v);
            let bytes = w.finish();
        let mut r = Reader::new(&bytes);
            assert_eq!(r.fixed64().unwrap(), v);
        }
    }

    #[test]
    fn float_and_double_round_trip() {
        let mut w = Writer::new();
        w.float(2.5);
        w.double(std::f64::consts::PI);
        let bytes = w.finish();
        let mut r = Reader::new(&bytes);
        assert!((r.float().unwrap() - 2.5).abs() < 1e-5);
        assert_eq!(r.double().unwrap(), std::f64::consts::PI);
    }

    #[test]
    fn utf8_strings_round_trip() {
        let mut w = Writer::new();
        w.string("héllo, 世界");
        let bytes = w.finish();
        let mut r = Reader::new(&bytes);
        assert_eq!(r.string().unwrap(), "héllo, 世界");
    }

    #[test]
    fn bytes_round_trip() {
        let mut w = Writer::new();
        w.bytes(&[0xde, 0xad, 0xbe, 0xef]);
        let bytes = w.finish();
        let mut r = Reader::new(&bytes);
        assert_eq!(r.bytes().unwrap(), vec![0xde, 0xad, 0xbe, 0xef]);
    }

    #[test]
    fn tag_for_field_1_varint_is_0x08() {
        let mut w = Writer::new();
        w.tag(1, WireType::Varint);
        assert_eq!(w.finish(), vec![0x08]);
    }

    #[test]
    fn tag_decodes_back_to_field_number_and_wire_type() {
        let mut w = Writer::new();
        w.tag(15, WireType::LengthDelimited);
        let bytes = w.finish();
        let mut r = Reader::new(&bytes);
        assert_eq!(r.tag().unwrap(), (15, WireType::LengthDelimited));
    }

    #[test]
    fn skip_handles_each_wire_type() {
        let mut w = Writer::new();
        w.tag(1, WireType::Varint);
        w.varint(150);
        w.tag(2, WireType::Fixed32);
        w.fixed32(0xdead_beef);
        w.tag(3, WireType::Fixed64);
        w.fixed64(0xdead_beef_cafe_babe);
        w.tag(4, WireType::LengthDelimited);
        w.string("skip me");
        w.tag(5, WireType::Varint);
        w.varint(7);

        let bytes = w.finish();
        let mut r = Reader::new(&bytes);
        let mut keep5: Option<u64> = None;
        while !r.eof() {
            let (num, wt) = r.tag().unwrap();
            if num == 5 {
                keep5 = Some(r.varint().unwrap());
            } else {
                r.skip(wt).unwrap();
            }
        }
        assert_eq!(keep5, Some(7));
    }

    #[test]
    fn truncated_varint_is_rejected() {
        let mut r = Reader::new(&[0x80]);
        assert!(matches!(r.varint(), Err(Error::TruncatedVarint)));
    }

    #[test]
    fn length_prefix_max_varint_does_not_overflow() {
        // tag(1, LengthDelimited) + 10-byte u64::MAX varint length.
        // Naive `pos + len > data.len()` would wrap and slip past the
        // bounds check, then panic on the slice. HARDENING.md §API
        // contract requires a clean reject.
        let mut bytes = vec![0x0a];
        // u64::MAX as a 10-byte varint
        bytes.extend_from_slice(&[0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0x01]);
        let mut r = Reader::new(&bytes);
        let (_, _) = r.tag().unwrap();
        assert!(matches!(r.bytes_view(), Err(Error::TruncatedLengthDelim)));
    }

    #[test]
    fn length_prefix_overflow_during_skip_does_not_panic() {
        let mut bytes = vec![0x0a];
        bytes.extend_from_slice(&[0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0x01]);
        let mut r = Reader::new(&bytes);
        let (_, wt) = r.tag().unwrap();
        assert!(matches!(r.skip(wt), Err(Error::TruncatedLengthDelim)));
    }
}
