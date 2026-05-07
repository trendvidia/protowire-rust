// SPDX-License-Identifier: MIT
// Copyright (c) 2026 TrendVidia, LLC.
//! SBE marshal: serializes a [`DynamicMessage`] into the SBE binary format
//! using a pre-built [`MessageTemplate`]. Mirrors
//! `protowire/encoding/sbe/marshal.go` and the TS port's `sbe/marshal.ts`.
//!
//! Wire layout (little-endian throughout):
//!
//! - 8-byte message header: `block_length` (u16), `template_id` (u16),
//!   `schema_id` (u16), `version` (u16)
//! - `block_length` bytes of root scalar/composite data
//! - For each group: 4-byte group header (`block_length` u16,
//!   `num_in_group` u16) followed by `num_in_group * block_length` bytes
//!   of element data

use prost_reflect::{DynamicMessage, FieldDescriptor, Kind, ReflectMessage, Value};

use crate::codec::{Codec, GROUP_HEADER_SIZE, HEADER_SIZE};
use crate::errors::SbeError;
use crate::template::{FieldTemplate, GroupTemplate, MessageTemplate, SbeEncoding};

pub fn marshal(codec: &Codec, msg: &DynamicMessage) -> Result<Vec<u8>, SbeError> {
    let tmpl = codec.template(msg.descriptor().full_name())?;
    marshal_message(msg, tmpl)
}

fn marshal_message(msg: &DynamicMessage, tmpl: &MessageTemplate) -> Result<Vec<u8>, SbeError> {
    let mut total = HEADER_SIZE + tmpl.block_length;
    for gt in &tmpl.groups {
        let n = list_len(msg, &gt.fd);
        total += GROUP_HEADER_SIZE + n * gt.block_length;
    }

    let mut buf = vec![0u8; total];

    write_u16_le(&mut buf, 0, tmpl.block_length as u16);
    write_u16_le(&mut buf, 2, tmpl.template_id as u16);
    write_u16_le(&mut buf, 4, tmpl.schema_id as u16);
    write_u16_le(&mut buf, 6, tmpl.version as u16);

    for ft in &tmpl.fields {
        write_field(&mut buf, HEADER_SIZE, ft, msg)?;
    }

    let mut pos = HEADER_SIZE + tmpl.block_length;
    for gt in &tmpl.groups {
        pos += marshal_group(&mut buf, pos, msg, gt)?;
    }
    Ok(buf)
}

fn marshal_group(
    buf: &mut [u8],
    pos: usize,
    parent: &DynamicMessage,
    gt: &GroupTemplate,
) -> Result<usize, SbeError> {
    let entries: Vec<DynamicMessage> = match parent.get_field(&gt.fd).into_owned() {
        Value::List(items) => items
            .into_iter()
            .map(|v| match v {
                Value::Message(m) => m,
                _ => DynamicMessage::new(group_element_descriptor(&gt.fd)),
            })
            .collect(),
        _ => Vec::new(),
    };
    let n = entries.len();

    write_u16_le(buf, pos, gt.block_length as u16);
    write_u16_le(buf, pos + 2, n as u16);

    for (i, entry) in entries.iter().enumerate() {
        let start = pos + GROUP_HEADER_SIZE + i * gt.block_length;
        for ft in &gt.fields {
            write_field(buf, start, ft, entry)?;
        }
    }
    Ok(GROUP_HEADER_SIZE + n * gt.block_length)
}

fn write_field(
    buf: &mut [u8],
    base: usize,
    ft: &FieldTemplate,
    parent: &DynamicMessage,
) -> Result<(), SbeError> {
    if !ft.composite.is_empty() {
        let sub = match parent.get_field(&ft.fd).into_owned() {
            Value::Message(m) => m,
            _ => DynamicMessage::new(composite_descriptor(&ft.fd)),
        };
        for sf in &ft.composite {
            write_field(buf, base + ft.offset, sf, &sub)?;
        }
        return Ok(());
    }

    let off = base + ft.offset;
    let value = parent.get_field(&ft.fd).into_owned();
    let encoding = ft.encoding.expect("non-composite field has encoding");

    match encoding {
        SbeEncoding::Int8 => write_i64_as(buf, off, 1, value_as_i64(&value)),
        SbeEncoding::Int16 => write_i64_as(buf, off, 2, value_as_i64(&value)),
        SbeEncoding::Int32 => write_i64_as(buf, off, 4, value_as_i64(&value)),
        SbeEncoding::Int64 => write_i64_as(buf, off, 8, value_as_i64(&value)),
        SbeEncoding::Uint8 => write_u64_as(buf, off, 1, value_as_u64(&value)),
        SbeEncoding::Uint16 => write_u64_as(buf, off, 2, value_as_u64(&value)),
        SbeEncoding::Uint32 => write_u64_as(buf, off, 4, value_as_u64(&value)),
        SbeEncoding::Uint64 => write_u64_as(buf, off, 8, value_as_u64(&value)),
        SbeEncoding::Float => {
            let f = match &value {
                Value::F32(v) => *v,
                Value::F64(v) => *v as f32,
                _ => 0.0,
            };
            buf[off..off + 4].copy_from_slice(&f.to_le_bytes());
        }
        SbeEncoding::Double => {
            let f = match &value {
                Value::F64(v) => *v,
                Value::F32(v) => *v as f64,
                _ => 0.0,
            };
            buf[off..off + 8].copy_from_slice(&f.to_le_bytes());
        }
        SbeEncoding::Char => {
            let bytes = char_bytes(&ft.fd, &value);
            let n = ft.size.min(bytes.len());
            buf[off..off + n].copy_from_slice(&bytes[..n]);
            // Remaining bytes already zero from the initial allocation.
        }
    }
    Ok(())
}

fn write_u16_le(buf: &mut [u8], off: usize, v: u16) {
    buf[off..off + 2].copy_from_slice(&v.to_le_bytes());
}

fn write_i64_as(buf: &mut [u8], off: usize, width: usize, v: i64) {
    match width {
        1 => buf[off] = (v as i8) as u8,
        2 => buf[off..off + 2].copy_from_slice(&(v as i16).to_le_bytes()),
        4 => buf[off..off + 4].copy_from_slice(&(v as i32).to_le_bytes()),
        8 => buf[off..off + 8].copy_from_slice(&v.to_le_bytes()),
        _ => unreachable!("invalid signed width {}", width),
    }
}

fn write_u64_as(buf: &mut [u8], off: usize, width: usize, v: u64) {
    match width {
        1 => buf[off] = v as u8,
        2 => buf[off..off + 2].copy_from_slice(&(v as u16).to_le_bytes()),
        4 => buf[off..off + 4].copy_from_slice(&(v as u32).to_le_bytes()),
        8 => buf[off..off + 8].copy_from_slice(&v.to_le_bytes()),
        _ => unreachable!("invalid unsigned width {}", width),
    }
}

/// Coerce a [`Value`] to an `i64` for signed-int encoding writes. Bool maps
/// to 0/1, enum to its number, floats truncate. Default 0 covers unset
/// fields (whose `get_field` returns the kind-default).
fn value_as_i64(v: &Value) -> i64 {
    match v {
        Value::I32(n) => *n as i64,
        Value::I64(n) => *n,
        Value::U32(n) => *n as i64,
        Value::U64(n) => *n as i64,
        Value::Bool(b) => *b as i64,
        Value::EnumNumber(n) => *n as i64,
        Value::F32(f) => *f as i64,
        Value::F64(f) => *f as i64,
        _ => 0,
    }
}

fn value_as_u64(v: &Value) -> u64 {
    match v {
        Value::U32(n) => *n as u64,
        Value::U64(n) => *n,
        Value::I32(n) => *n as u64,
        Value::I64(n) => *n as u64,
        Value::Bool(b) => *b as u64,
        Value::EnumNumber(n) => *n as u64,
        Value::F32(f) => *f as u64,
        Value::F64(f) => *f as u64,
        _ => 0,
    }
}

fn char_bytes(fd: &FieldDescriptor, value: &Value) -> Vec<u8> {
    match (fd.kind(), value) {
        (Kind::Bytes, Value::Bytes(b)) => b.to_vec(),
        (_, Value::String(s)) => s.as_bytes().to_vec(),
        _ => Vec::new(),
    }
}

fn list_len(msg: &DynamicMessage, fd: &FieldDescriptor) -> usize {
    match msg.get_field(fd).as_ref() {
        Value::List(l) => l.len(),
        _ => 0,
    }
}

fn composite_descriptor(fd: &FieldDescriptor) -> prost_reflect::MessageDescriptor {
    match fd.kind() {
        Kind::Message(m) => m,
        _ => panic!("composite descriptor on non-message field {}", fd.name()),
    }
}

fn group_element_descriptor(fd: &FieldDescriptor) -> prost_reflect::MessageDescriptor {
    match fd.kind() {
        Kind::Message(m) => m,
        _ => panic!("group element descriptor on non-message field {}", fd.name()),
    }
}
