// SPDX-License-Identifier: MIT
// Copyright (c) 2026 TrendVidia, LLC.
//! SBE unmarshal: decodes an SBE binary buffer into a [`DynamicMessage`]
//! using a pre-built [`MessageTemplate`]. Mirrors
//! `protowire/encoding/sbe/unmarshal.go` and the TS port's `sbe/unmarshal.ts`.

use prost_reflect::{
    DynamicMessage, FieldDescriptor, Kind, MessageDescriptor, ReflectMessage, Value,
};

use crate::codec::{check_group_header, Codec, GROUP_HEADER_SIZE, HEADER_SIZE};
use crate::errors::SbeError;
use crate::gofmt::go_g;
use crate::template::{
    float_fits, in_range, int_string, kind_name, kind_range, FieldTemplate, GroupTemplate,
    MessageTemplate, SbeEncoding,
};

/// Decode `data` into `msg` under the codec's limits: the input is refused
/// past `MaxMessageSize` before the header is read, the header's template ID
/// and schema ID must be `msg`'s (a template ID is unique only within a
/// schema, so bytes naming another schema are another message's; the
/// version is not checked), a wire block shorter
/// than the template's is rejected (HARDENING.md § SBE step 2), and each
/// group's header is validated before any entry is allocated (steps 3 and
/// 4, and `MaxRepeatedCount`).
pub fn unmarshal(codec: &Codec, msg: &mut DynamicMessage, data: &[u8]) -> Result<(), SbeError> {
    codec.check_message_size(data.len())?;
    let tmpl = codec.template(msg.descriptor().full_name())?;
    unmarshal_message(msg, tmpl, data, codec.limits().max_repeated_count)
}

fn unmarshal_message(
    msg: &mut DynamicMessage,
    tmpl: &MessageTemplate,
    data: &[u8],
    max_repeated_count: usize,
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
    // A template id is unique only within a schema, so the template id
    // alone does not say the bytes are this message's: they may be another
    // schema's message with the same id.
    let schema_id = read_u16_le(data, 4) as u32;
    if schema_id != tmpl.schema_id {
        return Err(SbeError::new(format!(
            "sbe: schema ID mismatch: got {}, want {} for template {}",
            schema_id, tmpl.schema_id, tmpl.template_id
        )));
    }

    // HARDENING.md § SBE step 2: a wire block strictly smaller than the
    // template's means some field's offset + size exceeds it; larger is
    // valid (a newer schema with trailing fields).
    if block_length < tmpl.block_length {
        return Err(SbeError::new(format!(
            "sbe: wire block_length {} is below template block_length {}",
            block_length, tmpl.block_length
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
        pos += unmarshal_group(data, pos, msg, gt, max_repeated_count)?;
    }
    Ok(())
}

fn unmarshal_group(
    data: &[u8],
    pos: usize,
    parent: &mut DynamicMessage,
    gt: &GroupTemplate,
    max_repeated_count: usize,
) -> Result<usize, SbeError> {
    let (block_length, num_in_group, total) =
        check_group_header(data, pos, gt.fd.name(), gt.block_length, max_repeated_count)?;

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
        SbeEncoding::Float => float_value(fd, read_f32_le(data, off) as f64),
        SbeEncoding::Double => {
            let f = read_f64_le(data, off);
            if matches!(fd.kind(), Kind::Float) && !float_fits(f) {
                return Err(SbeError::new(format!(
                    "sbe: field {}: wire value {} does not fit a float field, ±{}",
                    fd.full_name(),
                    go_g(f),
                    go_g(f32::MAX as f64)
                )));
            }
            float_value(fd, f)
        }
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
        int => {
            // An integer primitive, sign- or zero-extended per the primitive
            // (draft -01 § SBE Annotations), then stored by the field's kind.
            // A value the field cannot hold is refused rather than cut to its
            // low bits.
            let mut raw = [0u8; 8];
            raw[..ft.size].copy_from_slice(&data[off..off + ft.size]);
            let mut v = u64::from_le_bytes(raw);
            let signed = int.is_signed();
            if signed {
                let shift = 64 - 8 * ft.size as u32;
                v = (((v << shift) as i64) >> shift) as u64;
            }
            let kind = fd.kind();
            // A bool reads any nonzero primitive as true.
            if !matches!(kind, Kind::Bool) {
                let (lo, hi) = kind_range(&kind);
                if !in_range(v, !signed, lo, hi) {
                    return Err(SbeError::new(format!(
                        "sbe: field {}: wire value {} ({}) does not fit a {} field, [{}, {}]",
                        fd.full_name(),
                        int_string(v, !signed),
                        int.name(),
                        kind_name(&kind),
                        lo,
                        hi
                    )));
                }
            }
            integer_value(&kind, v)
        }
    };
    parent.set_field(fd, value);
    Ok(())
}

/// An integer that fits `kind` as the [`Value`] that kind takes. `v` holds
/// an `i64`'s bits or a `u64`, whichever the primitive was; once it fits,
/// both read the same.
fn integer_value(kind: &Kind, v: u64) -> Value {
    match kind {
        Kind::Bool => Value::Bool(v != 0),
        Kind::Enum(_) => Value::EnumNumber(v as i32),
        Kind::Int32 | Kind::Sint32 | Kind::Sfixed32 => Value::I32(v as i32),
        Kind::Int64 | Kind::Sint64 | Kind::Sfixed64 => Value::I64(v as i64),
        Kind::Uint32 | Kind::Fixed32 => Value::U32(v as u32),
        _ => Value::U64(v),
    }
}

/// A float or double field's [`Value`]. The template pairs a float or
/// double primitive with a float or double field only, but either with
/// either.
fn float_value(fd: &FieldDescriptor, f: f64) -> Value {
    match fd.kind() {
        Kind::Float => Value::F32(f as f32),
        _ => Value::F64(f),
    }
}

fn read_u16_le(buf: &[u8], off: usize) -> u16 {
    u16::from_le_bytes([buf[off], buf[off + 1]])
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
