// SPDX-License-Identifier: MIT
// Copyright (c) 2026 TrendVidia, LLC.
//! Read SBE schema annotations from File / Message / Field options via the
//! `prost_reflect::DescriptorPool`'s extension registry. Mirrors
//! `protowire/encoding/sbe/annotations.go`.
//!
//! The pool must contain `sbe/annotations.proto` (typically pulled in as a
//! transitive descriptor when the user's schema imports it). When the
//! extension isn't registered, the helpers return `None` rather than
//! erroring — callers decide whether absence is fatal.

use prost_reflect::{
    DescriptorPool, DynamicMessage, FieldDescriptor, FileDescriptor, MessageDescriptor,
};

use crate::errors::SbeError;

pub const EXT_SCHEMA_ID: &str = "sbe.schema_id";
pub const EXT_VERSION: &str = "sbe.version";
pub const EXT_TEMPLATE_ID: &str = "sbe.template_id";
pub const EXT_LENGTH: &str = "sbe.length";
pub const EXT_ENCODING: &str = "sbe.encoding";

// Retired option numbers (STABILITY.md promise 3, #133). Before protowire
// v1.12.0 the SBE options lived at 50100-50301. A descriptor compiled then
// still carries them, and a codec that reads only the registered numbers
// would report such a file as "missing (sbe.schema_id)", sending the reader
// to a .proto that does declare it. The two numbers whose absence the codec
// reports are diagnosed instead, as protowire-go does. protowire-pxf's
// bind-time check covers the whole retired surface; here the file or message
// was handed to the SBE codec explicitly, so no import gate is needed.
const RETIRED_SCHEMA_ID: u32 = 50100;
const RETIRED_TEMPLATE_ID: u32 = 50200;
const SCHEMA_ID_NUMBER: u32 = 1319;
const TEMPLATE_ID_NUMBER: u32 = 1321;

/// Whether `opts` carries field `num` among its unknown fields. Nothing
/// declares the retired numbers any more, so that is where they land.
fn has_unknown_field(opts: &DynamicMessage, num: u32) -> bool {
    opts.unknown_fields().any(|f| f.number() == num)
}

/// The diagnosis for a file whose `(sbe.schema_id)` sits at the retired
/// number, or `None` when it does not: protowire-go's `staleFileError`.
pub(crate) fn stale_file_error(file: &FileDescriptor) -> Option<SbeError> {
    has_unknown_field(&file.options(), RETIRED_SCHEMA_ID).then(|| {
        SbeError::new(format!(
            "sbe: file {} carries (sbe.schema_id) at retired option number {RETIRED_SCHEMA_ID} (registered as {SCHEMA_ID_NUMBER} since protowire v1.12.0): the descriptor predates the registered extension block and must be recompiled against the current sbe/annotations.proto",
            file.name()
        ))
    })
}

/// The same diagnosis for a message whose `(sbe.template_id)` sits at the
/// retired number: protowire-go's `staleMessageError`.
pub(crate) fn stale_message_error(desc: &MessageDescriptor) -> Option<SbeError> {
    has_unknown_field(&desc.options(), RETIRED_TEMPLATE_ID).then(|| {
        SbeError::new(format!(
            "sbe: message {} carries (sbe.template_id) at retired option number {RETIRED_TEMPLATE_ID} (registered as {TEMPLATE_ID_NUMBER} since protowire v1.12.0): the descriptor predates the registered extension block and must be recompiled against the current sbe/annotations.proto",
            desc.full_name()
        ))
    })
}

pub fn file_uint32(file: &FileDescriptor, name: &str) -> Option<u32> {
    let pool = file.parent_pool();
    let ext = pool.get_extension_by_name(name)?;
    let opts = file.options();
    if !opts.has_extension(&ext) {
        return None;
    }
    opts.get_extension(&ext).as_u32()
}

pub fn message_uint32(desc: &MessageDescriptor, name: &str) -> Option<u32> {
    let pool = desc.parent_pool();
    let ext = pool.get_extension_by_name(name)?;
    let opts = desc.options();
    if !opts.has_extension(&ext) {
        return None;
    }
    opts.get_extension(&ext).as_u32()
}

pub fn field_uint32(fd: &FieldDescriptor, name: &str) -> Option<u32> {
    let pool = fd.parent_pool();
    let ext = pool.get_extension_by_name(name)?;
    let opts = fd.options();
    if !opts.has_extension(&ext) {
        return None;
    }
    opts.get_extension(&ext).as_u32()
}

pub fn field_string(fd: &FieldDescriptor, name: &str) -> Option<String> {
    let pool = fd.parent_pool();
    let ext = pool.get_extension_by_name(name)?;
    let opts = fd.options();
    if !opts.has_extension(&ext) {
        return None;
    }
    opts.get_extension(&ext).as_str().map(|s| s.to_string())
}

pub fn has_template_id(desc: &MessageDescriptor) -> bool {
    message_uint32(desc, EXT_TEMPLATE_ID).is_some()
}

/// Convenience: lookup an extension by full name in the pool, returning
/// `None` if missing. The codec uses this in tight loops, so we keep it
/// exposed for callers that want to amortize the pool walk.
pub fn extension(pool: &DescriptorPool, name: &str) -> Option<prost_reflect::ExtensionDescriptor> {
    pool.get_extension_by_name(name)
}
