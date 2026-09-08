// SPDX-License-Identifier: MIT
// Copyright (c) 2026 TrendVidia, LLC.
//! SBE codec: registers proto messages by template ID and dispatches to the
//! marshal / unmarshal / view paths. Mirrors `protowire/encoding/sbe/sbe.go`.
//!
//! The header sizes are part of the SBE wire contract — see SBE 1.0 spec
//! sections 2.1 (message header) and 2.4 (group header).

use std::collections::HashMap;

use prost_reflect::{FileDescriptor, MessageDescriptor};

use crate::annotations::{file_uint32, has_template_id, EXT_SCHEMA_ID, EXT_VERSION};
use crate::errors::SbeError;
use crate::template::{build_template, MessageTemplate};

/// Message header size: `block_length(2) + template_id(2) + schema_id(2) + version(2)`.
pub const HEADER_SIZE: usize = 8;

/// Repeating-group header size: `block_length(2) + num_in_group(2)`.
pub const GROUP_HEADER_SIZE: usize = 4;

/// HARDENING.md `MaxMessageSize` — caps the total input to one decode or
/// view call, checked before the header is read.
pub const MAX_MESSAGE_SIZE: usize = 64 << 20;

/// HARDENING.md `MaxRepeatedCount` — caps a repeating group's `numInGroup`,
/// checked before any entry is allocated. The wire field is a `u16`, so
/// the default cannot trip on one group; the check holds when a caller
/// lowers it.
pub const MAX_REPEATED_COUNT: usize = MAX_MESSAGE_SIZE;

/// The limits a [`Codec`] decodes under (draft -01 § Mandatory Limits),
/// fixed at construction so the codec stays safe for concurrent use. The
/// default is the HARDENING constants.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Limits {
    /// [`MAX_MESSAGE_SIZE`]: total input to one decode or view.
    pub max_message_size: usize,
    /// [`MAX_REPEATED_COUNT`]: a group's `numInGroup`.
    pub max_repeated_count: usize,
}

impl Default for Limits {
    fn default() -> Self {
        Self {
            max_message_size: MAX_MESSAGE_SIZE,
            max_repeated_count: MAX_REPEATED_COUNT,
        }
    }
}

#[derive(Debug, Clone, Default)]
pub struct Codec {
    by_name: HashMap<String, MessageTemplate>,
    by_id: HashMap<u32, MessageTemplate>,
    limits: Limits,
}

impl Codec {
    /// Build a Codec from one or more file descriptors under the default
    /// [`Limits`]. Each file must declare `(sbe.schema_id)`;
    /// `(sbe.version)` defaults to 0 when absent.
    pub fn from_files(files: &[FileDescriptor]) -> Result<Self, SbeError> {
        Self::from_files_with_limits(files, Limits::default())
    }

    /// [`Codec::from_files`] under the given [`Limits`].
    pub fn from_files_with_limits(
        files: &[FileDescriptor],
        limits: Limits,
    ) -> Result<Self, SbeError> {
        let mut codec = Codec {
            limits,
            ..Codec::default()
        };
        for file in files {
            let schema_id = file_uint32(file, EXT_SCHEMA_ID).ok_or_else(|| {
                SbeError::new(format!(
                    "sbe: file {} missing (sbe.schema_id) option",
                    file.name()
                ))
            })?;
            let version = file_uint32(file, EXT_VERSION).unwrap_or(0);
            for desc in file.messages() {
                codec.register_message(&desc, schema_id, version)?;
            }
        }
        Ok(codec)
    }

    fn register_message(
        &mut self,
        desc: &MessageDescriptor,
        schema_id: u32,
        version: u32,
    ) -> Result<(), SbeError> {
        if has_template_id(desc) {
            let tmpl = build_template(desc, schema_id, version)?;
            let id = tmpl.template_id;
            self.by_name
                .insert(desc.full_name().to_string(), tmpl.clone());
            self.by_id.insert(id, tmpl);
        }
        for nested in desc.child_messages() {
            self.register_message(&nested, schema_id, version)?;
        }
        Ok(())
    }

    pub fn template(&self, type_name: &str) -> Result<&MessageTemplate, SbeError> {
        self.by_name
            .get(type_name)
            .ok_or_else(|| SbeError::new(format!("sbe: no template registered for {}", type_name)))
    }

    pub fn template_by_id(&self, id: u32) -> Result<&MessageTemplate, SbeError> {
        self.by_id
            .get(&id)
            .ok_or_else(|| SbeError::new(format!("sbe: unknown template ID {}", id)))
    }

    pub fn by_name(&self) -> &HashMap<String, MessageTemplate> {
        &self.by_name
    }

    pub fn by_id(&self) -> &HashMap<u32, MessageTemplate> {
        &self.by_id
    }

    /// The limits this codec decodes under.
    pub fn limits(&self) -> Limits {
        self.limits
    }

    /// HARDENING.md `MaxMessageSize`, checked before the header is read.
    pub(crate) fn check_message_size(&self, len: usize) -> Result<(), SbeError> {
        if len > self.limits.max_message_size {
            return Err(SbeError::new(format!(
                "sbe: input of {} bytes exceeds MaxMessageSize={}",
                len, self.limits.max_message_size
            )));
        }
        Ok(())
    }
}

/// Validate a repeating group's header against HARDENING.md § SBE before
/// any entry is allocated or read, returning `(block_length, count,
/// total)` with `total` the bytes the group occupies including its
/// header:
///
/// 1. the header fits (`pos + GROUP_HEADER_SIZE <= data.len()`);
/// 2. a wire `block_length` of zero with a non-zero count is rejected
///    (step 4 — otherwise `count` entries would be allocated for zero
///    further bytes);
/// 3. a wire `block_length` below the template's is rejected (step 2 —
///    some field's `offset + size` would exceed the entry);
/// 4. `count` is at most `max_repeated_count` (step 3, and § Mandatory
///    limits `MaxRepeatedCount`), checked before allocating for it;
/// 5. `pos + GROUP_HEADER_SIZE + count × block_length <= data.len()`, in
///    64-bit arithmetic before any narrowing (step 3).
pub(crate) fn check_group_header(
    data: &[u8],
    pos: usize,
    group_name: &str,
    template_block_length: usize,
    max_repeated_count: usize,
) -> Result<(usize, usize, usize), SbeError> {
    let header_end = pos
        .checked_add(GROUP_HEADER_SIZE)
        .ok_or_else(|| SbeError::new("sbe: group header offset overflows"))?;
    if data.len() < header_end {
        return Err(SbeError::new("sbe: data too short for group header"));
    }
    let block_length = u16::from_le_bytes([data[pos], data[pos + 1]]) as usize;
    let count = u16::from_le_bytes([data[pos + 2], data[pos + 3]]) as usize;
    if block_length == 0 && count > 0 {
        return Err(SbeError::new(format!(
            "sbe: group {} declares {} entries of block_length 0",
            group_name, count
        )));
    }
    if count > 0 && block_length < template_block_length {
        return Err(SbeError::new(format!(
            "sbe: group {} wire block_length {} is below template block_length {}",
            group_name, block_length, template_block_length
        )));
    }
    if count > max_repeated_count {
        return Err(SbeError::new(format!(
            "sbe: group {} declares {} entries, exceeds MaxRepeatedCount={}",
            group_name, count, max_repeated_count
        )));
    }
    let entries = (count as u64)
        .checked_mul(block_length as u64)
        .ok_or_else(|| SbeError::new("sbe: group size overflows"))?;
    let total = (GROUP_HEADER_SIZE as u64)
        .checked_add(entries)
        .ok_or_else(|| SbeError::new("sbe: group size overflows"))?;
    let end = (pos as u64)
        .checked_add(total)
        .ok_or_else(|| SbeError::new("sbe: group size overflows"))?;
    if end > data.len() as u64 {
        return Err(SbeError::new(format!(
            "sbe: data too short for group entries: need {}, have {}",
            end,
            data.len()
        )));
    }
    Ok((block_length, count, total as usize))
}
