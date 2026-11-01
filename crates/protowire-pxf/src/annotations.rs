// SPDX-License-Identifier: MIT
// Copyright (c) 2026 TrendVidia, LLC.
//! Reads `pxf.required` (1314), `pxf.default` (1315) and `pxf.key` (1316)
//! custom field options out of a [`FieldDescriptor`]'s options message,
//! their `@required` / `@default` spelling out of the 1327 carrier
//! (`carrier.rs`), plus the `_null` `google.protobuf.FieldMask`
//! lookup. Mirrors the upstream `encoding/pxf/annotations.go` interface.
//!
//! Resolves the extensions through the parent [`DescriptorPool`], so the
//! pool must contain `pxf/annotations.proto` (typically pulled in as a
//! transitive descriptor when the user's schema imports it).

use prost_reflect::{FieldDescriptor, Kind, MessageDescriptor};

use crate::carrier::read_carrier;

const PXF_REQUIRED: &str = "pxf.required";
const PXF_DEFAULT: &str = "pxf.default";
const PXF_KEY: &str = "pxf.key";

/// Which of PXF's two spellings supplied an annotation, so a diagnostic
/// quotes the form the author wrote.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum AnnotationSurface {
    /// `[(pxf.required) = true]`, `[(pxf.default) = "…"]`.
    #[default]
    Option,
    /// `@required`, `@default(…)`.
    Carrier,
}

impl AnnotationSurface {
    pub fn required_name(self) -> &'static str {
        match self {
            AnnotationSurface::Option => "(pxf.required)",
            AnnotationSurface::Carrier => "@required",
        }
    }

    pub fn default_name(self) -> &'static str {
        match self {
            AnnotationSurface::Option => "(pxf.default)",
            AnnotationSurface::Carrier => "@default",
        }
    }
}

/// Every required / default annotation on one field, from both spellings.
#[derive(Debug, Default, PartialEq, Eq)]
pub(crate) struct FieldAnnotations {
    pub required: bool,
    pub default: Option<String>,
    pub required_surface: AnnotationSurface,
    pub default_surface: AnnotationSurface,
    /// Why the field's default can't be applied: an `@default` that reduces
    /// to no literal, or two spellings that disagree.
    pub default_problem: Option<String>,
}

fn bracket_required(fd: &FieldDescriptor) -> bool {
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

fn bracket_default(fd: &FieldDescriptor) -> Option<String> {
    let pool = fd.parent_pool();
    let ext = pool.get_extension_by_name(PXF_DEFAULT)?;
    let opts = fd.options();
    if !opts.has_extension(&ext) {
        return None;
    }
    opts.get_extension(&ext).as_str().map(|s| s.to_string())
}

/// Read both spellings of required and default on `fd`.
///
/// They are not reconciled (RFC-001 §8.5): neither is synthesized from the
/// other. `required` has one answer, so either spelling asserts it. A
/// default carries a value, so the two can disagree, and picking one would
/// attach meaning to which spelling an author migrated first, so that is a
/// problem the schema is rejected for. Equal values are not a disagreement.
pub(crate) fn field_annotations(fd: &FieldDescriptor) -> FieldAnnotations {
    let mut got = FieldAnnotations {
        required: bracket_required(fd),
        default: bracket_default(fd),
        ..Default::default()
    };
    let Some(carrier) = read_carrier(fd) else {
        return got;
    };
    if carrier.required && !got.required {
        got.required = true;
        got.required_surface = AnnotationSurface::Carrier;
    }
    if let Some(problem) = carrier.default_problem {
        // The unusable annotation is the carrier's, so the diagnostic blames
        // @default even when a bracket default sits beside it.
        got.default_problem = Some(problem);
        got.default_surface = AnnotationSurface::Carrier;
    } else if let Some(def) = carrier.default {
        match &got.default {
            None => {
                got.default = Some(def);
                got.default_surface = AnnotationSurface::Carrier;
            }
            Some(bracket) if *bracket != def => {
                got.default_problem = Some(format!(
                    "(pxf.default) = {bracket:?} and @default = {def:?} carry different values, and the two surfaces must not be reconciled (RFC-001 §8.5)"
                ));
            }
            Some(_) => {}
        }
    }
    got
}

/// Whether the field is required, in either spelling: the bracket option
/// or `@required`.
pub fn is_required(fd: &FieldDescriptor) -> bool {
    field_annotations(fd).required
}

/// The field's default literal, in either spelling: the bracket option or
/// `@default(v)`, whose typed argument is reduced to the same literal form.
pub fn get_default(fd: &FieldDescriptor) -> Option<String> {
    field_annotations(fd).default
}

/// The `(pxf.key)` annotation value — the name of the element message's
/// key field for a keyed repeated field (draft -01 §3.13) — or `None`
/// when the field carries none. Placement is checked at bind time by
/// [`crate::validate_descriptor`]; the keyed surface form itself
/// (protowire-rust#20) is not implemented in this port yet.
pub fn get_key(fd: &FieldDescriptor) -> Option<String> {
    let pool = fd.parent_pool();
    let ext = pool.get_extension_by_name(PXF_KEY)?;
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
