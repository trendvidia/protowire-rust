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

use prost_reflect::{FieldDescriptor, Kind, MessageDescriptor};

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

pub(crate) fn empty_name_error(pos: Position, field: &str) -> PxfError {
    PxfError::new(
        pos,
        format!(
            "empty entry name in keyed field {:?}: the empty string is not a valid key",
            field
        ),
    )
}

pub(crate) fn duplicate_key_error(pos: Position, field: &str, key: &str) -> PxfError {
    PxfError::new(
        pos,
        format!("duplicate key {:?} in keyed field {:?}", key, field),
    )
}

pub(crate) fn quoted_name_unkeyed_error(pos: Position, name: &str) -> PxfError {
    PxfError::new(
        pos,
        format!(
            "quoted entry name {:?} is only valid inside a keyed repeated field's block (draft -01 §3.13)",
            name
        ),
    )
}

/// The key-field checks for the immediate body of one element of a keyed
/// repeated field: an explicit assignment to the key field must not be
/// empty, and in the named (keyed-block) form must agree with the entry
/// name (draft -01 §3.13). Set on the decoder for exactly the next body.
#[derive(Debug, Clone)]
pub(crate) struct KeyedElemState {
    /// The keyed repeated field's PXF name.
    pub field: String,
    /// The element message's key field name.
    pub key_name: String,
    /// The entry name; meaningful only when `named`.
    pub entry_name: String,
    /// Keyed-block form (true) vs an anonymous list element (false).
    pub named: bool,
}

impl KeyedElemState {
    pub(crate) fn check_explicit_key(&self, value: &str, pos: Position) -> Result<(), PxfError> {
        if value.is_empty() {
            return Err(PxfError::new(
                pos,
                format!(
                    "explicit empty-string assignment to key field {:?} of keyed field {:?}: the empty string is not a valid key",
                    self.key_name, self.field
                ),
            ));
        }
        if self.named && value != self.entry_name {
            return Err(PxfError::new(
                pos,
                format!(
                    "key field {:?} = {:?} conflicts with entry name {:?} in keyed field {:?}",
                    self.key_name, value, self.entry_name, self.field
                ),
            ));
        }
        Ok(())
    }
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
/// [`crate::format`]; that pair is the `pxf fmt` pipeline.
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
            Entry::MapEntry(_) => {}
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
        drop_key_assignments(&mut eb.entries, &key_name, &eb.name);
        canon_entries(&mut eb.entries, &elem);
    }
}

/// Remove `key_name = "entry_name"` assignments — the redundant agreeing
/// spelling of an entry's key. Disagreeing or non-string assignments are
/// kept (the document is invalid; formatting must not silently change its
/// meaning). Leading comments of a dropped assignment move to the next
/// surviving entry.
fn drop_key_assignments(entries: &mut Vec<Entry>, key_name: &str, entry_name: &str) {
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
                Entry::MapEntry(n) => &mut n.leading_comments,
            };
            let mut merged = std::mem::take(&mut pending);
            merged.append(target);
            *target = merged;
        }
        out.push(e);
    }
    *entries = out;
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
    };
    for (v, key) in lv.elements.drain(..).zip(keys) {
        let Value::Block(BlockVal { pos, mut entries }) = v else {
            unreachable!("eligibility checked every element is a block")
        };
        drop_key_assignments(&mut entries, key_name, &key);
        canon_entries(&mut entries, &elem);
        blk.entries.push(Entry::Block(Block {
            pos,
            name_quoted: !ident_safe_entry_name(&key),
            name: key,
            entries,
            leading_comments: Vec::new(),
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
