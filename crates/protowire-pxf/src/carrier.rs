// SPDX-License-Identifier: MIT
// Copyright (c) 2026 TrendVidia, LLC.
//! The schema-extension annotation carrier (RFC-001 §8.1). Mirrors
//! `encoding/pxf/carrier.go` in protowire-go.
//!
//! PXF has two spellings for the same two semantics. The bracket form
//! `[(pxf.required) = true]` / `[(pxf.default) = "…"]` lowers to extension
//! numbers 1314 / 1315. The annotation form `@required` / `@default(v)`
//! lowers to something else entirely: one `AnnotationList` at extension
//! number 1327 on the field's options, holding one entry per `@` use site.
//!
//! The two are disjoint by construction: a compiler emits the one the author
//! wrote and must not synthesize the other (RFC-001 §8.5). So a runtime that
//! reads only 1314 / 1315 binds a migrated schema without error and without
//! enforcement, and closing that is a runtime's job.
//!
//! The carrier's wire bytes are walked directly rather than decoded through
//! `protowire/schema/v1/descriptor.proto`: the shape is small and frozen by
//! STABILITY.md promise 3, and a schema's descriptors needn't include that
//! file, in which case 1327 is an unknown field.

use prost_reflect::{FieldDescriptor, Kind, Value};

use crate::bigfloat::{big_float_digits, big_float_value_exponent, format_big_float};
use crate::bignum::{format_big_int, format_decimal};
use crate::limits::{MAX_BIG_FLOAT_EXPONENT, MAX_NUMERIC_LITERAL_DIGITS};

/// The carrier's extension number. Every kind-specific carrier shares it:
/// the per-kind names exist to be unique in one proto package, not to have
/// distinct numbers.
const EXT_ANNOTATIONS: u32 = 1327;

/// The two annotations this binder acts on, by the fully-qualified name the
/// lowering pass writes into `Annotation.name`.
const ANNOTATION_REQUIRED: &str = "protowire.schema.v1.required";
const ANNOTATION_DEFAULT: &str = "protowire.schema.v1.default";

// Field numbers from protowire/proto/schema/v1/descriptor.proto.
const LIST_ENTRIES: u32 = 1;
const ANNOTATION_NAME: u32 = 1;
const ANNOTATION_ARGS: u32 = 2;
const ARG_NAME: u32 = 1;
const ARG_STRING: u32 = 10;
const ARG_INT: u32 = 11;
const ARG_DOUBLE: u32 = 12;
const ARG_BOOL: u32 = 13;
const ARG_BYTES: u32 = 14;
const ARG_LITERAL: u32 = 15;
const ARG_BIG_INT: u32 = 16;
const ARG_DECIMAL: u32 = 17;
const ARG_BIG_FLOAT: u32 = 18;
const LITERAL_ENUM_VALUE: u32 = 1;
const ENUM_VALUE_NAME: u32 = 2;

const WIRE_VARINT: u8 = 0;
const WIRE_FIXED64: u8 = 1;
const WIRE_BYTES: u8 = 2;
const WIRE_FIXED32: u8 = 5;

/// What the carrier says about one field: the part of the annotation
/// surface this binder enforces. `@validate`, `@description` and the rest
/// are skipped; they belong to other tools.
#[derive(Debug, Default, PartialEq, Eq)]
pub(crate) struct Carrier {
    pub required: bool,
    /// `@default`'s argument reduced to the literal the bracket form
    /// carries, so everything downstream is shared between the spellings.
    pub default: Option<String>,
    /// Why an `@default` the field does carry reduces to no literal. It
    /// becomes a violation rather than a dropped annotation, which would be
    /// the silent non-enforcement this path exists to end.
    pub default_problem: Option<String>,
}

/// The carrier on `fd`, or `None` when it has none.
pub(crate) fn read_carrier(fd: &FieldDescriptor) -> Option<Carrier> {
    let opts = fd.options();
    let mut bytes = Vec::new();
    for field in opts.unknown_fields() {
        if field.number() == EXT_ANNOTATIONS {
            let mut encoded = Vec::new();
            field.encode(&mut encoded);
            if let Some(f) = next_field(&encoded) {
                bytes.extend_from_slice(f.bytes);
            }
        }
    }
    for (ext, value) in opts.extensions() {
        if ext.number() == EXT_ANNOTATIONS {
            if let Value::Message(list) = value {
                bytes.extend_from_slice(&prost::Message::encode_to_vec(list));
            }
        }
    }
    (!bytes.is_empty()).then(|| parse_annotation_list(&bytes, fd))
}

/// Walk the carrier's entries. Unparseable bytes stop the walk and keep what
/// was read before them: reporting a schema as unannotated because its last
/// entry was truncated is the failure this path exists to remove.
fn parse_annotation_list(mut b: &[u8], fd: &FieldDescriptor) -> Carrier {
    let mut got = Carrier::default();
    while let Some(f) = next_field(b) {
        if f.number == LIST_ENTRIES && f.wire == WIRE_BYTES {
            got.apply_annotation(f.bytes, fd);
        }
        b = &b[f.len..];
        if b.is_empty() {
            break;
        }
    }
    got
}

impl Carrier {
    fn apply_annotation(&mut self, mut b: &[u8], fd: &FieldDescriptor) {
        let mut name: &[u8] = &[];
        let mut args: Vec<&[u8]> = Vec::new();
        while let Some(f) = next_field(b) {
            match (f.number, f.wire) {
                (ANNOTATION_NAME, WIRE_BYTES) => name = f.bytes,
                (ANNOTATION_ARGS, WIRE_BYTES) => args.push(f.bytes),
                _ => {}
            }
            b = &b[f.len..];
            if b.is_empty() {
                break;
            }
        }
        if name == ANNOTATION_REQUIRED.as_bytes() {
            self.required = true;
        } else if name == ANNOTATION_DEFAULT.as_bytes() {
            if self.default.is_some() || self.default_problem.is_some() {
                return; // first @default wins; a second is the compiler's to reject
            }
            let Some(arg) = default_arg(&args) else {
                self.default_problem = Some("@default carries no value argument".to_string());
                return;
            };
            match arg_literal(arg, fd) {
                Ok(lit) => self.default = Some(lit),
                Err(problem) => self.default_problem = Some(problem),
            }
        }
    }
}

/// `default`'s single `value` argument: the first positional one, or the
/// first named `value`.
fn default_arg<'a>(args: &[&'a [u8]]) -> Option<&'a [u8]> {
    args.iter()
        .copied()
        .find(|a| matches!(arg_name(a), b"" | b"value"))
}

fn arg_name(mut b: &[u8]) -> &[u8] {
    while let Some(f) = next_field(b) {
        if f.number == ARG_NAME && f.wire == WIRE_BYTES {
            return f.bytes;
        }
        b = &b[f.len..];
        if b.is_empty() {
            break;
        }
    }
    &[]
}

/// Reduce one `AnnotationArg` to the literal the bracket form would have
/// carried, or say why no single literal denotes it.
///
/// `fd` decides one case the bytes can't: `AnnotationArg` has no unsigned
/// variant, and the lowering pass carries a large unsigned literal through
/// `int64` to keep its bits, so 18446744073709551615 arrives as `-1`.
fn arg_literal(mut b: &[u8], fd: &FieldDescriptor) -> Result<String, String> {
    while !b.is_empty() {
        let f = next_field(b).ok_or("@default's value is not well-formed")?;
        match (f.number, f.wire) {
            (ARG_STRING, WIRE_BYTES) => {
                return Ok(String::from_utf8_lossy(f.bytes).into_owned());
            }
            (ARG_INT, WIRE_VARINT) => {
                return Ok(if unsigned_target(fd) {
                    f.value.to_string()
                } else {
                    (f.value as i64).to_string()
                });
            }
            (ARG_DOUBLE, WIRE_FIXED64) => return Ok(f64::from_bits(f.value).to_string()),
            (ARG_BOOL, WIRE_VARINT) => return Ok((f.value != 0).to_string()),
            (ARG_BYTES, WIRE_BYTES) => return Ok(crate::format::encode_base64(f.bytes)),
            (ARG_BIG_INT, WIRE_BYTES) => return big_int_literal(f.bytes),
            (ARG_DECIMAL, WIRE_BYTES) => return decimal_literal(f.bytes),
            (ARG_BIG_FLOAT, WIRE_BYTES) => return big_float_literal(f.bytes),
            (ARG_LITERAL, WIRE_BYTES) => return literal_value(f.bytes),
            _ => {}
        }
        b = &b[f.len..];
    }
    Err("@default's value is empty".to_string())
}

/// `pxf.BigInt { abs = 1, negative = 2 }`.
fn big_int_literal(mut b: &[u8]) -> Result<String, String> {
    let (mut abs, mut negative): (&[u8], bool) = (&[], false);
    while !b.is_empty() {
        let f = next_field(b).ok_or("@default's big_int_value is not well-formed")?;
        match f.number {
            1 => abs = f.bytes,
            2 => negative = f.value != 0,
            _ => {}
        }
        b = &b[f.len..];
    }
    format_big_int(abs, negative).map_err(|e| format!("@default's big_int_value: {e}"))
}

/// `pxf.Decimal { unscaled = 1, scale = 2, negative = 3 }`. `scale` is a
/// plain varint, not zigzag.
fn decimal_literal(mut b: &[u8]) -> Result<String, String> {
    let (mut unscaled, mut scale, mut negative): (&[u8], i32, bool) = (&[], 0, false);
    while !b.is_empty() {
        let f = next_field(b).ok_or("@default's decimal_value is not well-formed")?;
        match f.number {
            1 => unscaled = f.bytes,
            2 if f.wire == WIRE_VARINT => scale = f.value as i32,
            3 => negative = f.value != 0,
            _ => {}
        }
        b = &b[f.len..];
    }
    if scale.unsigned_abs() as usize > MAX_NUMERIC_LITERAL_DIGITS {
        return Err(format!(
            "@default's decimal_value has scale {scale}, and MaxNumericLiteralDigits={MAX_NUMERIC_LITERAL_DIGITS} bounds its magnitude"
        ));
    }
    format_decimal(unscaled, scale, negative).map_err(|e| format!("@default's decimal_value: {e}"))
}

/// `pxf.BigFloat { mantissa = 1, exponent = 2, prec = 3, negative = 4 }`.
fn big_float_literal(mut b: &[u8]) -> Result<String, String> {
    let (mut mantissa, mut exponent, mut prec, mut negative): (&[u8], i32, u32, bool) =
        (&[], 0, 0, false);
    while !b.is_empty() {
        let f = next_field(b).ok_or("@default's big_float_value is not well-formed")?;
        match f.number {
            1 => mantissa = f.bytes,
            2 if f.wire == WIRE_VARINT => exponent = f.value as i32,
            3 => prec = f.value as u32,
            4 => negative = f.value != 0,
            _ => {}
        }
        b = &b[f.len..];
    }
    if prec == 0 {
        return Err("@default's big_float_value declares no precision".to_string());
    }
    if let Some(e) = big_float_value_exponent(mantissa, exponent) {
        if e.unsigned_abs() > MAX_BIG_FLOAT_EXPONENT as u64 {
            return Err(format!(
                "@default's big_float_value has binary exponent {e}, and MaxBigFloatExponent={MAX_BIG_FLOAT_EXPONENT} bounds its magnitude"
            ));
        }
    }
    let digits = big_float_digits(prec);
    if digits > MAX_NUMERIC_LITERAL_DIGITS as u64 {
        return Err(format!(
            "@default's big_float_value has prec {prec}, which renders to {digits} digits; MaxNumericLiteralDigits={MAX_NUMERIC_LITERAL_DIGITS}"
        ));
    }
    format_big_float(mantissa, exponent, prec, negative)
        .map_err(|e| format!("@default's big_float_value: {e}"))
}

/// Reduce a `Literal`. Only the resolved enum reference has a literal: a
/// message or list literal denotes what no single literal can.
fn literal_value(mut b: &[u8]) -> Result<String, String> {
    while !b.is_empty() {
        let f = next_field(b).ok_or("@default's value is not well-formed")?;
        if f.number == LITERAL_ENUM_VALUE && f.wire == WIRE_BYTES {
            return enum_value_name(f.bytes).ok_or_else(|| {
                "@default names an enum value the lowering pass left unresolved".to_string()
            });
        }
        b = &b[f.len..];
    }
    Err("@default carries a message or list literal, and a default is exactly one literal".into())
}

fn enum_value_name(mut b: &[u8]) -> Option<String> {
    while !b.is_empty() {
        let f = next_field(b)?;
        if f.number == ENUM_VALUE_NAME && f.wire == WIRE_BYTES {
            return Some(String::from_utf8_lossy(f.bytes).into_owned());
        }
        b = &b[f.len..];
    }
    None
}

/// Whether `fd`'s default literal reads as an unsigned integer.
fn unsigned_target(fd: &FieldDescriptor) -> bool {
    match fd.kind() {
        Kind::Uint32 | Kind::Fixed32 | Kind::Uint64 | Kind::Fixed64 => true,
        Kind::Message(m) => matches!(
            m.full_name(),
            "google.protobuf.UInt32Value" | "google.protobuf.UInt64Value"
        ),
        _ => false,
    }
}

/// One field of a wire message. Descriptors come from outside, so nothing
/// here trusts a length.
struct WireField<'a> {
    number: u32,
    wire: u8,
    /// The payload of a varint or fixed field.
    value: u64,
    /// The payload of a length-delimited field.
    bytes: &'a [u8],
    /// Bytes this field took.
    len: usize,
}

fn read_varint(b: &[u8]) -> Option<(u64, usize)> {
    let mut v = 0u64;
    for (i, &byte) in b.iter().take(10).enumerate() {
        v |= u64::from(byte & 0x7f) << (7 * i);
        if byte & 0x80 == 0 {
            return Some((v, i + 1));
        }
    }
    None
}

/// The field `b` begins with, or `None` when it doesn't begin a well-formed
/// one. Groups are not produced by the lowering pass and count as malformed.
fn next_field(b: &[u8]) -> Option<WireField<'_>> {
    let (tag, tag_len) = read_varint(b)?;
    let number = u32::try_from(tag >> 3).ok().filter(|&n| n != 0)?;
    let wire = (tag & 7) as u8;
    let rest = &b[tag_len..];
    let mut field = WireField {
        number,
        wire,
        value: 0,
        bytes: &[],
        len: tag_len,
    };
    match wire {
        WIRE_VARINT => {
            let (v, n) = read_varint(rest)?;
            field.value = v;
            field.len += n;
        }
        WIRE_FIXED32 => {
            let raw: [u8; 4] = rest.get(..4)?.try_into().ok()?;
            field.value = u64::from(u32::from_le_bytes(raw));
            field.len += 4;
        }
        WIRE_FIXED64 => {
            let raw: [u8; 8] = rest.get(..8)?.try_into().ok()?;
            field.value = u64::from_le_bytes(raw);
            field.len += 8;
        }
        WIRE_BYTES => {
            let (len, n) = read_varint(rest)?;
            let len = usize::try_from(len).ok()?;
            let end = n.checked_add(len)?;
            field.bytes = rest.get(n..end)?;
            field.len += end;
        }
        _ => return None,
    }
    Some(field)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_field_past_its_input_is_refused() {
        // Field 1, bytes, length 2^64 - 1.
        let huge = [
            0x0a, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0x01,
        ];
        assert!(next_field(&huge).is_none());
        assert!(next_field(&[0x0a, 0x05, 0x01]).is_none());
        assert!(next_field(&[0x08]).is_none());
        assert!(next_field(&[0x0d, 0x01]).is_none());
        assert!(next_field(&[0x09, 0x01]).is_none());
        assert!(next_field(&[0x0b]).is_none(), "a group is malformed");
        assert!(next_field(&[0x00, 0x00]).is_none(), "field 0");
        assert!(next_field(&[]).is_none());
    }

    #[test]
    fn each_wire_type_reads() {
        let f = next_field(&[0x08, 0x96, 0x01]).unwrap();
        assert_eq!((f.number, f.value, f.len), (1, 150, 3));
        let f = next_field(&[0x12, 0x02, b'h', b'i', 0xff]).unwrap();
        assert_eq!((f.number, f.bytes, f.len), (2, &b"hi"[..], 4));
    }
}
