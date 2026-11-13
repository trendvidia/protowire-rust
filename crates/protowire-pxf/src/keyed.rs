// SPDX-License-Identifier: MIT
// Copyright (c) 2026 TrendVidia, LLC.
//! Keyed repeated fields (draft-trendvidia-protowire-01 §3.13;
//! protowire#116, protowire-rust#20): a `repeated <Message>` field carrying
//! the `(pxf.key)` option may be written as a block of named blocks —
//! entry name = key-field value, entry order = list order:
//!
//! ```text
//! children {
//!   greeting    { type = "Label" }
//!   counter_row { type = "HBox"  }
//! }
//! ```
//!
//! This module holds the schema-layer pieces shared by the decoder, the
//! encoder and tooling: the descriptor helper [`key_field`], the entry-name
//! spelling rule [`ident_safe_entry_name`], the decode-error wording, and
//! the schema-aware fmt canonicalizer [`canonicalize_keyed`]. The decode
//! loop itself is in `decode.rs`, keyed emission in `encode.rs`, and the
//! `(pxf.key)` placement check in `schema.rs`. Mirrors the reference's
//! `encoding/pxf/keyed.go`.

use std::fmt;
use std::sync::Arc;

use prost_reflect::{FieldDescriptor, Kind, MessageDescriptor, OneofDescriptor};

use crate::annotations::get_key;
use crate::ast::{Block, BlockVal, Comment, Document, Entry, Value};
use crate::errors::PxfError;
use crate::token::Position;

/// The key field of a keyed repeated field: the singular string field of
/// `fd`'s element message that `fd`'s `(pxf.key)` annotation names (draft
/// -01 §3.13). `None` when `fd` carries no `(pxf.key)`, or when the
/// annotation's placement is invalid — `fd` is not a repeated
/// message-typed field, the named field does not exist, or it is not a
/// singular string field. [`crate::validate_descriptor`] reports invalid
/// placements as violations; this helper simply declines to treat them as
/// keyed.
pub fn key_field(fd: &FieldDescriptor) -> Option<FieldDescriptor> {
    if !fd.is_list() || fd.is_map() {
        return None;
    }
    let Kind::Message(elem) = fd.kind() else {
        return None;
    };
    let name = get_key(fd)?;
    if name.is_empty() {
        return None;
    }
    let kf = elem.get_field_by_name(&name)?;
    if kf.is_list() || kf.is_map() || !matches!(kf.kind(), Kind::String) {
        return None;
    }
    Some(kf)
}

/// Whether `s` can be written as an unquoted entry name: it matches the
/// identifier production (an ident-start followed by ident-part bytes,
/// dots included) and is not one of the value keywords `null` / `true` /
/// `false`. Canonical form emits unquoted names exactly when this holds
/// (draft -01 §3.13).
pub fn ident_safe_entry_name(s: &str) -> bool {
    if s.is_empty() || s == "true" || s == "false" || s == "null" {
        return false;
    }
    let b = s.as_bytes();
    if !(b[0].is_ascii_alphabetic() || b[0] == b'_') {
        return false;
    }
    b[1..]
        .iter()
        .all(|&c| c.is_ascii_alphanumeric() || c == b'_' || c == b'.')
}

/// What a keyed repeated field error is (draft -01 §3.13), as protowire-go's
/// `KeyedErrorKind`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum KeyedErrorKind {
    /// Two entries in one keyed block whose names are equal once unquoted.
    DuplicateKey = 1,
    /// A binding in a named entry that disagrees with its name: an explicit
    /// key-field assignment of another value, `null` on the key field, or
    /// another member of the key field's oneof.
    KeyConflict,
    /// The empty string as a key: a quoted entry name `""`, or an explicit
    /// empty-string assignment to the key field of an element, in either
    /// surface form.
    EmptyKey,
    /// A quoted entry name anywhere but in a keyed repeated field's block,
    /// where a string never names a field.
    QuotedNameUnkeyed,
}

/// The kind's name, as protowire-go's `KeyedErrorKind.String` gives it.
impl fmt::Display for KeyedErrorKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            KeyedErrorKind::DuplicateKey => "duplicate key",
            KeyedErrorKind::KeyConflict => "key conflict",
            KeyedErrorKind::EmptyKey => "empty key",
            KeyedErrorKind::QuotedNameUnkeyed => "quoted entry name outside keyed field",
        })
    }
}

/// A keyed repeated field error (draft -01 §3.13), as protowire-go's
/// `KeyedError`.
///
/// A decode that refuses a document for one fails with a [`PxfError`] of
/// the same position and text, whose [`PxfError::keyed_error`] is this.
/// [`keyed_diagnostics`] gives every one in a parsed document.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct KeyedError {
    pub pos: Position,
    pub kind: KeyedErrorKind,
    /// The keyed repeated field's name. Empty for
    /// [`KeyedErrorKind::QuotedNameUnkeyed`], where there is no keyed field.
    pub field: String,
    /// The offending key or entry name, when there is one.
    pub key: String,
    pub msg: String,
}

/// `line:col: msg`, as protowire-go's `KeyedError.Error` prints.
impl fmt::Display for KeyedError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}: {}", self.pos, self.msg)
    }
}

impl std::error::Error for KeyedError {}

/// The decode error for a keyed error: its position and text, with the
/// keyed error as the source.
impl From<KeyedError> for PxfError {
    fn from(e: KeyedError) -> Self {
        let (pos, msg) = (e.pos, e.msg.clone());
        PxfError::with_source(pos, msg, Arc::new(e))
    }
}

fn keyed_empty_name(pos: Position, field: &str) -> KeyedError {
    KeyedError {
        pos,
        kind: KeyedErrorKind::EmptyKey,
        field: field.to_string(),
        key: String::new(),
        msg: format!(
            "empty entry name in keyed field {:?}: the empty string is not a valid key",
            field
        ),
    }
}

fn keyed_duplicate_key(pos: Position, field: &str, key: &str) -> KeyedError {
    KeyedError {
        pos,
        kind: KeyedErrorKind::DuplicateKey,
        field: field.to_string(),
        key: key.to_string(),
        msg: format!("duplicate key {:?} in keyed field {:?}", key, field),
    }
}

fn keyed_quoted_name_unkeyed(pos: Position, name: &str) -> KeyedError {
    KeyedError {
        pos,
        kind: KeyedErrorKind::QuotedNameUnkeyed,
        field: String::new(),
        key: name.to_string(),
        msg: format!(
            "quoted entry name {:?} is only valid inside a keyed repeated field's block (draft -01 §3.13)",
            name
        ),
    }
}

// The decoder's errors are built out of line and returned as a `PxfError`,
// the size of the decoder's other errors: a `KeyedError` in the decoder's
// recursive frames grows them past what a deep document's stack allows.

#[cold]
#[inline(never)]
pub(crate) fn empty_name_error(pos: Position, field: &str) -> PxfError {
    keyed_empty_name(pos, field).into()
}

#[cold]
#[inline(never)]
pub(crate) fn duplicate_key_error(pos: Position, field: &str, key: &str) -> PxfError {
    keyed_duplicate_key(pos, field, key).into()
}

#[cold]
#[inline(never)]
pub(crate) fn quoted_name_unkeyed_error(pos: Position, name: &str) -> PxfError {
    keyed_quoted_name_unkeyed(pos, name).into()
}

/// The key-field checks for the immediate body of one element of a keyed
/// repeated field: an explicit assignment to the key field must not be
/// empty, and in the named (keyed-block) form must agree with the entry
/// name (draft -01 §3.13). Set on the decoder for exactly the next body,
/// and used the same way by [`keyed_diagnostics`].
#[derive(Debug, Clone)]
pub(crate) struct KeyedElemState {
    /// The keyed repeated field's PXF name.
    pub field: String,
    /// The element message's key field.
    pub key_fd: FieldDescriptor,
    /// The entry name; meaningful only when `named`.
    pub entry_name: String,
    /// Keyed-block form (true) vs an anonymous list element (false).
    pub named: bool,
}

impl KeyedElemState {
    /// [`Self::explicit_key_conflict`], as the decoder's error.
    pub(crate) fn check_explicit_key(&self, value: &str, pos: Position) -> Result<(), PxfError> {
        match self.explicit_key_conflict(value, pos) {
            None => Ok(()),
            Some(e) => Err(e.into()),
        }
    }

    /// [`Self::binding_conflict`], as the decoder's error.
    pub(crate) fn check_binding(
        &self,
        fd: &FieldDescriptor,
        is_null: bool,
        pos: Position,
    ) -> Result<(), PxfError> {
        match self.binding_conflict(fd, is_null, pos) {
            None => Ok(()),
            Some(e) => Err(e.into()),
        }
    }

    /// An explicit assignment of `value` to the key field: the empty string
    /// is never a key, and in a named entry it must be the entry name.
    #[inline(never)]
    fn explicit_key_conflict(&self, value: &str, pos: Position) -> Option<KeyedError> {
        if value.is_empty() {
            return Some(KeyedError {
                pos,
                kind: KeyedErrorKind::EmptyKey,
                field: self.field.clone(),
                key: String::new(),
                msg: format!(
                    "explicit empty-string assignment to key field {:?} of keyed field {:?}: the empty string is not a valid key",
                    self.key_fd.name(),
                    self.field
                ),
            });
        }
        if self.named && value != self.entry_name {
            return Some(KeyedError {
                pos,
                kind: KeyedErrorKind::KeyConflict,
                field: self.field.clone(),
                key: self.entry_name.clone(),
                msg: format!(
                    "key field {:?} = {:?} conflicts with entry name {:?} in keyed field {:?}",
                    self.key_fd.name(),
                    value,
                    self.entry_name,
                    self.field
                ),
            });
        }
        None
    }

    /// A binding in a named entry's body that would undo the entry name,
    /// which binds the key field (draft -01 §3.13): `null` on the key
    /// field, or another member of the key field's oneof, by value or `null`.
    #[inline(never)]
    fn binding_conflict(
        &self,
        fd: &FieldDescriptor,
        is_null: bool,
        pos: Position,
    ) -> Option<KeyedError> {
        if !self.named {
            return None;
        }
        let msg = if fd.name() == self.key_fd.name() {
            if !is_null {
                return None;
            }
            format!(
                "null on key field {:?} conflicts with entry name {:?} in keyed field {:?}",
                self.key_fd.name(),
                self.entry_name,
                self.field
            )
        } else {
            match (real_oneof(fd), real_oneof(&self.key_fd)) {
                (Some(oo), Some(key_oo)) if oo == key_oo => format!(
                    "field {:?} shares oneof {:?} with key field {:?}, which entry name {:?} already set in keyed field {:?}",
                    fd.name(),
                    oo.name(),
                    self.key_fd.name(),
                    self.entry_name,
                    self.field
                ),
                _ => return None,
            }
        };
        Some(KeyedError {
            pos,
            kind: KeyedErrorKind::KeyConflict,
            field: self.field.clone(),
            key: self.entry_name.clone(),
            msg,
        })
    }
}

/// Every keyed repeated field error in `doc` against `desc` (draft -01
/// §3.13), in source order: duplicate entry names in a keyed block, empty
/// keys, bindings in a named entry that disagree with its name, and quoted
/// entry names outside a keyed field's block. A port of protowire-go's
/// `KeyedDiagnostics`.
///
/// It is the tolerant counterpart of the errors a decode refuses with:
/// editor tooling parses a document, binds a schema later, and shows these
/// as diagnostics instead of failing. Entries the schema does not resolve
/// (unknown fields, a value of the wrong shape) are skipped, since those
/// are ordinary schema errors. So is the body of an `Any`, which needs a
/// type resolver, and so are `@dataset` rows.
pub fn keyed_diagnostics(doc: &Document, desc: &MessageDescriptor) -> Vec<KeyedError> {
    let mut out = Vec::new();
    diag_entries(&doc.entries, desc, None, &mut out);
    out.sort_by_key(|e| e.pos.offset);
    out
}

fn diag_entries(
    entries: &[Entry],
    desc: &MessageDescriptor,
    ke: Option<&KeyedElemState>,
    out: &mut Vec<KeyedError>,
) {
    for entry in entries {
        match entry {
            Entry::Assignment(a) => {
                if a.key_quoted {
                    out.push(keyed_quoted_name_unkeyed(a.pos, &a.key));
                    continue;
                }
                let Some(fd) = desc.get_field_by_name(&a.key) else {
                    continue;
                };
                if let Some(ke) = ke {
                    let is_null = matches!(a.value, Value::Null(_));
                    out.extend(ke.binding_conflict(&fd, is_null, a.pos));
                    if fd.name() == ke.key_fd.name() {
                        if let Value::String(sv) = &a.value {
                            out.extend(ke.explicit_key_conflict(&sv.value, sv.pos));
                        }
                    }
                }
                diag_value(&a.value, &fd, out);
            }
            Entry::Block(b) => {
                if b.name_quoted {
                    out.push(keyed_quoted_name_unkeyed(b.pos, &b.name));
                    continue;
                }
                let Some(fd) = desc.get_field_by_name(&b.name) else {
                    continue;
                };
                if let Some(ke) = ke {
                    out.extend(ke.binding_conflict(&fd, false, b.pos));
                }
                if let Some(key_fd) = key_field(&fd) {
                    diag_keyed_block(&b.entries, &fd, &key_fd, out);
                    continue;
                }
                let Kind::Message(inner) = fd.kind() else {
                    continue;
                };
                if fd.is_list() {
                    // The block form on a repeated field with no
                    // (pxf.key) is an ordinary schema error, but a quoted
                    // entry name inside it is this diagnostic.
                    for be in &b.entries {
                        match be {
                            Entry::Assignment(ba) if ba.key_quoted => {
                                out.push(keyed_quoted_name_unkeyed(ba.pos, &ba.key));
                            }
                            Entry::Block(bb) if bb.name_quoted => {
                                out.push(keyed_quoted_name_unkeyed(bb.pos, &bb.name));
                            }
                            _ => {}
                        }
                    }
                    continue;
                }
                if !fd.is_map() {
                    diag_entries(&b.entries, &inner, None, out);
                }
            }
            Entry::MapEntry(_) | Entry::TypeEntry(_) => {}
        }
    }
}

/// Descends into an assignment's value by `fd`'s shape: a keyed block,
/// anonymous list elements, a singular message, and message map values.
fn diag_value(v: &Value, fd: &FieldDescriptor, out: &mut Vec<KeyedError>) {
    if fd.is_map() {
        let Value::Block(bv) = v else {
            return;
        };
        let Kind::Message(entry) = fd.kind() else {
            return;
        };
        let Kind::Message(value_desc) = entry.map_entry_value_field().kind() else {
            return;
        };
        for e in &bv.entries {
            if let Entry::MapEntry(me) = e {
                if let Value::Block(inner) = &me.value {
                    diag_entries(&inner.entries, &value_desc, None, out);
                }
            }
        }
        return;
    }
    let Kind::Message(elem) = fd.kind() else {
        return;
    };
    if fd.is_list() {
        let key_fd = key_field(fd);
        match v {
            Value::List(lv) => {
                let state = key_fd.map(|key_fd| KeyedElemState {
                    field: fd.name().to_string(),
                    key_fd,
                    entry_name: String::new(),
                    named: false,
                });
                for el in &lv.elements {
                    if let Value::Block(bv) = el {
                        diag_entries(&bv.entries, &elem, state.as_ref(), out);
                    }
                }
            }
            // The `name = { ... }` spelling of the keyed block form.
            Value::Block(bv) => {
                if let Some(key_fd) = key_fd {
                    diag_keyed_block(&bv.entries, fd, &key_fd, out);
                }
            }
            _ => {}
        }
        return;
    }
    if let Value::Block(bv) = v {
        diag_entries(&bv.entries, &elem, None, out);
    }
}

/// Checks the entries of one keyed block: empty and duplicate names,
/// compared unquoted, then each entry's body with the key-field context.
fn diag_keyed_block(
    entries: &[Entry],
    fd: &FieldDescriptor,
    key_fd: &FieldDescriptor,
    out: &mut Vec<KeyedError>,
) {
    let Kind::Message(elem) = fd.kind() else {
        return;
    };
    let mut seen = std::collections::HashSet::new();
    for entry in entries {
        let (name, pos, body) = match entry {
            Entry::Block(b) => (&b.name, b.pos, &b.entries),
            Entry::Assignment(a) => match &a.value {
                Value::Block(bv) => (&a.key, a.pos, &bv.entries),
                // A non-block value on a named entry is an ordinary decode
                // error (the element type is a message).
                _ => continue,
            },
            _ => continue,
        };
        if name.is_empty() {
            out.push(keyed_empty_name(pos, fd.name()));
        } else if !seen.insert(name.as_str()) {
            out.push(keyed_duplicate_key(pos, fd.name(), name));
        }
        let state = KeyedElemState {
            field: fd.name().to_string(),
            key_fd: key_fd.clone(),
            entry_name: name.clone(),
            named: true,
        };
        diag_entries(body, &elem, Some(&state), out);
    }
}

/// `fd`'s oneof, unless it is the synthetic one of a proto3 `optional`.
fn real_oneof(fd: &FieldDescriptor) -> Option<OneofDescriptor> {
    fd.containing_oneof().filter(|oo| !oo.is_synthetic())
}

/// Rewrite `doc` in place to the canonical keyed form of draft -01 §3.13,
/// using `desc` as the document's message schema. Per keyed repeated field
/// binding it:
///
/// - converts an eligible anonymous list binding (every element with
///   exactly one non-empty, distinct explicit key assignment) to the keyed
///   block form, removing the now-implicit key assignments;
/// - normalizes `name = { ... }` entry spellings to `name { ... }`;
/// - unquotes quoted entry names that are identifier-safe;
/// - drops redundant (agreeing) explicit key-field assignments inside
///   named entries.
///
/// Bindings that are not eligible for the keyed form — duplicate keys,
/// absent or empty keys — are left in the anonymous form, and entries that
/// don't resolve against the schema are left untouched, so formatting an
/// invalid document never destroys information. Callers follow with
/// [`crate::format()`]; that pair is the `pxf fmt` pipeline.
pub fn canonicalize_keyed(doc: &mut Document, desc: &MessageDescriptor) {
    canon_entries(&mut doc.entries, desc);
}

fn canon_entries(entries: &mut [Entry], desc: &MessageDescriptor) {
    for entry in entries.iter_mut() {
        match entry {
            Entry::Assignment(a) => {
                if a.key_quoted {
                    continue; // invalid outside keyed blocks; leave untouched
                }
                let Some(fd) = desc.get_field_by_name(&a.key) else {
                    continue;
                };
                if let Some(rewritten) = canon_assignment(a, &fd) {
                    *entry = rewritten;
                }
            }
            Entry::Block(b) => {
                if b.name_quoted {
                    continue;
                }
                let Some(fd) = desc.get_field_by_name(&b.name) else {
                    continue;
                };
                if let Some(key_fd) = key_field(&fd) {
                    canon_keyed_entries(b, &fd, &key_fd);
                    continue;
                }
                if let Kind::Message(inner) = fd.kind() {
                    if !fd.is_list() && !fd.is_map() {
                        canon_entries(&mut b.entries, &inner);
                    }
                }
            }
            Entry::MapEntry(_) | Entry::TypeEntry(_) => {}
        }
    }
}

/// Canonicalize one assignment's value per `fd`'s shape; returns a
/// replacement entry when the assignment becomes a keyed block.
fn canon_assignment(a: &mut crate::ast::Assignment, fd: &FieldDescriptor) -> Option<Entry> {
    if fd.is_map() {
        let Kind::Message(entry_desc) = fd.kind() else {
            return None;
        };
        let val_fd = entry_desc.map_entry_value_field();
        let Kind::Message(val_desc) = val_fd.kind() else {
            return None;
        };
        if let Value::Block(bv) = &mut a.value {
            for e in bv.entries.iter_mut() {
                if let Entry::MapEntry(me) = e {
                    if let Value::Block(inner) = &mut me.value {
                        canon_entries(&mut inner.entries, &val_desc);
                    }
                }
            }
        }
        return None;
    }
    if fd.is_list() {
        let Kind::Message(elem) = fd.kind() else {
            return None;
        };
        let key_fd = key_field(fd);
        match &mut a.value {
            Value::List(lv) => {
                if let Some(key_fd) = &key_fd {
                    return canon_anonymous_keyed(a, fd, key_fd);
                }
                for v in lv.elements.iter_mut() {
                    if let Value::Block(bv) = v {
                        canon_entries(&mut bv.entries, &elem);
                    }
                }
            }
            Value::Block(bv) => {
                if let Some(key_fd) = &key_fd {
                    // `children = { ... }` → `children { ... }`.
                    let mut blk = Block {
                        pos: a.pos,
                        name: std::mem::take(&mut a.key),
                        name_quoted: false,
                        entries: std::mem::take(&mut bv.entries),
                        leading_comments: std::mem::take(&mut a.leading_comments),
                        trailing_comments: std::mem::take(&mut bv.trailing_comments),
                    };
                    canon_keyed_entries(&mut blk, fd, key_fd);
                    return Some(Entry::Block(blk));
                }
            }
            _ => {}
        }
        return None;
    }
    if let Kind::Message(inner) = fd.kind() {
        if let Value::Block(bv) = &mut a.value {
            canon_entries(&mut bv.entries, &inner);
        }
    }
    None
}

/// Normalize the entries of a keyed block in place: assignment-spelled
/// entries become blocks, identifier-safe quoted names are unquoted,
/// redundant agreeing key assignments are dropped, and entry bodies are
/// canonicalized recursively.
fn canon_keyed_entries(b: &mut Block, fd: &FieldDescriptor, key_fd: &FieldDescriptor) {
    let Kind::Message(elem) = fd.kind() else {
        return;
    };
    let key_name = key_fd.name().to_string();
    for entry in b.entries.iter_mut() {
        if let Entry::Assignment(a) = entry {
            if let Value::Block(bv) = &mut a.value {
                let blk = Block {
                    pos: a.pos,
                    name: std::mem::take(&mut a.key),
                    name_quoted: a.key_quoted,
                    entries: std::mem::take(&mut bv.entries),
                    leading_comments: std::mem::take(&mut a.leading_comments),
                    trailing_comments: std::mem::take(&mut bv.trailing_comments),
                };
                *entry = Entry::Block(blk);
            }
        }
        let Entry::Block(eb) = entry else {
            continue; // malformed entry; leave untouched
        };
        if eb.name_quoted && ident_safe_entry_name(&eb.name) {
            eb.name_quoted = false;
        }
        let mut left = drop_key_assignments(&mut eb.entries, &key_name, &eb.name);
        left.append(&mut eb.trailing_comments);
        eb.trailing_comments = left;
        canon_entries(&mut eb.entries, &elem);
    }
}

/// Remove `key_name = "entry_name"` assignments — the redundant agreeing
/// spelling of an entry's key. Disagreeing or non-string assignments are
/// kept (the document is invalid; formatting must not silently change its
/// meaning). Leading comments of a dropped assignment move to the next
/// surviving entry; when none follows, they are returned, for the block's
/// trailing comments.
fn drop_key_assignments(
    entries: &mut Vec<Entry>,
    key_name: &str,
    entry_name: &str,
) -> Vec<Comment> {
    let mut pending: Vec<Comment> = Vec::new();
    let mut out: Vec<Entry> = Vec::with_capacity(entries.len());
    for mut e in entries.drain(..) {
        if let Entry::Assignment(a) = &e {
            if !a.key_quoted && a.key == key_name {
                if let Value::String(sv) = &a.value {
                    if sv.value == entry_name {
                        pending.extend(a.leading_comments.iter().cloned());
                        continue;
                    }
                }
            }
        }
        if !pending.is_empty() {
            let target = match &mut e {
                Entry::Assignment(n) => &mut n.leading_comments,
                Entry::Block(n) => &mut n.leading_comments,
                Entry::TypeEntry(t) => &mut t.leading_comments,
                Entry::MapEntry(n) => &mut n.leading_comments,
            };
            let mut merged = std::mem::take(&mut pending);
            merged.append(target);
            *target = merged;
        }
        out.push(e);
    }
    *entries = out;
    pending
}

/// Convert an eligible anonymous list binding of a keyed repeated field to
/// the keyed block form. Ineligible bindings (non-block elements, absent /
/// empty / duplicate / non-string keys) stay anonymous; their element
/// bodies are still canonicalized.
fn canon_anonymous_keyed(
    a: &mut crate::ast::Assignment,
    fd: &FieldDescriptor,
    key_fd: &FieldDescriptor,
) -> Option<Entry> {
    let Kind::Message(elem) = fd.kind() else {
        return None;
    };
    let key_name = key_fd.name();
    let Value::List(lv) = &mut a.value else {
        return None;
    };
    let mut keys: Vec<String> = Vec::with_capacity(lv.elements.len());
    let mut seen = std::collections::HashSet::new();
    let mut eligible = true;
    for v in &lv.elements {
        let Value::Block(bv) = v else {
            eligible = false;
            break;
        };
        match explicit_key_of(&bv.entries, key_name) {
            Some(key) if !key.is_empty() && seen.insert(key.clone()) => keys.push(key),
            _ => {
                eligible = false;
                break;
            }
        }
    }
    if !eligible {
        for v in lv.elements.iter_mut() {
            if let Value::Block(bv) = v {
                canon_entries(&mut bv.entries, &elem);
            }
        }
        return None;
    }
    let mut blk = Block {
        pos: a.pos,
        name: std::mem::take(&mut a.key),
        name_quoted: false,
        entries: Vec::with_capacity(lv.elements.len()),
        leading_comments: std::mem::take(&mut a.leading_comments),
        trailing_comments: Vec::new(),
    };
    for (v, key) in lv.elements.drain(..).zip(keys) {
        let Value::Block(BlockVal {
            pos,
            mut entries,
            trailing_comments,
        }) = v
        else {
            unreachable!("eligibility checked every element is a block")
        };
        let mut left = drop_key_assignments(&mut entries, key_name, &key);
        left.extend(trailing_comments);
        canon_entries(&mut entries, &elem);
        blk.entries.push(Entry::Block(Block {
            pos,
            name_quoted: !ident_safe_entry_name(&key),
            name: key,
            entries,
            leading_comments: Vec::new(),
            trailing_comments: left,
        }));
    }
    Some(Entry::Block(blk))
}

/// The value of the single explicit string assignment to `key_name` among
/// `entries`. `None` when there is no such assignment, more than one, a
/// quoted-key spelling, or a non-string value.
fn explicit_key_of(entries: &[Entry], key_name: &str) -> Option<String> {
    let mut found: Option<String> = None;
    for e in entries {
        let Entry::Assignment(a) = e else { continue };
        if a.key != key_name {
            continue;
        }
        let Value::String(sv) = &a.value else {
            return None;
        };
        if a.key_quoted || found.is_some() {
            return None;
        }
        found = Some(sv.value.clone());
    }
    found
}
