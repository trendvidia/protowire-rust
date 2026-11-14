// SPDX-License-Identifier: MIT
// Copyright (c) 2026 TrendVidia, LLC.
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

/// HARDENING.md `MaxMessageSize` — caps the total input to one decode call,
/// checked before anything is read: peak memory is a multiple of the input,
/// so the input is what bounds it.
pub const MAX_MESSAGE_SIZE: usize = 64 << 20;

/// HARDENING.md `MaxRepeatedCount` — caps the element count of any repeated
/// or map field. Elements arrive one record each, so the count is bounded
/// by [`MAX_MESSAGE_SIZE`] transitively; the check keeps the bound when a
/// caller raises the message cap alone. A `Message` impl enforces it by
/// appending through [`Reader::push_element`] (and checking a map with
/// [`Reader::check_map`]) rather than pushing directly.
pub const MAX_REPEATED_COUNT: usize = MAX_MESSAGE_SIZE;

/// HARDENING.md `MaxNumericLiteralDigits` — bounds the magnitude of a
/// `pxf.Decimal`'s scale (a digit count) and the digits a `pxf.BigFloat`'s
/// prec implies (prec 13603 at the default), as protowire-go's
/// `pb.MaxNumericLiteralDigits` does (protowire#279, #499).
pub const MAX_NUMERIC_LITERAL_DIGITS: usize = 4096;

/// HARDENING.md `MaxBigFloatExponent` — bounds the magnitude of a
/// `pxf.BigFloat` value's binary exponent, as protowire-go's
/// `pb.MaxBigFloatExponent` does (protowire#281).
pub const MAX_BIG_FLOAT_EXPONENT: usize = 16384;

/// The per-call limits of one decode (draft -01 § Mandatory Limits: every
/// limit but `MaxVarintBytes` is "configurable per call by the calling
/// application"). The default is the HARDENING constants; lower a field
/// to tighten one call: `Limits { max_message_size: 1 << 20,
/// ..Limits::default() }`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Limits {
    /// [`MAX_MESSAGE_SIZE`]: total input to this call.
    pub max_message_size: usize,
    /// [`MAX_NESTING_DEPTH`]: submessage / group / map-entry recursion.
    pub max_nesting_depth: usize,
    /// [`MAX_REPEATED_COUNT`]: elements of a repeated or map field.
    pub max_repeated_count: usize,
    /// [`MAX_NUMERIC_LITERAL_DIGITS`]: a `pxf.Decimal`'s scale and the
    /// digits a `pxf.BigFloat`'s prec implies ([`crate::big`]).
    pub max_numeric_literal_digits: usize,
    /// [`MAX_BIG_FLOAT_EXPONENT`]: a `pxf.BigFloat` value's binary exponent.
    pub max_big_float_exponent: usize,
}

impl Default for Limits {
    fn default() -> Self {
        Self {
            max_message_size: MAX_MESSAGE_SIZE,
            max_nesting_depth: MAX_NESTING_DEPTH,
            max_repeated_count: MAX_REPEATED_COUNT,
            max_numeric_literal_digits: MAX_NUMERIC_LITERAL_DIGITS,
            max_big_float_exponent: MAX_BIG_FLOAT_EXPONENT,
        }
    }
}

/// What a [`Reader`] could not read, as protowire-go's pb codec names it in
/// `corrupt <kind>`. A value is corrupt when its bytes stop before it ends,
/// or when a varint runs past 64 bits.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum Kind {
    /// A field's tag, or field number 0.
    Tag,
    Bool,
    /// A signed integer, plain or zigzag.
    Int,
    Uint,
    Float64,
    Float32,
    /// A `fixed32` read as such; protowire-go's codec has no such field.
    Fixed32,
    /// A `fixed64` read as such; protowire-go's codec has no such field.
    Fixed64,
    String,
    Bytes,
    EmbeddedMessage,
    /// The length of a packed repeated field.
    PackedField,
    MapEntry,
    TagInMapEntry,
}

impl std::fmt::Display for Kind {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Kind::Tag => "tag",
            Kind::Bool => "bool",
            Kind::Int => "int",
            Kind::Uint => "uint",
            Kind::Float64 => "float64",
            Kind::Float32 => "float32",
            Kind::Fixed32 => "fixed32",
            Kind::Fixed64 => "fixed64",
            Kind::String => "string",
            Kind::Bytes => "bytes",
            Kind::EmbeddedMessage => "embedded message",
            Kind::PackedField => "packed field",
            Kind::MapEntry => "map entry",
            Kind::TagInMapEntry => "tag in map entry",
        })
    }
}

/// A decode error, worded as protowire-go's `pb.Unmarshal` words it (#144).
/// An error inside a field the message names ([`crate::Message::field_name`])
/// comes wrapped in [`Error::Field`], one wrapper per message it sits in:
/// `field Error: field Code: invalid UTF-8 in string`.
#[derive(Debug, Error)]
#[non_exhaustive]
pub enum Error {
    /// A value of this kind stops before it ends, or its varint runs past
    /// 64 bits.
    #[error("corrupt {0}")]
    Corrupt(Kind),
    /// A field the message does not name could not be read or skipped.
    #[error("corrupt field {0}")]
    CorruptField(u32),
    /// A map entry field other than the key (1) and value (2) could not be
    /// skipped.
    #[error("corrupt unknown field {0} in map entry")]
    CorruptMapEntryField(u32),
    #[error("invalid UTF-8 in string")]
    InvalidUtf8,
    /// An error inside the field `name`, as [`crate::Message::field_name`]
    /// names it.
    #[error("field {name}: {inner}")]
    Field {
        name: &'static str,
        inner: Box<Error>,
    },
    /// An error in a map entry's key.
    #[error("map key: {0}")]
    MapKey(Box<Error>),
    /// An error in a map entry's value.
    #[error("map value: {0}")]
    MapValue(Box<Error>),
    #[error("unknown wire type {0}")]
    UnknownWireType(u8),
    #[error("nesting depth exceeds MaxNestingDepth={0}")]
    DepthExceeded(usize),
    #[error("input of {len} bytes exceeds MaxMessageSize={max}")]
    MessageTooLarge { len: usize, max: usize },
    #[error("repeated field exceeds MaxRepeatedCount={0}")]
    RepeatedCountExceeded(usize),
    #[error("map field exceeds MaxRepeatedCount={0}")]
    MapCountExceeded(usize),
    /// A big number ([`crate::big`]) that a limit refuses, or that has no
    /// wire form; the text is protowire-go's.
    #[error("{0}")]
    BigNumber(String),
    /// The [`crate::UnmarshalOptions::validator`] reported violations; the
    /// error holds its report.
    #[error(transparent)]
    Check(protowire_check::Error),
    /// The [`crate::UnmarshalOptions::validator`] could not run: its own
    /// error, as it returned it.
    #[error(transparent)]
    Validator(protowire_check::BoxError),
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

/// An output buffer for one message. It also counts how deep it sits, as a
/// [`Reader`] counts on the way back in: the root is depth 0 and every
/// submessage or map entry written through [`crate::write_message`] or
/// [`crate::write_map_entry`] is one descent. A value that would go past
/// `max_nesting_depth` is not written, and the writer remembers it, so
/// [`Writer::finish_checked`] (and [`crate::marshal`]) refuse the message
/// a reader at the same limit would refuse (draft -01 § Recursion,
/// protowire#502).
#[derive(Debug)]
pub struct Writer {
    buf: Vec<u8>,
    depth: usize,
    max_depth: usize,
    exceeded: bool,
    /// The first value refused for something other than its depth, which
    /// [`Writer::finish_checked`] returns.
    refused: Option<Error>,
}

impl Default for Writer {
    fn default() -> Self {
        Self::with_max_nesting_depth(MAX_NESTING_DEPTH)
    }
}

impl Writer {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn with_capacity(cap: usize) -> Self {
        Self {
            buf: Vec::with_capacity(cap),
            ..Self::default()
        }
    }

    /// A root writer that refuses values nested past `max_nesting_depth`.
    pub fn with_max_nesting_depth(max_nesting_depth: usize) -> Self {
        Self {
            buf: Vec::new(),
            depth: 0,
            max_depth: max_nesting_depth,
            exceeded: false,
            refused: None,
        }
    }

    /// A writer for a value one descent below this one, or `None`, with
    /// the refusal recorded here, when that descent passes the limit.
    pub(crate) fn nested(&mut self) -> Option<Writer> {
        if self.depth >= self.max_depth {
            self.exceeded = true;
            return None;
        }
        Some(Writer {
            buf: Vec::new(),
            depth: self.depth + 1,
            max_depth: self.max_depth,
            exceeded: false,
            refused: None,
        })
    }

    /// The bytes of a nested writer, or `None`, with the refusal carried
    /// up, when something under it went past the limit.
    pub(crate) fn finish_nested(&mut self, inner: Writer) -> Option<Vec<u8>> {
        if let Some(err) = inner.refused {
            self.refused.get_or_insert(err);
        }
        if inner.exceeded {
            self.exceeded = true;
            return None;
        }
        Some(inner.buf)
    }

    /// Refuse the message being written: [`Writer::finish_checked`] (and
    /// [`crate::marshal`]) return `err`, the first one recorded, instead of
    /// the bytes. For a value a `Message` impl cannot write, such as a
    /// [`crate::big::Decimal`] whose scale a reader would refuse.
    pub fn refuse(&mut self, err: Error) {
        self.refused.get_or_insert(err);
    }

    /// Whether a value nested past the limit was refused. The bytes
    /// [`Writer::finish`] returns then lack it.
    pub fn depth_exceeded(&self) -> bool {
        self.exceeded
    }

    /// The bytes written. They lack any value refused for its depth; see
    /// [`Writer::finish_checked`].
    pub fn finish(self) -> Vec<u8> {
        self.buf
    }

    /// The bytes written, or the refusal: the first error passed to
    /// [`Writer::refuse`], or [`Error::DepthExceeded`] if a value nested
    /// past the limit was refused.
    pub fn finish_checked(self) -> Result<Vec<u8>> {
        if let Some(err) = self.refused {
            return Err(err);
        }
        if self.exceeded {
            return Err(Error::DepthExceeded(self.max_depth));
        }
        Ok(self.buf)
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

    /// IEEE 754 32-bit float, little-endian. A NaN is written as the
    /// canonical quiet NaN, `0x7FC00000`, whatever its sign and payload, as
    /// protowire-go's `Marshal` writes a struct field (draft -01 § Numeric
    /// Literals, protowire-go#241). [`Writer::fixed32`] writes bits as given.
    pub fn float(&mut self, v: f32) {
        let bits = if v.is_nan() { 0x7FC0_0000 } else { v.to_bits() };
        self.fixed32(bits);
    }

    /// IEEE 754 64-bit double, little-endian. A NaN is written as the
    /// canonical quiet NaN, `0x7FF8000000000000`, whatever its sign and
    /// payload, as protowire-go's `Marshal` writes a struct field (draft -01
    /// § Numeric Literals, protowire-go#241). [`Writer::fixed64`] writes bits
    /// as given.
    pub fn double(&mut self, v: f64) {
        let bits = if v.is_nan() {
            0x7FF8_0000_0000_0000
        } else {
            v.to_bits()
        };
        self.fixed64(bits);
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
    /// This decode's limits; `read_message` and the element helpers below
    /// read them, and a packed sub-reader inherits them.
    pub(crate) limits: Limits,
    /// The field number of the last tag read, so [`Reader::skip`] can find
    /// the end of a group that field starts.
    last_field: u32,
}

/// The largest field number a tag can carry (2^29 - 1).
const MAX_FIELD_NUMBER: u64 = (1 << 29) - 1;

impl<'a> Reader<'a> {
    pub fn new(data: &'a [u8]) -> Self {
        Self::with_limits(data, Limits::default())
    }

    /// A reader whose nested decodes and element helpers enforce `limits`.
    /// The message-size cap is the caller's to check on the whole input
    /// ([`crate::codec::unmarshal_with`] does); a reader over a slice of
    /// it does not re-check.
    pub fn with_limits(data: &'a [u8], limits: Limits) -> Self {
        Self {
            data,
            pos: 0,
            depth: 0,
            limits,
            last_field: 0,
        }
    }

    /// This decode's limits.
    pub fn limits(&self) -> Limits {
        self.limits
    }

    /// Append one element of a repeated field, refusing it when the field
    /// already holds `max_repeated_count` elements (HARDENING.md
    /// `MaxRepeatedCount`) — before the element is added. `Message` impls
    /// use this in place of `vec.push` for every repeated field.
    pub fn push_element<T>(&self, list: &mut Vec<T>, elem: T) -> Result<()> {
        self.check_repeated(list.len())?;
        list.push(elem);
        Ok(())
    }

    /// Refuse to add an element to a repeated field that already holds
    /// `len` elements when `len` has reached `max_repeated_count`.
    pub fn check_repeated(&self, len: usize) -> Result<()> {
        if len >= self.limits.max_repeated_count {
            return Err(Error::RepeatedCountExceeded(self.limits.max_repeated_count));
        }
        Ok(())
    }

    /// [`Reader::check_repeated`] for a map, worded as protowire-go words
    /// it: check with the map's length before inserting a key it does not
    /// already hold.
    pub fn check_map(&self, len: usize) -> Result<()> {
        if len >= self.limits.max_repeated_count {
            return Err(Error::MapCountExceeded(self.limits.max_repeated_count));
        }
        Ok(())
    }

    /// The payload of a packed repeated field as its own reader, sharing
    /// this reader's limits and depth, so the elements can be read with the
    /// scalar methods and appended through [`Reader::push_element`]. The
    /// length prefix is validated against the buffer before the slice is
    /// taken.
    pub fn packed(&mut self) -> Result<Reader<'a>> {
        let view = self.len_view(Kind::PackedField)?;
        Ok(self.over(view))
    }

    /// A reader over `data` that shares this one's depth and limits.
    pub(crate) fn over(&self, data: &'a [u8]) -> Reader<'a> {
        Reader {
            data,
            pos: 0,
            depth: self.depth,
            limits: self.limits,
            last_field: 0,
        }
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

    /// A varint of at most 10 bytes whose value fits 64 bits, as
    /// protobuf-go's `ConsumeVarint` reads one: a tenth byte above 1 sets a
    /// bit past the 64th, and is refused.
    fn raw_varint(&mut self) -> Option<u64> {
        let mut result: u64 = 0;
        for i in 0..10 {
            let byte = *self.data.get(self.pos)?;
            self.pos += 1;
            if i == 9 && byte > 1 {
                return None;
            }
            result |= ((byte & 0x7f) as u64) << (7 * i);
            if byte & 0x80 == 0 {
                return Some(result);
            }
        }
        None
    }

    fn varint_as(&mut self, kind: Kind) -> Result<u64> {
        self.raw_varint().ok_or(Error::Corrupt(kind))
    }

    /// Read an unsigned varint (uint64 range).
    pub fn varint(&mut self) -> Result<u64> {
        self.varint_as(Kind::Uint)
    }

    /// A `bool`: any varint other than 0 is true.
    pub fn bool(&mut self) -> Result<bool> {
        Ok(self.varint_as(Kind::Bool)? != 0)
    }

    /// A proto3 `int64`: the varint as signed.
    pub fn int64(&mut self) -> Result<i64> {
        Ok(self.varint_as(Kind::Int)? as i64)
    }

    /// A proto3 `int32`: the varint's low 32 bits as signed, so a negative
    /// value written as 10 bytes reads back.
    pub fn int32(&mut self) -> Result<i32> {
        Ok(self.varint_as(Kind::Int)? as i32)
    }

    /// A proto3 `uint64`.
    pub fn uint64(&mut self) -> Result<u64> {
        self.varint_as(Kind::Uint)
    }

    /// A proto3 `uint32`: the varint's low 32 bits.
    pub fn uint32(&mut self) -> Result<u32> {
        Ok(self.varint_as(Kind::Uint)? as u32)
    }

    /// Decode a zigzag varint as a 32-bit signed integer (`sint32`).
    pub fn zigzag32(&mut self) -> Result<i32> {
        let u = self.varint_as(Kind::Int)? as u32;
        Ok(((u >> 1) as i32) ^ -((u & 1) as i32))
    }

    /// Decode a zigzag varint as a 64-bit signed integer (`sint64`).
    pub fn zigzag64(&mut self) -> Result<i64> {
        let u = self.varint_as(Kind::Int)?;
        Ok(((u >> 1) as i64) ^ -((u & 1) as i64))
    }

    fn take<const N: usize>(&mut self, kind: Kind) -> Result<[u8; N]> {
        let end = self.pos.checked_add(N).filter(|&e| e <= self.data.len());
        let end = end.ok_or(Error::Corrupt(kind))?;
        let bytes = self.data[self.pos..end].try_into().unwrap();
        self.pos = end;
        Ok(bytes)
    }

    pub fn fixed32(&mut self) -> Result<u32> {
        Ok(u32::from_le_bytes(self.take(Kind::Fixed32)?))
    }

    pub fn fixed64(&mut self) -> Result<u64> {
        Ok(u64::from_le_bytes(self.take(Kind::Fixed64)?))
    }

    pub fn float(&mut self) -> Result<f32> {
        Ok(f32::from_bits(u32::from_le_bytes(
            self.take(Kind::Float32)?,
        )))
    }

    pub fn double(&mut self) -> Result<f64> {
        Ok(f64::from_bits(u64::from_le_bytes(
            self.take(Kind::Float64)?,
        )))
    }

    /// Length-prefixed bytes; returns a copy.
    pub fn bytes(&mut self) -> Result<Vec<u8>> {
        Ok(self.bytes_view()?.to_vec())
    }

    /// Length-prefixed bytes; returns a borrow into the underlying buffer.
    pub fn bytes_view(&mut self) -> Result<&'a [u8]> {
        self.len_view(Kind::Bytes)
    }

    /// A varint length and the bytes it covers, or `Corrupt(kind)` when
    /// they run past the buffer.
    ///
    /// Guards against attacker-supplied length-prefix overflow per
    /// HARDENING.md §API contract item 3: a 10-byte varint of `2^64-1`
    /// would wrap `pos + len` to a small value and slip past a naive
    /// bounds check, then trip a slice-indexing panic. The end offset is
    /// computed with `checked_add` and refused before slicing. Every
    /// length-delimited consumer goes through here, so the guard exists in
    /// exactly one place.
    pub(crate) fn len_view(&mut self, kind: Kind) -> Result<&'a [u8]> {
        let len = self.varint_as(kind)?;
        let end = usize::try_from(len)
            .ok()
            .and_then(|len| self.pos.checked_add(len))
            .filter(|&end| end <= self.data.len())
            .ok_or(Error::Corrupt(kind))?;
        let view = &self.data[self.pos..end];
        self.pos = end;
        Ok(view)
    }

    /// UTF-8 length-prefixed string, refused when it is not valid UTF-8
    /// (HARDENING.md § UTF-8).
    pub fn string(&mut self) -> Result<String> {
        let bytes = self.len_view(Kind::String)?;
        std::str::from_utf8(bytes)
            .map(str::to_owned)
            .map_err(|_| Error::InvalidUtf8)
    }

    /// Decode a tag varint into (field_number, wire_type), as protobuf-go's
    /// `ConsumeTag` does: a field number of 0 or past 2^29 - 1 is a corrupt
    /// tag. Wire types 6 and 7 name no encoding, so the field cannot be
    /// read: `corrupt field N`, protowire-go's text for an unknown field.
    pub fn tag(&mut self) -> Result<(u32, WireType)> {
        let t = self.varint_as(Kind::Tag)?;
        let field_number = t >> 3;
        if field_number == 0 || field_number > MAX_FIELD_NUMBER {
            return Err(Error::Corrupt(Kind::Tag));
        }
        let field_number = field_number as u32;
        self.last_field = field_number;
        let wire_type =
            WireType::from_u8((t & 0x7) as u8).map_err(|_| Error::CorruptField(field_number))?;
        Ok((field_number, wire_type))
    }

    /// Skip the value of a field with the given wire type, the one of the
    /// tag just read. A group is skipped to the end-group tag of the same
    /// field, and each group inside it is one descent (HARDENING.md §
    /// Recursion). An end-group tag with no group open cannot be skipped.
    pub fn skip(&mut self, wire_type: WireType) -> Result<()> {
        match wire_type {
            WireType::Varint => {
                self.raw_varint().ok_or(Error::Corrupt(Kind::Uint))?;
            }
            WireType::Fixed64 => {
                self.take::<8>(Kind::Fixed64)?;
            }
            WireType::LengthDelimited => {
                self.len_view(Kind::Bytes)?;
            }
            WireType::Fixed32 => {
                self.take::<4>(Kind::Fixed32)?;
            }
            WireType::StartGroup => self.skip_group(self.last_field)?,
            WireType::EndGroup => return Err(Error::Corrupt(Kind::Tag)),
        }
        Ok(())
    }

    /// Skips the fields of the group `field` starts, through its end-group
    /// tag.
    fn skip_group(&mut self, field: u32) -> Result<()> {
        if self.depth >= self.limits.max_nesting_depth {
            return Err(Error::DepthExceeded(self.limits.max_nesting_depth));
        }
        self.depth += 1;
        let result = (|| loop {
            let (num, wt) = self.tag()?;
            if wt == WireType::EndGroup {
                return if num == field {
                    Ok(())
                } else {
                    Err(Error::Corrupt(Kind::Tag))
                };
            }
            self.skip(wt)?;
        })();
        self.depth -= 1;
        result
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
        assert!(matches!(r.varint(), Err(Error::Corrupt(Kind::Uint))));
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
        assert!(matches!(r.bytes_view(), Err(Error::Corrupt(Kind::Bytes))));
    }

    #[test]
    fn length_prefix_overflow_during_skip_does_not_panic() {
        let mut bytes = vec![0x0a];
        bytes.extend_from_slice(&[0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0x01]);
        let mut r = Reader::new(&bytes);
        let (_, wt) = r.tag().unwrap();
        assert!(matches!(r.skip(wt), Err(Error::Corrupt(Kind::Bytes))));
    }
}
