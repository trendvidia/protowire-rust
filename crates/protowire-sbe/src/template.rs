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

/// Build a [`MessageTemplate`] for a top-level SBE message. Errors if the
/// message lacks `(sbe.template_id)` or contains unsupported features
/// (oneofs, maps, nested repeats, repeated scalars).
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
        if fd.containing_oneof().is_some() {
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
        if let Kind::Message(inner) = fd.kind() {
            let (size, sub) = build_composite_fields(&inner)?;
            tmpl.fields.push(FieldTemplate {
                fd: fd.clone(),
                offset,
                size,
                encoding: None,
                composite: sub,
            });
            offset += size;
            continue;
        }

        let (enc, size) = field_encoding_size(&fd)?;
        tmpl.fields.push(FieldTemplate {
            fd: fd.clone(),
            offset,
            size,
            encoding: Some(enc),
            composite: Vec::new(),
        });
        offset += size;
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
            let (size, sub) = build_composite_fields(&inner)?;
            gt.fields.push(FieldTemplate {
                fd: f.clone(),
                offset,
                size,
                encoding: None,
                composite: sub,
            });
            offset += size;
            continue;
        }
        let (enc, size) = field_encoding_size(&f)?;
        gt.fields.push(FieldTemplate {
            fd: f.clone(),
            offset,
            size,
            encoding: Some(enc),
            composite: Vec::new(),
        });
        offset += size;
    }
    gt.block_length = offset;
    Ok(gt)
}

fn build_composite_fields(md: &MessageDescriptor) -> Result<(usize, Vec<FieldTemplate>), SbeError> {
    let mut out = Vec::new();
    let mut offset: usize = 0;
    for fd in sorted_fields(md) {
        if fd.is_list() || fd.is_map() {
            return Err(SbeError::new(format!(
                "sbe: composite {} contains list/map field {}",
                md.full_name(),
                fd.name()
            )));
        }
        if fd.containing_oneof().is_some() {
            return Err(SbeError::new(format!(
                "sbe: composite {} contains oneof field {}",
                md.full_name(),
                fd.name()
            )));
        }
        if let Kind::Message(inner) = fd.kind() {
            let (size, sub) = build_composite_fields(&inner)?;
            out.push(FieldTemplate {
                fd: fd.clone(),
                offset,
                size,
                encoding: None,
                composite: sub,
            });
            offset += size;
            continue;
        }
        let (enc, size) = field_encoding_size(&fd)?;
        out.push(FieldTemplate {
            fd: fd.clone(),
            offset,
            size,
            encoding: Some(enc),
            composite: Vec::new(),
        });
        offset += size;
    }
    Ok((offset, out))
}

pub fn field_encoding_size(fd: &FieldDescriptor) -> Result<(SbeEncoding, usize), SbeError> {
    if let Some(explicit) = field_string(fd, EXT_ENCODING) {
        return SbeEncoding::from_str(&explicit).ok_or_else(|| {
            SbeError::new(format!(
                "sbe: unknown encoding {:?} on {}",
                explicit,
                fd.name()
            ))
        });
    }

    if matches!(fd.kind(), Kind::Enum(_)) {
        return Ok((SbeEncoding::Uint8, 1));
    }

    Ok(match fd.kind() {
        Kind::Bool => (SbeEncoding::Uint8, 1),
        Kind::Int32 | Kind::Sint32 | Kind::Sfixed32 => (SbeEncoding::Int32, 4),
        Kind::Int64 | Kind::Sint64 | Kind::Sfixed64 => (SbeEncoding::Int64, 8),
        Kind::Uint32 | Kind::Fixed32 => (SbeEncoding::Uint32, 4),
        Kind::Uint64 | Kind::Fixed64 => (SbeEncoding::Uint64, 8),
        Kind::Float => (SbeEncoding::Float, 4),
        Kind::Double => (SbeEncoding::Double, 8),
        Kind::String | Kind::Bytes => {
            let len = field_uint32(fd, EXT_LENGTH).ok_or_else(|| {
                let kind_name = if matches!(fd.kind(), Kind::String) {
                    "string"
                } else {
                    "bytes"
                };
                SbeError::new(format!(
                    "sbe: {} field {} requires (sbe.length) annotation",
                    kind_name,
                    fd.name()
                ))
            })?;
            (SbeEncoding::Char, len as usize)
        }
        other => {
            return Err(SbeError::new(format!(
                "sbe: unsupported field kind {:?} on {}",
                other,
                fd.name()
            )));
        }
    })
}

fn sorted_fields(desc: &MessageDescriptor) -> Vec<FieldDescriptor> {
    let mut v: Vec<FieldDescriptor> = desc.fields().collect();
    v.sort_by_key(|f| f.number());
    v
}
