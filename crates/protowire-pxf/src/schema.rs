// SPDX-License-Identifier: MIT
// Copyright (c) 2026 TrendVidia, LLC.
//! PXF schema-level conformance check per draft §3.13.
//!
//! A protobuf schema bound for PXF use MUST NOT declare a message field,
//! oneof, or enum value whose name is case-sensitively equal to a PXF
//! value keyword (`null` / `true` / `false`) — such a name lexes as the
//! keyword, so the declared element is unreachable from PXF surface
//! syntax.
//!
//! Enforcement runs at descriptor-bind time inside [`crate::unmarshal`]
//! and [`crate::unmarshal_full`]. Callers that have already validated
//! their descriptors (typically via [`validate_descriptor`] in a
//! one-time codegen or registry-load pass) may set
//! [`crate::UnmarshalOptions::skip_validate`] to bypass the per-call
//! recheck.
//!
//! Mirrors `protowire/encoding/pxf/schema.go`.

use prost_reflect::{EnumDescriptor, FileDescriptor, MessageDescriptor};
use std::fmt;

/// Which kind of schema element collides with a reserved PXF value keyword.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ViolationKind {
    Field,
    Oneof,
    EnumValue,
}

impl ViolationKind {
    fn label(self) -> &'static str {
        match self {
            ViolationKind::Field => "message field",
            ViolationKind::Oneof => "oneof",
            ViolationKind::EnumValue => "enum value",
        }
    }
}

impl fmt::Display for ViolationKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.label())
    }
}

/// A schema element whose name collides with a reserved PXF keyword.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Violation {
    /// .proto file path the offending element is declared in.
    pub file: String,
    /// Fully-qualified protobuf name, e.g. "trades.v1.Side.null".
    pub element: String,
    /// Bare reserved identifier — `"null"` / `"true"` / `"false"`.
    pub name: String,
    pub kind: ViolationKind,
}

impl fmt::Display for Violation {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "{}: {} \"{}\" uses PXF-reserved name \"{}\" (draft §3.13)",
            self.file, self.kind, self.element, self.name
        )
    }
}

/// Walks the file containing `desc` and returns every reserved-name
/// collision among messages, oneofs, and enum values reachable from
/// that file. The returned vector is sorted by element fully-qualified
/// name for stable output. An empty vector means the schema is
/// conformant.
///
/// The check is case-sensitive: identifiers such as `NULL` or `True`
/// lex as ordinary identifiers and are accepted.
pub fn validate_descriptor(desc: &MessageDescriptor) -> Vec<Violation> {
    validate_file(&desc.parent_file())
}

/// Walks `fd` and returns every reserved-name collision in the file.
/// See [`validate_descriptor`] for the rule and semantics.
pub fn validate_file(fd: &FileDescriptor) -> Vec<Violation> {
    let path = fd.name().to_string();
    let mut out = Vec::new();
    for msg in fd.messages() {
        walk_message(&path, &msg, &mut out);
    }
    for en in fd.enums() {
        walk_enum(&path, &en, &mut out);
    }
    out.sort_by(|a, b| a.element.cmp(&b.element));
    out
}

fn is_reserved(name: &str) -> bool {
    name == "null" || name == "true" || name == "false"
}

fn walk_message(path: &str, md: &MessageDescriptor, out: &mut Vec<Violation>) {
    for f in md.fields() {
        if is_reserved(f.name()) {
            out.push(Violation {
                file: path.to_string(),
                element: f.full_name().to_string(),
                name: f.name().to_string(),
                kind: ViolationKind::Field,
            });
        }
    }
    // Skip synthetic oneofs (generated for proto3 `optional` fields).
    // prost-reflect's OneofDescriptor::is_synthetic() matches the Go
    // reference's IsSynthetic() filter.
    for o in md.oneofs() {
        if o.is_synthetic() {
            continue;
        }
        if is_reserved(o.name()) {
            out.push(Violation {
                file: path.to_string(),
                element: o.full_name().to_string(),
                name: o.name().to_string(),
                kind: ViolationKind::Oneof,
            });
        }
    }
    for inner in md.child_messages() {
        walk_message(path, &inner, out);
    }
    for en in md.child_enums() {
        walk_enum(path, &en, out);
    }
}

fn walk_enum(path: &str, en: &EnumDescriptor, out: &mut Vec<Violation>) {
    for v in en.values() {
        if is_reserved(v.name()) {
            out.push(Violation {
                file: path.to_string(),
                element: v.full_name().to_string(),
                name: v.name().to_string(),
                kind: ViolationKind::EnumValue,
            });
        }
    }
}

/// Join a list of violations into a single error message suitable for
/// returning from a decode call. Returns `None` when `vs` is empty.
pub(crate) fn as_validation_error_message(vs: &[Violation]) -> Option<String> {
    if vs.is_empty() {
        return None;
    }
    let mut msg = String::from("PXF schema reserved-name violations:");
    for v in vs {
        msg.push_str("\n  ");
        msg.push_str(&v.to_string());
    }
    Some(msg)
}
