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

use prost_reflect::{DescriptorPool, FieldDescriptor, FileDescriptor, MessageDescriptor};

pub const EXT_SCHEMA_ID: &str = "sbe.schema_id";
pub const EXT_VERSION: &str = "sbe.version";
pub const EXT_TEMPLATE_ID: &str = "sbe.template_id";
pub const EXT_LENGTH: &str = "sbe.length";
pub const EXT_ENCODING: &str = "sbe.encoding";

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
