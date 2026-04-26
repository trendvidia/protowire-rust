//! Reads `pxf.required` (50000) and `pxf.default` (50001) custom field
//! options out of a [`FieldDescriptor`]'s options message, plus the
//! `_null` `google.protobuf.FieldMask` lookup. Mirrors the upstream
//! `protowire/encoding/pxf/annotations.go` interface.
//!
//! Resolves the extensions through the parent [`DescriptorPool`], so the
//! pool must contain `pxf/annotations.proto` (typically pulled in as a
//! transitive descriptor when the user's schema imports it).

use prost_reflect::{FieldDescriptor, Kind, MessageDescriptor};

const PXF_REQUIRED: &str = "pxf.required";
const PXF_DEFAULT: &str = "pxf.default";

pub fn is_required(fd: &FieldDescriptor) -> bool {
    let pool = fd.parent_pool();
    let Some(ext) = pool.get_extension_by_name(PXF_REQUIRED) else {
        return false;
    };
    let opts = fd.options();
    if !opts.has_extension(&ext) {
        return false;
    }
    opts.get_extension(&ext).as_bool().unwrap_or(false)
}

pub fn get_default(fd: &FieldDescriptor) -> Option<String> {
    let pool = fd.parent_pool();
    let ext = pool.get_extension_by_name(PXF_DEFAULT)?;
    let opts = fd.options();
    if !opts.has_extension(&ext) {
        return None;
    }
    opts.get_extension(&ext).as_str().map(|s| s.to_string())
}

/// Returns the `_null` field if the message has one of type
/// `google.protobuf.FieldMask`. Both the field name and the message type
/// must match — a stray `_null` of any other type is ignored.
pub fn find_null_mask_field(desc: &MessageDescriptor) -> Option<FieldDescriptor> {
    let fd = desc.get_field_by_name("_null")?;
    if let Kind::Message(inner) = fd.kind() {
        if inner.full_name() == "google.protobuf.FieldMask" {
            return Some(fd);
        }
    }
    None
}
