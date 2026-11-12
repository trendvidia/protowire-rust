// SPDX-License-Identifier: MIT
// Copyright (c) 2026 TrendVidia, LLC.
//! SBE wire-layout templates derived from proto descriptors. Mirrors
//! `protowire/encoding/sbe/template.go` and the TS port's `sbe/template.ts`.
//!
//! A [`MessageTemplate`] captures everything needed to lay out a message on
//! the SBE wire: ordered scalar/composite fields, their offsets and sizes,
//! plus any repeating-group templates.

use prost_reflect::{FieldDescriptor, Kind, MessageDescriptor};

use crate::annotations::{
    field_string, field_uint32, message_uint32, EXT_ENCODING, EXT_LENGTH, EXT_TEMPLATE_ID,
};
use crate::errors::SbeError;

/// SBE primitive encoding name. Maps directly to a fixed byte width.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SbeEncoding {
    Int8,
    Int16,
    Int32,
    Int64,
    Uint8,
    Uint16,
    Uint32,
    Uint64,
    Float,
    Double,
    /// Char (fixed-length string/bytes). Width is field-specific.
    Char,
}

impl SbeEncoding {
    pub fn name(self) -> &'static str {
        match self {
            SbeEncoding::Int8 => "int8",
            SbeEncoding::Int16 => "int16",
            SbeEncoding::Int32 => "int32",
            SbeEncoding::Int64 => "int64",
            SbeEncoding::Uint8 => "uint8",
            SbeEncoding::Uint16 => "uint16",
            SbeEncoding::Uint32 => "uint32",
            SbeEncoding::Uint64 => "uint64",
            SbeEncoding::Float => "float",
            SbeEncoding::Double => "double",
            SbeEncoding::Char => "char",
        }
    }

    /// Whether this is one of the eight integer primitives.
    pub fn is_integer(self) -> bool {
        !matches!(
            self,
            SbeEncoding::Float | SbeEncoding::Double | SbeEncoding::Char
        )
    }

    /// Whether this is a signed integer primitive.
    pub fn is_signed(self) -> bool {
        matches!(
            self,
            SbeEncoding::Int8 | SbeEncoding::Int16 | SbeEncoding::Int32 | SbeEncoding::Int64
        )
    }

    /// The range of an integer primitive of `size` bytes, as `(lo, hi)`.
    pub(crate) fn int_range(self, size: usize) -> (i64, u64) {
        let bits = 8 * size as u32;
        if self.is_signed() {
            (i64::MIN >> (64 - bits), (i64::MAX >> (64 - bits)) as u64)
        } else {
            (0, u64::MAX >> (64 - bits))
        }
    }

    fn from_str(s: &str) -> Option<(SbeEncoding, usize)> {
        Some(match s {
            "int8" => (SbeEncoding::Int8, 1),
            "uint8" => (SbeEncoding::Uint8, 1),
            "int16" => (SbeEncoding::Int16, 2),
            "uint16" => (SbeEncoding::Uint16, 2),
            "int32" => (SbeEncoding::Int32, 4),
            "uint32" => (SbeEncoding::Uint32, 4),
            "float" => (SbeEncoding::Float, 4),
            "int64" => (SbeEncoding::Int64, 8),
            "uint64" => (SbeEncoding::Uint64, 8),
            "double" => (SbeEncoding::Double, 8),
            _ => return None,
        })
    }
}

#[derive(Debug, Clone)]
pub struct FieldTemplate {
    pub fd: FieldDescriptor,
    pub offset: usize,
    pub size: usize,
    /// `None` for composite (nested message) fields.
    pub encoding: Option<SbeEncoding>,
    /// Non-empty for composite fields.
    pub composite: Vec<FieldTemplate>,
}

#[derive(Debug, Clone)]
pub struct GroupTemplate {
    pub fd: FieldDescriptor,
    pub block_length: usize,
    pub fields: Vec<FieldTemplate>,
}

#[derive(Debug, Clone)]
pub struct MessageTemplate {
    pub desc: MessageDescriptor,
    pub template_id: u32,
    pub schema_id: u32,
    pub version: u32,
    pub block_length: usize,
    pub fields: Vec<FieldTemplate>,
    pub groups: Vec<GroupTemplate>,
}

/// The largest value of every SBE header field: blockLength, templateId,
/// schemaId and version in the message header, blockLength and numInGroup
/// in a group header. A schema or a message needing a larger value cannot be
/// written: narrowing it would write a header that disagrees with the bytes
/// after it.
pub(crate) const MAX_UINT16: usize = u16::MAX as usize;

/// The offset after a field of `size` bytes placed at `offset`, refusing a
/// field that ends past what a `u16` blockLength can state. Every offset in
/// the block is then a `u16` as well.
fn end_of_field(offset: usize, size: usize) -> Result<usize, String> {
    let end = offset + size;
    if end > MAX_UINT16 {
        return Err(format!(
            "ends at byte {end} of its block, past {MAX_UINT16}, the largest blockLength SBE can state"
        ));
    }
    Ok(end)
}

/// Build a [`MessageTemplate`] for a top-level SBE message. Errors if the
/// message lacks `(sbe.template_id)`, contains unsupported features (oneofs,
/// maps, nested repeats, repeated scalars), or needs a header or block value
/// past 65535, which the SBE wire cannot state.
pub fn build_template(
    desc: &MessageDescriptor,
    schema_id: u32,
    version: u32,
) -> Result<MessageTemplate, SbeError> {
    let template_id = message_uint32(desc, EXT_TEMPLATE_ID).ok_or_else(|| {
        SbeError::new(format!(
            "sbe: message {} missing (sbe.template_id)",
            desc.full_name()
        ))
    })?;
    if template_id as usize > MAX_UINT16 {
        return Err(SbeError::new(format!(
            "sbe: message {}: (sbe.template_id) = {} is past {}, the largest templateId SBE can state",
            desc.full_name(),
            template_id,
            MAX_UINT16
        )));
    }

    let mut tmpl = MessageTemplate {
        desc: desc.clone(),
        template_id,
        schema_id,
        version,
        block_length: 0,
        fields: Vec::new(),
        groups: Vec::new(),
    };

    let mut offset: usize = 0;
    for fd in sorted_fields(desc) {
        if fd.is_map() {
            return Err(SbeError::new(format!(
                "sbe: map field {}.{} not supported",
                desc.full_name(),
                fd.name()
            )));
        }
        // A proto3 `optional` field sits in a synthetic oneof of its own,
        // which is not a oneof to SBE.
        if fd.containing_oneof().is_some_and(|o| !o.is_synthetic()) {
            return Err(SbeError::new(format!(
                "sbe: oneof field {}.{} not supported",
                desc.full_name(),
                fd.name()
            )));
        }
        if fd.is_list() {
            if matches!(fd.kind(), Kind::Message(_)) {
                tmpl.groups.push(build_group_template(&fd)?);
                continue;
            }
            return Err(SbeError::new(format!(
                "sbe: repeated scalar field {}.{} not supported; wrap in a message",
                desc.full_name(),
                fd.name()
            )));
        }
        let wrap = |e: String| {
            SbeError::new(format!(
                "sbe: composite {}.{}: {}",
                desc.full_name(),
                fd.name(),
                e
            ))
        };
        if let Kind::Message(inner) = fd.kind() {
            let (size, sub) = build_composite_fields(&inner).map_err(wrap)?;
            let end = end_of_field(offset, size).map_err(wrap)?;
            tmpl.fields.push(FieldTemplate {
                fd: fd.clone(),
                offset,
                size,
                encoding: None,
                composite: sub,
            });
            offset = end;
            continue;
        }

        let wrap = |e: String| {
            SbeError::new(format!(
                "sbe: field {}.{}: {}",
                desc.full_name(),
                fd.name(),
                e
            ))
        };
        let (enc, size) = encoding_size(&fd).map_err(wrap)?;
        let end = end_of_field(offset, size).map_err(wrap)?;
        tmpl.fields.push(FieldTemplate {
            fd: fd.clone(),
            offset,
            size,
            encoding: Some(enc),
            composite: Vec::new(),
        });
        offset = end;
    }

    tmpl.block_length = offset;
    Ok(tmpl)
}

fn build_group_template(fd: &FieldDescriptor) -> Result<GroupTemplate, SbeError> {
    let md = match fd.kind() {
        Kind::Message(m) => m,
        _ => {
            return Err(SbeError::new(format!(
                "sbe: group field {} must be a repeated message",
                fd.name()
            )))
        }
    };
    let mut gt = GroupTemplate {
        fd: fd.clone(),
        block_length: 0,
        fields: Vec::new(),
    };
    let mut offset: usize = 0;
    for f in sorted_fields(&md) {
        if f.is_map() {
            return Err(SbeError::new(format!(
                "sbe: map field in group {} not supported",
                md.full_name()
            )));
        }
        if f.is_list() {
            return Err(SbeError::new(format!(
                "sbe: nested repeated field in group {} not supported",
                md.full_name()
            )));
        }
        if let Kind::Message(inner) = f.kind() {
            let wrap = |e: String| {
                SbeError::new(format!(
                    "sbe: composite in group {}.{}: {}",
                    md.full_name(),
                    f.name(),
                    e
                ))
            };
            let (size, sub) = build_composite_fields(&inner).map_err(wrap)?;
            let end = end_of_field(offset, size).map_err(wrap)?;
            gt.fields.push(FieldTemplate {
                fd: f.clone(),
                offset,
                size,
                encoding: None,
                composite: sub,
            });
            offset = end;
            continue;
        }
        let wrap = |e: String| {
            SbeError::new(format!(
                "sbe: group field {}.{}: {}",
                md.full_name(),
                f.name(),
                e
            ))
        };
        let (enc, size) = encoding_size(&f).map_err(wrap)?;
        let end = end_of_field(offset, size).map_err(wrap)?;
        gt.fields.push(FieldTemplate {
            fd: f.clone(),
            offset,
            size,
            encoding: Some(enc),
            composite: Vec::new(),
        });
        offset = end;
    }
    gt.block_length = offset;
    Ok(gt)
}

/// A composite's fields and size. The error is unprefixed: the caller names
/// the composite, as protowire-go's `buildCompositeFields` leaves it to.
fn build_composite_fields(md: &MessageDescriptor) -> Result<(usize, Vec<FieldTemplate>), String> {
    let mut out = Vec::new();
    let mut offset: usize = 0;
    for fd in sorted_fields(md) {
        if fd.is_list() || fd.is_map() {
            return Err(format!(
                "composite {} contains list/map field {}",
                md.full_name(),
                fd.name()
            ));
        }
        if fd.containing_oneof().is_some_and(|o| !o.is_synthetic()) {
            return Err(format!(
                "composite {} contains oneof field {}",
                md.full_name(),
                fd.name()
            ));
        }
        let wrap = |e: String| format!("composite field {}.{}: {}", md.full_name(), fd.name(), e);
        if let Kind::Message(inner) = fd.kind() {
            let (size, sub) = build_composite_fields(&inner)?;
            let end = end_of_field(offset, size).map_err(wrap)?;
            out.push(FieldTemplate {
                fd: fd.clone(),
                offset,
                size,
                encoding: None,
                composite: sub,
            });
            offset = end;
            continue;
        }
        let (enc, size) = encoding_size(&fd).map_err(wrap)?;
        let end = end_of_field(offset, size).map_err(wrap)?;
        out.push(FieldTemplate {
            fd: fd.clone(),
            offset,
            size,
            encoding: Some(enc),
            composite: Vec::new(),
        });
        offset = end;
    }
    Ok((offset, out))
}

/// The SBE primitive and byte size of a scalar, string or bytes field. The
/// error states the reason alone; the template builders name the field, as
/// in `sbe: field pkg.M.v: (sbe.length) = 70000 is past 65535, ...`.
pub fn field_encoding_size(fd: &FieldDescriptor) -> Result<(SbeEncoding, usize), SbeError> {
    encoding_size(fd).map_err(SbeError::new)
}

fn encoding_size(fd: &FieldDescriptor) -> Result<(SbeEncoding, usize), String> {
    // An explicit (sbe.encoding) narrows a numeric field. The draft says
    // nothing of crossing between integers and floating point, and a string
    // or bytes field takes (sbe.length) instead.
    if let Some(enc) = field_string(fd, EXT_ENCODING) {
        let kind = fd.kind();
        let parsed = SbeEncoding::from_str(&enc);
        match kind {
            Kind::String | Kind::Bytes => {
                let k = kind_name(&kind);
                return Err(format!(
                    "(sbe.encoding) = {} on a {k} field; it narrows a numeric field, and a {k} field takes (sbe.length)",
                    go_quote(&enc)
                ));
            }
            Kind::Float | Kind::Double => {
                if parsed.is_some_and(|(e, _)| e.is_integer()) {
                    return Err(format!(
                        "(sbe.encoding) = {} is an integer primitive and the field is {}",
                        go_quote(&enc),
                        kind_name(&kind)
                    ));
                }
            }
            _ => {
                if matches!(parsed, Some((SbeEncoding::Float | SbeEncoding::Double, _))) {
                    return Err(format!(
                        "(sbe.encoding) = {} is a floating-point primitive and the field is {}",
                        go_quote(&enc),
                        kind_name(&kind)
                    ));
                }
            }
        }
        return parsed.ok_or_else(|| format!("unknown encoding {}", go_quote(&enc)));
    }

    Ok(match fd.kind() {
        Kind::Bool => (SbeEncoding::Uint8, 1),
        Kind::Int32 | Kind::Sint32 | Kind::Sfixed32 => (SbeEncoding::Int32, 4),
        Kind::Int64 | Kind::Sint64 | Kind::Sfixed64 => (SbeEncoding::Int64, 8),
        Kind::Uint32 | Kind::Fixed32 => (SbeEncoding::Uint32, 4),
        Kind::Uint64 | Kind::Fixed64 => (SbeEncoding::Uint64, 8),
        Kind::Float => (SbeEncoding::Float, 4),
        Kind::Double => (SbeEncoding::Double, 8),
        Kind::Enum(_) => (SbeEncoding::Uint8, 1),
        kind @ (Kind::String | Kind::Bytes) => {
            let len = field_uint32(fd, EXT_LENGTH).ok_or_else(|| {
                format!(
                    "{} field requires (sbe.length) annotation",
                    kind_name(&kind)
                )
            })?;
            if len as usize > MAX_UINT16 {
                return Err(format!(
                    "(sbe.length) = {len} is past {MAX_UINT16}, the largest blockLength SBE can state"
                ));
            }
            (SbeEncoding::Char, len as usize)
        }
        other => return Err(format!("unsupported proto kind {}", kind_name(&other))),
    })
}

/// A field kind as protowire-go's errors name it (`protoreflect.Kind`'s
/// String).
pub(crate) fn kind_name(kind: &Kind) -> &'static str {
    match kind {
        Kind::Double => "double",
        Kind::Float => "float",
        Kind::Int32 => "int32",
        Kind::Int64 => "int64",
        Kind::Uint32 => "uint32",
        Kind::Uint64 => "uint64",
        Kind::Sint32 => "sint32",
        Kind::Sint64 => "sint64",
        Kind::Fixed32 => "fixed32",
        Kind::Fixed64 => "fixed64",
        Kind::Sfixed32 => "sfixed32",
        Kind::Sfixed64 => "sfixed64",
        Kind::Bool => "bool",
        Kind::String => "string",
        Kind::Bytes => "bytes",
        Kind::Message(_) => "message",
        Kind::Enum(_) => "enum",
    }
}

/// Go's `%q` of an encoding name. Printable ASCII is written as is, with
/// `"` and `\` escaped, and other ASCII as `\a`-style or `\x` escapes. A
/// non-ASCII character is written as is, where Go escapes one that is not
/// printable; an encoding name holding one is refused either way, and only
/// the error's spelling of it could differ.
fn go_quote(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 2);
    out.push('"');
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\x07' => out.push_str("\\a"),
            '\x08' => out.push_str("\\b"),
            '\x0c' => out.push_str("\\f"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            '\x0b' => out.push_str("\\v"),
            c if (c as u32) < 0x20 || c as u32 == 0x7f => {
                out.push_str(&format!("\\x{:02x}", c as u32))
            }
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

/// The range of values an integer, bool or enum field holds. An enum is
/// open in proto3, so any `int32`.
pub(crate) fn kind_range(kind: &Kind) -> (i64, u64) {
    match kind {
        Kind::Int32 | Kind::Sint32 | Kind::Sfixed32 | Kind::Enum(_) => {
            (i32::MIN as i64, i32::MAX as u64)
        }
        Kind::Int64 | Kind::Sint64 | Kind::Sfixed64 => (i64::MIN, i64::MAX as u64),
        Kind::Uint32 | Kind::Fixed32 => (0, u32::MAX as u64),
        Kind::Bool => (0, 1),
        _ => (0, u64::MAX),
    }
}

/// Whether the integer `v` is within `[lo, hi]`. `v` holds a `u64` when
/// `unsigned`, and an `i64`'s bits otherwise.
pub(crate) fn in_range(v: u64, unsigned: bool, lo: i64, hi: u64) -> bool {
    if !unsigned && (v as i64) < 0 {
        return v as i64 >= lo;
    }
    v <= hi
}

/// An integer held as in [`in_range`], in decimal.
pub(crate) fn int_string(v: u64, unsigned: bool) -> String {
    if unsigned {
        v.to_string()
    } else {
        (v as i64).to_string()
    }
}

/// Whether a double is within float's range. NaN and the infinities are, as
/// float has them; a finite value past ±`f32::MAX` is not, even one that
/// would round down to it.
pub(crate) fn float_fits(f: f64) -> bool {
    f.is_infinite() || !(f > f32::MAX as f64 || f < -(f32::MAX as f64))
}

fn sorted_fields(desc: &MessageDescriptor) -> Vec<FieldDescriptor> {
    let mut v: Vec<FieldDescriptor> = desc.fields().collect();
    v.sort_by_key(|f| f.number());
    v
}
