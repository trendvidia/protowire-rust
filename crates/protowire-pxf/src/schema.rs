// SPDX-License-Identifier: MIT
// Copyright (c) 2026 TrendVidia, LLC.
//! PXF schema-level conformance checks. Four families, all reported as
//! [`Violation`] and all enforced at descriptor-bind time:
//!
//! - Reserved names (draft §3.13 / -01 §schema-constraints). A schema
//!   bound for PXF use MUST NOT declare a message field, oneof, or enum
//!   value whose name is case-sensitively equal to a PXF value keyword
//!   (`null` / `true` / `false`): such a name lexes as the keyword, so
//!   the declared element is unreachable from PXF surface syntax.
//! - `(pxf.key)` placement (draft -01 §3.13.1 "Schema Placement"), see
//!   [`check_key_option`].
//! - `(pxf.default)` placement (draft -01 §annotation-extensions,
//!   "Default Placement"), see [`check_default_option`]; plus the cap of
//!   one `(pxf.default)` per oneof (same section, "Oneof Members"), see
//!   [`check_oneof_default_cap`].
//! - `(pxf.required)` placement ("Oneof Members"): not valid on a member
//!   of a oneof at all.
//!
//! Enforcement runs at descriptor-bind time inside [`crate::unmarshal`]
//! and [`crate::unmarshal_full`]. Callers that have already validated
//! their descriptors (typically via [`validate_descriptor`] in a
//! one-time codegen or registry-load pass) may set
//! [`crate::UnmarshalOptions::skip_validate`] to bypass the per-call
//! recheck.
//!
//! All four families are scoped to the **import closure** of the bound
//! descriptor, not to the file declaring it (draft -01
//! §schema-constraints, "Scope of Bind-Time Checks"): a violation in an
//! imported `.proto` is reported, and [`Violation::file`] names the file
//! that declares it. Per-file results are memoized, so the closure walk
//! costs less than the single-file walk it replaced.
//!
//! Mirrors `protowire-go/encoding/pxf/schema.go`.

use prost_reflect::{
    EnumDescriptor, FieldDescriptor, FileDescriptor, Kind, MessageDescriptor, OneofDescriptor,
};
use std::collections::HashMap;
use std::fmt;
use std::sync::{Arc, LazyLock, Mutex};

use crate::annotations::{get_default, get_key, is_required};

/// Which bind-time check an element failed: which kind of schema element
/// collides with a reserved PXF value keyword, or which annotation is
/// misplaced.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ViolationKind {
    /// A message field whose name is reserved.
    Field,
    /// A oneof declaration whose name is reserved.
    Oneof,
    /// An enum value whose name is reserved.
    EnumValue,
    /// A `(pxf.key)` annotation whose placement violates draft -01
    /// §3.13.1: the annotated field is not a repeated message-typed
    /// field, or the annotation value does not name a singular string
    /// field of the element message.
    KeyOption,
    /// A `(pxf.default)` annotation whose placement violates draft -01
    /// "Default Placement": the annotation carries exactly one PXF
    /// literal, and the annotated field is one no single literal can
    /// denote — a repeated field, a map field, a group, or a
    /// message-typed field outside the set the decoder honors — or a
    /// second default on one oneof ("Oneof Members").
    DefaultOption,
    /// A `(pxf.required)` annotation on a member of a oneof, which draft
    /// -01 "Oneof Members" forbids: read per field it demands that one
    /// specific arm always be chosen, which makes every other arm of the
    /// oneof undecodable.
    RequiredOption,
}

impl ViolationKind {
    fn label(self) -> &'static str {
        match self {
            ViolationKind::Field => "message field",
            ViolationKind::Oneof => "oneof",
            ViolationKind::EnumValue => "enum value",
            ViolationKind::KeyOption => "keyed field option",
            ViolationKind::DefaultOption => "default field option",
            ViolationKind::RequiredOption => "required field option",
        }
    }
}

impl fmt::Display for ViolationKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.label())
    }
}

/// One schema element that fails a PXF bind-time check: a name colliding
/// with a reserved PXF keyword, or an invalid `(pxf.key)`, `(pxf.default)`
/// or `(pxf.required)` placement. Returned by [`validate_descriptor`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Violation {
    /// `.proto` file path the offending element is declared in — which
    /// need not be the bound descriptor's own file.
    pub file: String,
    /// Fully-qualified protobuf name, e.g. `"trades.v1.Side.null"`.
    pub element: String,
    /// The bare reserved identifier (`"null"` / `"true"` / `"false"`) for
    /// the reserved-name kinds; the `(pxf.key)` annotation value for
    /// [`ViolationKind::KeyOption`]; the `(pxf.default)` literal for
    /// [`ViolationKind::DefaultOption`]; the containing oneof's bare name
    /// for [`ViolationKind::RequiredOption`] (not rendered by `Display`,
    /// since `detail` already names the oneof).
    pub name: String,
    pub kind: ViolationKind,
    /// A human-readable explanation; set for the three annotation kinds,
    /// empty for the reserved-name kinds.
    pub detail: String,
}

impl fmt::Display for Violation {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self.kind {
            ViolationKind::KeyOption => write!(
                f,
                "{}: field \"{}\": invalid (pxf.key) = \"{}\": {} (draft -01 §3.13)",
                self.file, self.element, self.name, self.detail
            ),
            ViolationKind::DefaultOption => write!(
                f,
                "{}: field \"{}\": invalid (pxf.default) = \"{}\": {} (draft -01 §annotation-extensions)",
                self.file, self.element, self.name, self.detail
            ),
            ViolationKind::RequiredOption => write!(
                f,
                "{}: field \"{}\": invalid (pxf.required): {} (draft -01 §annotation-extensions)",
                self.file, self.element, self.detail
            ),
            ViolationKind::Field | ViolationKind::Oneof | ViolationKind::EnumValue => write!(
                f,
                "{}: {} \"{}\" uses PXF-reserved name \"{}\" (draft §3.13)",
                self.file, self.kind, self.element, self.name
            ),
        }
    }
}

/// Walks the file containing `desc` **together with its transitive
/// imports**, and returns every bind-time violation in that closure:
/// reserved-name collisions among messages, oneofs and enum values;
/// invalid `(pxf.key)` placements (draft -01 §3.13.1); invalid
/// `(pxf.default)` placements ("Default Placement"); and the two oneof
/// rules of "Oneof Members" — at most one member of any one oneof may
/// carry `(pxf.default)`, and `(pxf.required)` is not valid on a oneof
/// member at all. Sorted by declaring file path and then by element
/// fully-qualified name, for stable output. Empty means conformant.
///
/// The reserved-name check is case-sensitive: identifiers such as `NULL`
/// or `True` lex as ordinary identifiers and are accepted.
///
/// Scope is the import closure, per draft -01 §schema-constraints ("Scope
/// of Bind-Time Checks"): a misplaced annotation on a message type
/// declared in an imported `.proto` is reported here whether or not any
/// field of `desc` refers to that type, and [`Violation::file`] names the
/// file that declares it. One non-conforming file therefore fails every
/// file that transitively imports it — intended, since a widely-imported
/// file is where a dead annotation does the most damage.
///
/// Results are memoized per file descriptor (by identity, not path), so
/// this costs less than the single-file walk it replaced. The returned
/// vector is a fresh copy on every call and callers may modify it.
pub fn validate_descriptor(desc: &MessageDescriptor) -> Vec<Violation> {
    validate_file(&desc.parent_file())
}

/// Walks `fd` and its transitive imports, and returns every bind-time
/// violation in that closure. See [`validate_descriptor`] for the rules
/// and the scope.
///
/// Deduplication is by path rather than by descriptor identity: within
/// one closure a path names one file, and the diamond — A imports B and
/// C, both importing D — is the common shape that would otherwise report
/// D's violations twice.
pub fn validate_file(fd: &FileDescriptor) -> Vec<Violation> {
    if let Some(vs) = CLOSURE_CACHE.get(fd) {
        // A conformant closure caches as an empty slice, and cloning an
        // empty slice neither allocates nor copies — which is the whole
        // hot path, since a non-empty result fails the decode that asked.
        return vs.to_vec();
    }
    let mut walk = ClosureWalk::default();
    walk.walk(fd);
    let mut out = walk.out;
    // Stable, not unstable: one field can yield up to four violations
    // sharing an element (reserved name, key, default placement, and one
    // of the two oneof rules), and only a stable sort keeps "sorted for
    // stable output" true for those ties. File first, so a multi-file
    // closure reports one file's violations together.
    out.sort_by(|a, b| a.file.cmp(&b.file).then_with(|| a.element.cmp(&b.element)));
    let shared: Arc<[Violation]> = out.into();
    CLOSURE_CACHE.insert(fd, shared.clone());
    shared.to_vec()
}

/// The state of one [`validate_file`] traversal.
#[derive(Default)]
struct ClosureWalk {
    /// Paths already walked. Import closures are small — a handful of
    /// files even for schemas that import widely — so a linear scan beats
    /// a map.
    seen: Vec<String>,
    out: Vec<Violation>,
}

impl ClosureWalk {
    fn walk(&mut self, fd: &FileDescriptor) {
        let path = fd.name();
        if self.seen.iter().any(|s| s == path) {
            return;
        }
        self.seen.push(path.to_string());
        self.out.extend(file_violations(fd).iter().cloned());
        for dep in fd.dependencies() {
            self.walk(&dep);
        }
    }
}

/// The violations declared in `fd` itself, ignoring its imports. Memoized
/// and shared between callers; [`ClosureWalk::walk`] only ever clones out
/// of it.
fn file_violations(fd: &FileDescriptor) -> Arc<[Violation]> {
    if let Some(vs) = FILE_CACHE.get(fd) {
        return vs;
    }
    let path = fd.name().to_string();
    let mut out = Vec::new();
    for msg in fd.messages() {
        walk_message(&path, &msg, &mut out);
    }
    for en in fd.enums() {
        walk_enum(&path, &en, &mut out);
    }
    // Deliberately unsorted: validate_file sorts the assembled closure
    // once, and walk_message appends deterministically.
    let shared: Arc<[Violation]> = out.into();
    FILE_CACHE.insert(fd, shared.clone());
    shared
}

// Bind-time validation is memoized at two grains, because validate_file
// runs per decode for every caller that does not set skip_validate and
// the closure walk is far too expensive to repeat there: every schema
// using the annotations imports pxf/annotations.proto and through it
// google/protobuf/descriptor.proto — 54 messages and enums no PXF
// document can name.
//
//   - CLOSURE_CACHE holds the finished result for a bound file. It is the
//     hot path: a hit skips the traversal outright.
//   - FILE_CACHE holds one file's own violations, and is what the
//     traversal consults. It makes a closure-cache miss cheap rather than
//     catastrophic — the shared imports are walked once per process
//     however many schemas pull them in — so a registry-load pass over
//     many descriptors pays for descriptor.proto once, not once per
//     descriptor, and a caller past the bound below still decodes at
//     traversal cost rather than at walk cost.
//
// The key is the descriptor's identity, not its path: two pools can hold
// different files at the same path and must not see each other's
// results. prost-reflect gives a FileDescriptor no Hash, so the key is
// the address of the file's FileDescriptorProto inside the pool's shared
// allocation, which is unique per (pool, file) while the pool lives; the
// entry holds a clone of the FileDescriptor — and through it the pool —
// so that address cannot be reused by another pool while cached, and a
// hit is confirmed by descriptor equality (pool pointer and file index)
// before it is trusted.
//
// The bound exists because a live entry keeps that descriptor's whole
// graph alive. Callers that compile descriptors dynamically would
// otherwise hand these maps an unbounded number of them; they are also
// the callers for whom re-walking is noise, having just paid milliseconds
// to compile. Callers with a fixed schema set never approach the bound.
// Descriptors are immutable, so a cached result never goes stale.
const MAX_CACHED_DESCRIPTORS: usize = 4096;

/// A cached result together with the descriptor that keeps its key's
/// address alive and confirms a hit.
type CacheEntry = (FileDescriptor, Arc<[Violation]>);

struct ValidationCache {
    entries: Mutex<HashMap<usize, CacheEntry>>,
}

impl ValidationCache {
    fn new() -> Self {
        Self {
            entries: Mutex::new(HashMap::new()),
        }
    }

    fn key(fd: &FileDescriptor) -> usize {
        fd.file_descriptor_proto() as *const _ as usize
    }

    fn get(&self, fd: &FileDescriptor) -> Option<Arc<[Violation]>> {
        let entries = self.entries.lock().unwrap_or_else(|e| e.into_inner());
        match entries.get(&Self::key(fd)) {
            Some((cached, vs)) if cached == fd => Some(vs.clone()),
            _ => None,
        }
    }

    fn insert(&self, fd: &FileDescriptor, vs: Arc<[Violation]>) {
        let mut entries = self.entries.lock().unwrap_or_else(|e| e.into_inner());
        if entries.len() >= MAX_CACHED_DESCRIPTORS {
            return;
        }
        entries
            .entry(Self::key(fd))
            .or_insert_with(|| (fd.clone(), vs));
    }
}

static CLOSURE_CACHE: LazyLock<ValidationCache> = LazyLock::new(ValidationCache::new);
static FILE_CACHE: LazyLock<ValidationCache> = LazyLock::new(ValidationCache::new);

fn is_reserved(name: &str) -> bool {
    name == "null" || name == "true" || name == "false"
}

/// Returns `true` when `name` is one of the directive names the spec
/// reserves for future allocation (draft §3.4.6): `"table"`,
/// `"datasource"`, `"view"`, `"procedure"`, `"function"`,
/// `"permissions"`. v1 decoders MUST reject these as unknown reserved
/// directives. The names with their own production (`"type"`,
/// `"dataset"`, `"proto"`) and the spec-registered `"entry"` aren't
/// covered here — they're already handled either by the lexer or the
/// named_directive shape.
pub fn is_future_reserved_directive(name: &str) -> bool {
    matches!(
        name,
        "table" | "datasource" | "view" | "procedure" | "function" | "permissions"
    )
}

/// A oneof member carrying `(pxf.default)`, recorded during the field
/// pass and capped once the message's fields are known.
struct OneofDefault {
    oneof: OneofDescriptor,
    field: FieldDescriptor,
    def: String,
}

fn walk_message(path: &str, md: &MessageDescriptor, out: &mut Vec<Violation>) {
    let mut oneof_defaults: Vec<OneofDefault> = Vec::new();
    for f in md.fields() {
        if is_reserved(f.name()) {
            out.push(Violation {
                file: path.to_string(),
                element: f.full_name().to_string(),
                name: f.name().to_string(),
                kind: ViolationKind::Field,
                detail: String::new(),
            });
        }
        if let Some(key) = get_key(&f) {
            check_key_option(path, &f, &key, out);
        }
        let def = get_default(&f);
        if let Some(def) = &def {
            check_default_option(path, &f, def, out);
        }
        if let Some(oo) = real_oneof(&f) {
            if is_required(&f) {
                out.push(Violation {
                    file: path.to_string(),
                    element: f.full_name().to_string(),
                    name: oo.name().to_string(),
                    kind: ViolationKind::RequiredOption,
                    detail: format!(
                        "(pxf.required) is not valid on a member of oneof \"{}\": read per field it demands that one arm always be chosen, which makes every other arm undecodable",
                        oo.name()
                    ),
                });
            }
            if let Some(def) = def {
                oneof_defaults.push(OneofDefault {
                    oneof: oo,
                    field: f.clone(),
                    def,
                });
            }
        }
    }
    check_oneof_default_cap(path, &oneof_defaults, out);
    // Skip synthetic oneofs (generated for proto3 `optional` fields).
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
                detail: String::new(),
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

/// `f`'s containing oneof, or `None` when `f` is not a member of one. A
/// proto3 `optional` field sits in a synthetic single-member oneof, which
/// carries none of the semantics the oneof rules are about — nothing else
/// can clear it — so it reports `None`, and such fields keep plain
/// per-field presence.
fn real_oneof(f: &FieldDescriptor) -> Option<OneofDescriptor> {
    f.containing_oneof().filter(|oo| !oo.is_synthetic())
}

/// Reports every member of a oneof that carries `(pxf.default)` when a
/// sibling carries one too: at most one member of any one oneof may carry
/// it (draft -01 "Oneof Members"). With two, some default must win, and
/// deciding by declaration order or field number would attach meaning to
/// a detail authors are free to change — so the schema is rejected.
///
/// Reported on every offending member rather than once on the oneof, so
/// the author sees each annotation to remove and `Display` stays a
/// field-shaped message. `annotated` is in field-declaration order;
/// entries for one oneof need not be adjacent, and each group is emitted
/// at its first member's position.
fn check_oneof_default_cap(path: &str, annotated: &[OneofDefault], out: &mut Vec<Violation>) {
    if annotated.len() < 2 {
        return;
    }
    for (i, a) in annotated.iter().enumerate() {
        let members: Vec<&OneofDefault> = annotated.iter().filter(|m| m.oneof == a.oneof).collect();
        if members.len() < 2 || annotated[..i].iter().any(|m| m.oneof == a.oneof) {
            continue; // conformant, or already reported from its first member
        }
        let names: Vec<&str> = members.iter().map(|m| m.field.name()).collect();
        let detail = format!(
            "at most one member of oneof \"{}\" may carry a default; {} do ({})",
            a.oneof.name(),
            members.len(),
            names.join(", ")
        );
        for m in members {
            out.push(Violation {
                file: path.to_string(),
                element: m.field.full_name().to_string(),
                name: m.def.clone(),
                kind: ViolationKind::DefaultOption,
                detail: detail.clone(),
            });
        }
    }
}

/// Validates the placement of a `(pxf.key)` annotation on `f` per draft
/// -01 §3.13.1: the annotated field must be a repeated message-typed
/// field, and the annotation value must name a singular string field of
/// the element message.
fn check_key_option(path: &str, f: &FieldDescriptor, key_name: &str, out: &mut Vec<Violation>) {
    let violation = |detail: String, out: &mut Vec<Violation>| {
        out.push(Violation {
            file: path.to_string(),
            element: f.full_name().to_string(),
            name: key_name.to_string(),
            kind: ViolationKind::KeyOption,
            detail,
        });
    };
    let element = match f.kind() {
        Kind::Message(m) if f.is_list() && !f.is_map() => m,
        _ => {
            violation(
                "(pxf.key) is valid only on repeated message-typed fields".to_string(),
                out,
            );
            return;
        }
    };
    let Some(kf) = element.get_field_by_name(key_name) else {
        violation(
            format!(
                "element message {} has no field \"{}\"",
                element.full_name(),
                key_name
            ),
            out,
        );
        return;
    };
    if kf.is_list() || kf.is_map() || !matches!(kf.kind(), Kind::String) {
        violation(
            format!(
                "key field {} must be a singular string field",
                kf.full_name()
            ),
            out,
        );
    }
}

/// Validates the placement of a `(pxf.default)` annotation on `f` per
/// draft -01 "Default Placement": the annotation carries exactly one PXF
/// literal, so it is valid only on fields a single literal can denote —
/// singular scalars, enums, and the message types
/// [`is_defaultable_message`] names, which is exactly the set the
/// decoder's `apply_message_default` honors. Keep the two in lockstep.
///
/// Placement only — a literal that does not parse as the field's type
/// (`"abc"` on an int32) stays a decode-time error. Placement is
/// decidable from the descriptor alone; the literal is not, without
/// running the value parser here.
fn check_default_option(path: &str, f: &FieldDescriptor, def: &str, out: &mut Vec<Violation>) {
    let violation = |detail: String, out: &mut Vec<Violation>| {
        out.push(Violation {
            file: path.to_string(),
            element: f.full_name().to_string(),
            name: def.to_string(),
            kind: ViolationKind::DefaultOption,
            detail,
        });
    };
    // Map before list: a map field reports is_map (and is_list), and the
    // ordering matches the decoder's runtime guard.
    if f.is_map() {
        violation(
            "(pxf.default) is not valid on map fields: one literal cannot denote a map".to_string(),
            out,
        );
        return;
    }
    if f.is_list() {
        violation(
            "(pxf.default) is not valid on repeated fields: one literal cannot denote a list"
                .to_string(),
            out,
        );
        return;
    }
    if f.is_group() {
        violation(
            "(pxf.default) is not valid on group fields".to_string(),
            out,
        );
        return;
    }
    if let Kind::Message(m) = f.kind() {
        if !is_defaultable_message(m.full_name()) {
            violation(
                format!(
                    "(pxf.default) is not valid on message type {}: no PXF literal denotes it",
                    m.full_name()
                ),
                out,
            );
        }
    }
}

/// The message types a single PXF literal can denote, and so the only
/// message types `(pxf.default)` may be placed on (draft -01 "Default
/// Placement"): `Timestamp`, `Duration`, the nine `*Value` wrappers, and
/// the three `pxf` arbitrary-precision types. `pxf.BigFloat` is in the
/// set because the spec admits it, although this port's decoder cannot
/// apply its literal yet (protowire-rust#39): the placement is
/// conformant, and the literal fails where it is applied.
pub fn is_defaultable_message(full: &str) -> bool {
    matches!(
        full,
        "google.protobuf.Timestamp"
            | "google.protobuf.Duration"
            | "google.protobuf.DoubleValue"
            | "google.protobuf.FloatValue"
            | "google.protobuf.Int64Value"
            | "google.protobuf.UInt64Value"
            | "google.protobuf.Int32Value"
            | "google.protobuf.UInt32Value"
            | "google.protobuf.BoolValue"
            | "google.protobuf.StringValue"
            | "google.protobuf.BytesValue"
            | "pxf.BigInt"
            | "pxf.Decimal"
            | "pxf.BigFloat"
    )
}

fn walk_enum(path: &str, en: &EnumDescriptor, out: &mut Vec<Violation>) {
    for v in en.values() {
        if is_reserved(v.name()) {
            out.push(Violation {
                file: path.to_string(),
                element: v.full_name().to_string(),
                name: v.name().to_string(),
                kind: ViolationKind::EnumValue,
                detail: String::new(),
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
    let mut msg = String::from("PXF schema violations:");
    for v in vs {
        msg.push_str("\n  ");
        msg.push_str(&v.to_string());
    }
    Some(msg)
}
