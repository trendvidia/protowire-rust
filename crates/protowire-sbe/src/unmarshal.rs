// SPDX-License-Identifier: MIT
// Copyright (c) 2026 TrendVidia, LLC.
//! SBE unmarshal: decodes an SBE binary buffer into a [`DynamicMessage`]
//! using a pre-built [`MessageTemplate`]. Mirrors
//! `protowire/encoding/sbe/unmarshal.go` and the TS port's `sbe/unmarshal.ts`.

use prost_reflect::{
    DynamicMessage, FieldDescriptor, Kind, MessageDescriptor, ReflectMessage, Value,
};

use crate::codec::{Codec, GROUP_HEADER_SIZE, HEADER_SIZE};
use crate::errors::SbeError;
use crate::template::{FieldTemplate, GroupTemplate, MessageTemplate, SbeEncoding};

pub fn unmarshal(codec: &Codec, msg: &mut DynamicMessage, data: &[u8]) -> Result<(), SbeError> {
    let tmpl = codec.template(msg.descriptor().full_name())?;
    unmarshal_message(msg, tmpl, data)
}

fn unmarshal_message(
    msg: &mut DynamicMessage,
    tmpl: &MessageTemplate,
    data: &[u8],
) -> Result<(), SbeError> {
    if data.len() < HEADER_SIZE {
        return Err(SbeError::new(format!(
            "sbe: data too short for header: {} bytes",
            data.len()
        )));
    }
    let block_length = read_u16_le(data, 0) as usize;
    let template_id = read_u16_le(data, 2) as u32;
    if template_id != tmpl.template_id {
        return Err(SbeError::new(format!(
            "sbe: template ID mismatch: got {}, want {}",
            template_id, tmpl.template_id
        )));
    }

    let end = HEADER_SIZE + block_length;
    if data.len() < end {
        return Err(SbeError::new(format!(
            "sbe: data too short for root block: need {}, have {}",
            end,
            data.len()
        )));
    }

    for ft in &tmpl.fields {
        read_field(data, HEADER_SIZE, ft, msg)?;
    }

    let mut pos = end;
    for gt in &tmpl.groups {
        pos += unmarshal_group(data, pos, msg, gt)?;
    }
    Ok(())
}

fn unmarshal_group(
    data: &[u8],
    pos: usize,
    parent: &mut DynamicMessage,
    gt: &GroupTemplate,
) -> Result<usize, SbeError> {
    if data.len() < pos + GROUP_HEADER_SIZE {
        return Err(SbeError::new("sbe: data too short for group header"));
    }
    let block_length = read_u16_le(data, pos) as usize;
    let num_in_group = read_u16_le(data, pos + 2) as usize;
    let total = GROUP_HEADER_SIZE + num_in_group * block_length;
    if data.len() < pos + total {
        return Err(SbeError::new(format!(
            "sbe: data too short for group entries: need {}, have {}",
            pos + total,
            data.len()
        )));
    }

    let elem_desc = group_element_descriptor(&gt.fd);
    let mut entries: Vec<Value> = Vec::with_capacity(num_in_group);
    for i in 0..num_in_group {
        let mut entry = DynamicMessage::new(elem_desc.clone());
        let start = pos + GROUP_HEADER_SIZE + i * block_length;
        for ft in &gt.fields {
            read_field(data, start, ft, &mut entry)?;
        }
        entries.push(Value::Message(entry));
    }
    parent.set_field(&gt.fd, Value::List(entries));
    Ok(total)
}

fn read_field(
    data: &[u8],
    base: usize,
    ft: &FieldTemplate,
    parent: &mut DynamicMessage,
) -> Result<(), SbeError> {
    if !ft.composite.is_empty() {
        let inner_desc = composite_descriptor(&ft.fd);
        let mut sub = DynamicMessage::new(inner_desc);
        for sf in &ft.composite {
            read_field(data, base + ft.offset, sf, &mut sub)?;
        }
        parent.set_field(&ft.fd, Value::Message(sub));
        return Ok(());
    }

    let off = base + ft.offset;
    let fd = &ft.fd;
    let encoding = ft.encoding.expect("non-composite field has encoding");

    let value = match encoding {
        SbeEncoding::Int8 => int_to_value(fd, (data[off] as i8) as i64),
        SbeEncoding::Int16 => int_to_value(fd, read_i16_le(data, off) as i64),
        SbeEncoding::Int32 => int_to_value(fd, read_i32_le(data, off) as i64),
        SbeEncoding::Int64 => int_to_value(fd, read_i64_le(data, off)),
        SbeEncoding::Uint8 => uint_to_value(fd, data[off] as u64),
        SbeEncoding::Uint16 => uint_to_value(fd, read_u16_le(data, off) as u64),
        SbeEncoding::Uint32 => uint_to_value(fd, read_u32_le(data, off) as u64),
        SbeEncoding::Uint64 => uint_to_value(fd, read_u64_le(data, off)),
        SbeEncoding::Float => Value::F32(read_f32_le(data, off)),
        SbeEncoding::Double => Value::F64(read_f64_le(data, off)),
        SbeEncoding::Char => {
            let slice = &data[off..off + ft.size];
            if matches!(fd.kind(), Kind::Bytes) {
                Value::Bytes(slice.to_vec().into())
            } else {
                let mut n = slice.len();
                while n > 0 && slice[n - 1] == 0 {
                    n -= 1;
                }
                Value::String(String::from_utf8(slice[..n].to_vec()).map_err(|e| {
                    SbeError::new(format!("sbe: invalid utf-8 in {}: {}", fd.name(), e))
                })?)
            }
        }
    };
    parent.set_field(fd, value);
    Ok(())
}

/// Coerce a signed-int wire value into the proto field's expected [`Value`]
/// variant. Mirrors `setIntField`/`setInt64Field` in the TS port.
fn int_to_value(fd: &FieldDescriptor, v: i64) -> Value {
    match fd.kind() {
        Kind::Int32 | Kind::Sint32 | Kind::Sfixed32 => Value::I32(v as i32),
        Kind::Int64 | Kind::Sint64 | Kind::Sfixed64 => Value::I64(v),
        Kind::Uint32 | Kind::Fixed32 => Value::U32(v as u32),
        Kind::Uint64 | Kind::Fixed64 => Value::U64(v as u64),
        Kind::Bool => Value::Bool(v != 0),
        Kind::Enum(_) => Value::EnumNumber(v as i32),
        Kind::Float => Value::F32(v as f32),
        Kind::Double => Value::F64(v as f64),
        _ => Value::I64(v),
    }
}

/// Coerce an unsigned-int wire value into the proto field's expected
/// [`Value`] variant. Mirrors `setUintField`/`setUint64Field`.
fn uint_to_value(fd: &FieldDescriptor, v: u64) -> Value {
    match fd.kind() {
        Kind::Bool => Value::Bool(v != 0),
        Kind::Enum(_) => Value::EnumNumber(v as i32),
        Kind::Uint32 | Kind::Fixed32 => Value::U32(v as u32),
        Kind::Uint64 | Kind::Fixed64 => Value::U64(v),
        Kind::Int32 | Kind::Sint32 | Kind::Sfixed32 => Value::I32(v as i32),
        Kind::Int64 | Kind::Sint64 | Kind::Sfixed64 => Value::I64(v as i64),
        _ => Value::U64(v),
    }
}

fn read_u16_le(buf: &[u8], off: usize) -> u16 {
    u16::from_le_bytes([buf[off], buf[off + 1]])
}

fn read_i16_le(buf: &[u8], off: usize) -> i16 {
    i16::from_le_bytes([buf[off], buf[off + 1]])
}

fn read_u32_le(buf: &[u8], off: usize) -> u32 {
    u32::from_le_bytes([buf[off], buf[off + 1], buf[off + 2], buf[off + 3]])
}

fn read_i32_le(buf: &[u8], off: usize) -> i32 {
    i32::from_le_bytes([buf[off], buf[off + 1], buf[off + 2], buf[off + 3]])
}

fn read_u64_le(buf: &[u8], off: usize) -> u64 {
    let mut bytes = [0u8; 8];
    bytes.copy_from_slice(&buf[off..off + 8]);
    u64::from_le_bytes(bytes)
}

fn read_i64_le(buf: &[u8], off: usize) -> i64 {
    let mut bytes = [0u8; 8];
    bytes.copy_from_slice(&buf[off..off + 8]);
    i64::from_le_bytes(bytes)
}

fn read_f32_le(buf: &[u8], off: usize) -> f32 {
    f32::from_le_bytes([buf[off], buf[off + 1], buf[off + 2], buf[off + 3]])
}

fn read_f64_le(buf: &[u8], off: usize) -> f64 {
    let mut bytes = [0u8; 8];
    bytes.copy_from_slice(&buf[off..off + 8]);
    f64::from_le_bytes(bytes)
}

fn composite_descriptor(fd: &FieldDescriptor) -> MessageDescriptor {
    match fd.kind() {
        Kind::Message(m) => m,
        _ => panic!("composite descriptor on non-message field {}", fd.name()),
    }
}

fn group_element_descriptor(fd: &FieldDescriptor) -> MessageDescriptor {
    match fd.kind() {
        Kind::Message(m) => m,
        _ => panic!(
            "group element descriptor on non-message field {}",
            fd.name()
        ),
    }
}
