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
use crate::gofmt::go_g;
use crate::template::{
    float_fits, in_range, int_string, FieldTemplate, GroupTemplate, MessageTemplate, SbeEncoding,
    MAX_UINT16,
};

pub fn marshal(codec: &Codec, msg: &DynamicMessage) -> Result<Vec<u8>, SbeError> {
    let tmpl = codec.template(msg.descriptor().full_name())?;
    marshal_message(msg, tmpl)
}

fn marshal_message(msg: &DynamicMessage, tmpl: &MessageTemplate) -> Result<Vec<u8>, SbeError> {
    let mut total = HEADER_SIZE + tmpl.block_length;
    for gt in &tmpl.groups {
        let n = list_len(msg, &gt.fd);
        // numInGroup is a u16: a larger count would be written as its low
        // 16 bits, and a reader would take the entries past them for
        // trailing bytes.
        if n > MAX_UINT16 {
            return Err(SbeError::new(format!(
                "sbe: field {} has {} entries, past {}, the largest numInGroup SBE can state",
                gt.fd.full_name(),
                n,
                MAX_UINT16
            )));
        }
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
        SbeEncoding::Float => {
            let f = match &value {
                Value::F32(v) => *v as f64,
                Value::F64(v) => *v,
                _ => 0.0,
            };
            // A double narrowed to float must be within float's range.
            if matches!(ft.fd.kind(), Kind::Double) && !float_fits(f) {
                return Err(SbeError::new(format!(
                    "sbe: field {}: value {} is outside the range of float, ±{}",
                    ft.fd.full_name(),
                    go_g(f),
                    go_g(f32::MAX as f64)
                )));
            }
            buf[off..off + 4].copy_from_slice(&(f as f32).to_le_bytes());
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
            let bytes: &[u8] = match (ft.fd.kind(), &value) {
                (Kind::Bytes, Value::Bytes(b)) => &b[..b.len().min(ft.size)],
                (_, Value::String(s)) => cut_string(s, ft.size).as_bytes(),
                _ => &[],
            };
            buf[off..off + bytes.len()].copy_from_slice(bytes);
            // Remaining bytes already zero from the initial allocation.
        }
        int => {
            // An integer primitive. The value is read by the field's kind,
            // and written as the low bytes of its two's complement once it
            // is known to fit: draft -01 § SBE Annotations, "Emitters MUST
            // reject values that fall outside the narrowed type's range".
            let (v, unsigned) = match value {
                Value::Bool(b) => (b as u64, true),
                Value::EnumNumber(n) => (n as i64 as u64, false),
                Value::U32(n) => (n as u64, true),
                Value::U64(n) => (n, true),
                Value::I32(n) => (n as i64 as u64, false),
                Value::I64(n) => (n as u64, false),
                _ => (0, true),
            };
            let (lo, hi) = int.int_range(ft.size);
            if !in_range(v, unsigned, lo, hi) {
                return Err(SbeError::new(format!(
                    "sbe: field {}: value {} is outside the range of {}, [{}, {}]",
                    ft.fd.full_name(),
                    int_string(v, unsigned),
                    int.name(),
                    lo,
                    hi
                )));
            }
            buf[off..off + ft.size].copy_from_slice(&v.to_le_bytes()[..ft.size]);
        }
    }
    Ok(())
}

fn write_u16_le(buf: &mut [u8], off: usize, v: u16) {
    buf[off..off + 2].copy_from_slice(&v.to_le_bytes());
}

/// Shorten `s` to at most `n` bytes without splitting a UTF-8 character
/// (draft -01 § SBE Annotations, protowire#495): a byte-exact cut can leave
/// half a character, and a strict decoder then rejects the whole message.
/// The bytes the cut leaves over stay NUL. A `bytes` field is cut at the
/// limit instead. Mirrors protowire-go's `cutString`.
fn cut_string(s: &str, mut n: usize) -> &str {
    if s.len() <= n {
        return s;
    }
    while !s.is_char_boundary(n) {
        n -= 1;
    }
    &s[..n]
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
        _ => panic!(
            "group element descriptor on non-message field {}",
            fd.name()
        ),
    }
}
