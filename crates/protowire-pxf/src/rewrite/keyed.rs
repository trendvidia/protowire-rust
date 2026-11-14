// SPDX-License-Identifier: MIT
// Copyright (c) 2026 TrendVidia, LLC.
//! Keyed-collection editing, ported line for line from protowire-go's
//! `encoding/pxf/rewrite_keyed.go` (#131).
//!
//! A keyed repeated field (draft -01 §3.13) is written as a block of named
//! blocks: the entry name is the key, and document order is list order. A
//! dotted path cannot address its elements reliably, because an entry name
//! is an atom: `user.name` is one key, never two segments, and a name that
//! is not identifier-shaped is outside path syntax altogether. These
//! editors take the key as an atom instead.
//!
//! Like the rest of the [`Rewriter`], they are schema-less and work on the
//! collection's shape, whether or not a `(pxf.key)` annotation exists.
//! `field_path` addresses the collection by first-match dotted path, as
//! [`Rewriter::set`] does; `key`, `before`, `old_key` and `new_key` are
//! atoms and never split on dots. An element is matched by its unquoted
//! key, and an entry name is written quoted exactly when it is not
//! identifier-safe.
//!
//! Two facts about a keyed field's surface (§3.13) shape these editors:
//!
//! - **Concatenation:** a keyed field may be bound more than once, and the
//!   bindings concatenate in document order. The editors search every
//!   binding for the key, and an append lands in the last binding.
//! - **Two surface forms:** the keyed-block form (`children { greeting {
//!   … } }`) needs no schema, since the entry name is the key. The
//!   anonymous list form (`children = [ { id = "greeting" } ]`) carries the
//!   key as the key field's value, so the editors address it only once the
//!   caller names that field with [`Rewriter::keyed_by_field`].

use std::sync::Arc;

use super::{
    contains_bad_val, entry_contains_bad_val, entry_key, entry_value, is_blank, line_start_offset,
    render_entry_lines, render_value, resolve_in, write_entry_inline, Container, Rewriter,
    SpanEdit,
};
use crate::ast::{Assignment, Block, BlockVal, Document, Entry, ListVal, StringVal, Value};
use crate::errors::PxfError;
use crate::format::{needs_quoting, quote_string, Formatter};
use crate::keyed::{explicit_key_of, ident_safe_entry_name};
use crate::lexer::is_ident_part;
use crate::token::Position;

fn err(msg: String) -> PxfError {
    PxfError::new(Position::default(), msg)
}

/// One binding of a keyed field, a keyed block or an anonymous list, and
/// its source span.
struct KeyedBinding<'d> {
    anon: bool,
    /// The offset of the binding's entry.
    scope_start: usize,
    /// The offset of its `}` (block form) or `]` (anonymous form).
    close_off: usize,
    /// Block form: the block's entries.
    entries: &'d [Entry],
    /// Anonymous form: the list.
    list: Option<&'d ListVal>,
    /// Anonymous form: the `(pxf.key)` field's name.
    key_field: String,
}

/// One element found in a binding.
#[derive(Clone)]
struct KeyedElem<'d> {
    /// The binding's index.
    b: usize,
    key: String,
    /// Block form: the block or assignment entry.
    node: Option<&'d Entry>,
    /// Anonymous form: the index in the list.
    idx: usize,
    /// The element's body as a block value (anonymous form; `name = {}`).
    bv: Option<&'d BlockVal>,
    /// The element's body entries, for resolving a subpath.
    body: &'d [Entry],
}

impl<'d> KeyedBinding<'d> {
    /// Every addressable element of the binding, in document order.
    fn elems(&self, b: usize) -> Vec<KeyedElem<'d>> {
        let mut out = Vec::new();
        if let Some(list) = self.list.filter(|_| self.anon) {
            for (i, v) in list.elements.iter().enumerate() {
                let Value::Block(bv) = v else { continue };
                let Some(key) = explicit_key_of(&bv.entries, &self.key_field) else {
                    continue;
                };
                if key.is_empty() {
                    continue;
                }
                out.push(KeyedElem {
                    b,
                    key,
                    node: None,
                    idx: i,
                    bv: Some(bv),
                    body: &bv.entries,
                });
            }
            return out;
        }
        for e in self.entries {
            match e {
                Entry::Block(n) => out.push(KeyedElem {
                    b,
                    key: n.name.clone(),
                    node: Some(e),
                    idx: 0,
                    bv: None,
                    body: &n.entries,
                }),
                Entry::Assignment(n) => {
                    if let Value::Block(bv) = &n.value {
                        out.push(KeyedElem {
                            b,
                            key: n.key.clone(),
                            node: Some(e),
                            idx: 0,
                            bv: Some(bv),
                            body: &bv.entries,
                        });
                    }
                }
                _ => {}
            }
        }
        out
    }
}

impl KeyedElem<'_> {
    /// The scope that holds the element's body, for resolving a subpath
    /// and editing values: its entry, or in the anonymous form, which has
    /// no entry, its block value's own span.
    fn container(&self) -> Container {
        match self.node {
            Some(n) => Container::of(n),
            None => {
                let bv = self.bv.expect("an anonymous element has a block value");
                Container {
                    pos: bv.pos.offset,
                    close_off: bv.end.offset as isize - 1,
                }
            }
        }
    }
}

/// The first element named `key` across the bindings, in document order.
fn find_keyed_elem<'d>(bs: &[KeyedBinding<'d>], key: &str) -> Option<KeyedElem<'d>> {
    bs.iter()
        .enumerate()
        .find_map(|(i, b)| b.elems(i).into_iter().find(|e| e.key == key))
}

fn spanless_block(name: &str, name_quoted: bool, entries: Vec<Entry>) -> Block {
    Block {
        pos: Position::default(),
        end: Position::default(),
        name: name.to_string(),
        name_quoted,
        entries,
        leading_comments: Vec::new(),
        trailing_comments: Vec::new(),
    }
}

fn spanless_assignment(key: &str, value: Value) -> Assignment {
    Assignment {
        pos: Position::default(),
        end: Position::default(),
        key: key.to_string(),
        key_quoted: false,
        value,
        leading_comments: Vec::new(),
        trailing_comment: String::new(),
    }
}

/// A new keyed-block-form element, its name quoted exactly when it is not
/// identifier-safe.
fn new_keyed_element_block(key: &str, body: Vec<Entry>) -> Block {
    spanless_block(key, !ident_safe_entry_name(key), body)
}

/// A new anonymous-form element: the key field first, then the body.
fn new_anon_element(key_field: &str, key: &str, body: Vec<Entry>) -> BlockVal {
    let mut entries = Vec::with_capacity(body.len() + 1);
    entries.push(Entry::Assignment(spanless_assignment(
        key_field,
        Value::String(StringVal {
            pos: Position::default(),
            end: Position::default(),
            value: key.to_string(),
        }),
    )));
    entries.extend(body);
    BlockVal {
        pos: Position::default(),
        end: Position::default(),
        entries,
        trailing_comments: Vec::new(),
    }
}

/// An insertion into a list, and the comma it needs after the element
/// before it when that is not next to the insertion: a comment on that
/// element must stay on it.
struct ListSplice {
    at: usize,
    text: Vec<u8>,
    comma_at: Option<usize>,
}

impl Rewriter {
    /// Declares that the keyed collection at `field_path` uses `key_field`
    /// as its `(pxf.key)` field. The keyed editors can then also address a
    /// collection in the anonymous list form (`field_path = [ { <key_field>
    /// = "k" } ]`), where an element's key is the key field's value rather
    /// than an entry name. The keyed-block form needs no declaration. A
    /// declaration matches its exact `field_path`, and a later one for the
    /// same path replaces it. Returns the `Rewriter`, for chaining.
    pub fn keyed_by_field(&mut self, field_path: &str, key_field: &str) -> &mut Self {
        self.key_fields
            .insert(field_path.to_string(), key_field.to_string());
        self
    }

    /// The scope that holds `field_path`'s bindings, and the collection's
    /// own segment (the last), so every binding of the field can be found.
    fn keyed_scope_entries<'d>(
        &self,
        doc: &'d Document,
        field_path: &str,
    ) -> Result<(&'d [Entry], String), String> {
        let Some(segs) = split_path_segs(field_path) else {
            return Err(format!("invalid path {}", quote_string(field_path)));
        };
        let key = segs[segs.len() - 1].to_string();
        if segs.len() == 1 {
            return Ok((&doc.entries, key));
        }
        let parent = segs[..segs.len() - 1].join(".");
        let t = resolve_in(&doc.entries, None, &parent).map_err(|e| e.to_string())?;
        let Some(entry) = t.entry else {
            return Err(format!("no entry at {}", quote_string(&parent)));
        };
        if let Entry::Block(b) = entry {
            return Ok((&b.entries, key));
        }
        if let Some(Value::Block(bv)) = entry_value(entry) {
            return Ok((&bv.entries, key));
        }
        Err(format!("{} is not a block", quote_string(&parent)))
    }

    /// Every binding of the keyed field at `field_path`, in document order.
    /// An anonymous list is one only when the field's key field was
    /// declared with [`Rewriter::keyed_by_field`].
    fn keyed_bindings<'d>(
        &self,
        doc: &'d Document,
        op: &str,
        field_path: &str,
    ) -> Result<Vec<KeyedBinding<'d>>, PxfError> {
        let (scope, key) = self
            .keyed_scope_entries(doc, field_path)
            .map_err(|e| err(format!("pxf: {op}: {e}")))?;
        let key_field = self.key_fields.get(field_path).cloned().unwrap_or_default();
        let mut bs = Vec::new();
        let mut saw_undeclared_list = false;
        // The close offset is signed until it is checked below.
        let mut closes = Vec::new();
        for e in scope {
            if entry_key(e) != key {
                continue;
            }
            match e {
                Entry::Block(n) => {
                    closes.push(n.end.offset as isize - 1);
                    bs.push(KeyedBinding {
                        anon: false,
                        scope_start: n.pos.offset,
                        close_off: 0,
                        entries: &n.entries,
                        list: None,
                        key_field: String::new(),
                    });
                }
                Entry::Assignment(n) => match &n.value {
                    Value::Block(v) => {
                        closes.push(v.end.offset as isize - 1);
                        bs.push(KeyedBinding {
                            anon: false,
                            scope_start: n.pos.offset,
                            close_off: 0,
                            entries: &v.entries,
                            list: None,
                            key_field: String::new(),
                        });
                    }
                    Value::List(v) => {
                        if key_field.is_empty() {
                            saw_undeclared_list = true;
                            continue;
                        }
                        closes.push(v.end.offset as isize - 1);
                        bs.push(KeyedBinding {
                            anon: true,
                            scope_start: n.pos.offset,
                            close_off: 0,
                            entries: &[],
                            list: Some(v),
                            key_field: key_field.clone(),
                        });
                    }
                    _ => {}
                },
                _ => {}
            }
        }
        if bs.is_empty() {
            if saw_undeclared_list {
                return Err(err(format!(
                    "pxf: {op}: {} is written in the anonymous list form; declare its key field with KeyedByField to edit it",
                    quote_string(field_path)
                )));
            }
            return Err(err(format!(
                "pxf: {op}: no keyed collection at {}",
                quote_string(field_path)
            )));
        }
        for (b, close) in bs.iter_mut().zip(closes) {
            let want = if b.anon { b']' } else { b'}' };
            if close < b.scope_start as isize
                || close as usize >= self.src.len()
                || self.src[close as usize] != want
            {
                return Err(err(format!(
                    "pxf: {op}: binding at {} has no source span",
                    quote_string(field_path)
                )));
            }
            b.close_off = close as usize;
        }
        Ok(bs)
    }

    /// Stages an upsert of the field at `subpath` inside the element named
    /// `key` of the keyed collection at `field_path`: [`Rewriter::set`] for
    /// a keyed element, with its value-span and missing-chain behaviour
    /// inside the element. When no element is named `key`, a new one is
    /// appended to the collection's last binding, in its form: a `key {
    /// <chain> }` block, or a `{ <key_field> = "key" <chain> }` anonymous
    /// element. As with `set`, edits are computed against the document as
    /// parsed, so two `set_keyed`s that both create the same missing
    /// element create it twice.
    ///
    /// Setting the same `field_path`, `key` and `subpath` again replaces the
    /// edit staged for them.
    pub fn set_keyed(
        &mut self,
        field_path: &str,
        key: &str,
        subpath: &str,
        v: &Value,
    ) -> Result<(), PxfError> {
        let op = format!("SetKeyed {field_path}[{}].{subpath}", quote_string(key));
        if contains_bad_val(v) {
            return Err(err(format!("pxf: {op}: cannot write a BadVal placeholder")));
        }
        if key.is_empty() {
            return Err(err(format!("pxf: {op}: empty key")));
        }
        let doc = Arc::clone(&self.doc);
        let bs = self.keyed_bindings(&doc, &op, field_path)?;
        let path_key = format!("\x00keyed:set:{field_path}\x00{key}\x00{subpath}");
        let Some(elem) = find_keyed_elem(&bs, key) else {
            let chain = build_entry_chain(&op, subpath, v)?;
            let s = self.render_append_keyed_element(&bs[bs.len() - 1], key, chain);
            self.stage_splice(&path_key, s);
            return Ok(());
        };
        let t = resolve_in(elem.body, Some(elem.container()), subpath)?;
        self.set_resolved(&op, &path_key, &t, v)
    }

    /// The insertion that appends a new element with `key` and `body` to a
    /// binding, in the binding's form.
    fn render_append_keyed_element(
        &self,
        b: &KeyedBinding<'_>,
        key: &str,
        body: Vec<Entry>,
    ) -> ListSplice {
        if b.anon {
            return self.render_append_list_element(b, &new_anon_element(&b.key_field, key, body));
        }
        let (at, text) = self.render_append_entry(
            b.scope_start,
            b.close_off,
            b.entries,
            &Entry::Block(new_keyed_element_block(key, body)),
        );
        ListSplice {
            at,
            text,
            comma_at: None,
        }
    }

    /// Stages the removal of the field at `subpath` inside the element
    /// named `key`: [`Rewriter::remove`] for a keyed element, with its
    /// whole-line and trailing-comment handling.
    pub fn remove_keyed(
        &mut self,
        field_path: &str,
        key: &str,
        subpath: &str,
    ) -> Result<(), PxfError> {
        let op = format!("RemoveKeyed {field_path}[{}].{subpath}", quote_string(key));
        let doc = Arc::clone(&self.doc);
        let bs = self.keyed_bindings(&doc, &op, field_path)?;
        let Some(elem) = find_keyed_elem(&bs, key) else {
            return Err(err(format!("pxf: {op}: no element {}", quote_string(key))));
        };
        let t = resolve_in(elem.body, Some(elem.container()), subpath)?;
        let Some(e) = t.entry else {
            return Err(err(format!("pxf: {op}: no such entry")));
        };
        let (start, end, _) = self.entry_removal_span(e);
        self.stage(SpanEdit {
            path: format!("\x00keyed:rm:{field_path}\x00{key}\x00{subpath}"),
            start,
            end,
            text: Vec::new(),
        });
        Ok(())
    }

    /// Stages the removal of the whole element named `key` from the keyed
    /// collection at `field_path`. In the keyed-block form the element goes
    /// like any entry, with [`Rewriter::remove`]'s whole-line handling. In
    /// the anonymous list form the element, its separating comma and a
    /// trailing comment on its last line go, and the list stays
    /// well-formed. As with `remove`, comments on the lines around the
    /// element stay, and so does a trailing comma on the list.
    pub fn remove_keyed_element(&mut self, field_path: &str, key: &str) -> Result<(), PxfError> {
        let op = format!("RemoveKeyedElement {field_path}[{}]", quote_string(key));
        let doc = Arc::clone(&self.doc);
        let bs = self.keyed_bindings(&doc, &op, field_path)?;
        let Some(elem) = find_keyed_elem(&bs, key) else {
            return Err(err(format!("pxf: {op}: no element {}", quote_string(key))));
        };
        let path_key = format!("\x00keyed:rmel:{field_path}\x00{key}");
        let b = &bs[elem.b];
        if b.anon {
            let spans = self.list_element_removal_spans(b.list.unwrap(), elem.idx, false);
            self.stage_list_removal(&path_key, &spans);
            return Ok(());
        }
        let (start, end, _) = self.entry_removal_span(elem.node.unwrap());
        self.stage(SpanEdit {
            path: path_key,
            start,
            end,
            text: Vec::new(),
        });
        Ok(())
    }

    /// Stages the insertion of a new element named `key`, with `body`, into
    /// the keyed collection at `field_path`. Document order is list order,
    /// so placement matters. With `before` empty the element is appended to
    /// the last binding. Otherwise it goes just before the element named
    /// `before`, in whichever binding holds it and in that binding's form,
    /// above the anchor's own leading comments. Inserting a key that
    /// already names an element, in any binding, is an error: duplicate keys
    /// have no keyed-form representation.
    pub fn insert_keyed_element(
        &mut self,
        field_path: &str,
        key: &str,
        before: &str,
        body: &[Entry],
    ) -> Result<(), PxfError> {
        let op = format!("InsertKeyedElement {field_path}[{}]", quote_string(key));
        if key.is_empty() {
            return Err(err(format!("pxf: {op}: empty key")));
        }
        if body.iter().any(entry_contains_bad_val) {
            return Err(err(format!("pxf: {op}: cannot write a BadVal placeholder")));
        }
        let doc = Arc::clone(&self.doc);
        let bs = self.keyed_bindings(&doc, &op, field_path)?;
        if find_keyed_elem(&bs, key).is_some() {
            return Err(err(format!(
                "pxf: {op}: element {} already exists",
                quote_string(key)
            )));
        }
        let path_key = format!("\x00keyed:ins:{field_path}\x00{key}");

        if before.is_empty() {
            let s = self.render_append_keyed_element(&bs[bs.len() - 1], key, body.to_vec());
            self.stage_splice(&path_key, s);
            return Ok(());
        }
        let Some(anchor) = find_keyed_elem(&bs, before) else {
            return Err(err(format!(
                "pxf: {op}: no element {} to insert before",
                quote_string(before)
            )));
        };
        let ab = &bs[anchor.b];
        if ab.anon {
            let s = self.render_insert_list_element_before(
                ab,
                anchor.idx,
                &new_anon_element(&ab.key_field, key, body.to_vec()),
            );
            self.stage_splice(&path_key, s);
            return Ok(());
        }
        let (at, text) = self.render_insert_block_element_before(
            ab,
            anchor.node.unwrap(),
            &Entry::Block(new_keyed_element_block(key, body.to_vec())),
        );
        self.stage(SpanEdit {
            path: path_key,
            start: at,
            end: at,
            text,
        });
        Ok(())
    }

    /// The insertion that places `nb` just before `anchor` in a keyed-block
    /// binding: above the anchor's own leading comments when the anchor
    /// starts its line, or before it on the line otherwise.
    fn render_insert_block_element_before(
        &self,
        b: &KeyedBinding<'_>,
        anchor: &Entry,
        nb: &Entry,
    ) -> (usize, Vec<u8>) {
        let a_start = anchor.pos().offset;
        let ls = line_start_offset(&self.src, a_start);
        if is_blank(&self.src[ls..a_start]) {
            let indent = String::from_utf8_lossy(&self.src[ls..a_start]).into_owned();
            let at = self.extend_over_leading_comments(ls);
            let mut text = render_entry_lines(nb, &indent, &self.step_for(b.entries));
            text.push(b'\n');
            return (at, text);
        }
        let mut f = Formatter::new("  ");
        write_entry_inline(&mut f, nb);
        f.out.push(' ');
        (a_start, f.out.into_bytes())
    }

    /// Stages a retargeting of the element named `old_key` to `new_key`. In
    /// the keyed-block form only the name token is replaced, quoted or
    /// bare by the canonical rule (§3.13), and the element's body, comments
    /// and layout stay byte for byte. In the anonymous list form the key
    /// field's value is replaced instead. Renaming to a key that already
    /// names a sibling is an error; renaming an element to its own key
    /// stages nothing.
    pub fn rename_keyed_element(
        &mut self,
        field_path: &str,
        old_key: &str,
        new_key: &str,
    ) -> Result<(), PxfError> {
        let op = format!("RenameKeyedElement {field_path}[{}]", quote_string(old_key));
        if new_key.is_empty() {
            return Err(err(format!("pxf: {op}: empty new key")));
        }
        if new_key == old_key {
            return Ok(());
        }
        let doc = Arc::clone(&self.doc);
        let bs = self.keyed_bindings(&doc, &op, field_path)?;
        let Some(elem) = find_keyed_elem(&bs, old_key) else {
            return Err(err(format!(
                "pxf: {op}: no element {}",
                quote_string(old_key)
            )));
        };
        if find_keyed_elem(&bs, new_key).is_some() {
            return Err(err(format!(
                "pxf: {op}: element {} already exists",
                quote_string(new_key)
            )));
        }
        let path_key = format!("\x00keyed:ren:{field_path}\x00{old_key}");

        let b = &bs[elem.b];
        if b.anon {
            // The key is the key-field assignment's value, always a quoted
            // string in the anonymous form: replace that literal.
            let Some(ka) = anon_key_assignment(&elem.bv.unwrap().entries, &b.key_field) else {
                return Err(err(format!(
                    "pxf: {op}: element has no {} assignment",
                    quote_string(&b.key_field)
                )));
            };
            let (start, end) = (ka.value.pos().offset, ka.value.end().offset);
            self.stage(SpanEdit {
                path: path_key,
                start,
                end,
                text: quote_string(new_key).into_bytes(),
            });
            return Ok(());
        }

        let start = elem.node.unwrap().pos().offset;
        let Some(end) = name_token_end(&self.src, start) else {
            return Err(err(format!("pxf: {op}: element has no source span")));
        };
        let text = if ident_safe_entry_name(new_key) {
            new_key.to_string()
        } else {
            quote_string(new_key)
        };
        self.stage(SpanEdit {
            path: path_key,
            start,
            end,
            text: text.into_bytes(),
        });
        Ok(())
    }

    /// Stages a reorder of the element named `key` in the keyed collection
    /// at `field_path`: its source text moves verbatim. In the keyed-block
    /// form the `#` and `//` comment lines right above it move with it (a
    /// group header after a blank line stays). In the anonymous list form
    /// the element value moves, with a trailing comment on its line when
    /// the new place is a line of its own; a comment that cannot go there
    /// stays where it was. With `before` empty the element moves to the end
    /// of the collection; otherwise it lands just before the element named
    /// `before`. An element and its anchor must be in the same surface
    /// form. Moving an element before its current successor reproduces the
    /// input.
    pub fn move_keyed_element(
        &mut self,
        field_path: &str,
        key: &str,
        before: &str,
    ) -> Result<(), PxfError> {
        let op = format!("MoveKeyedElement {field_path}[{}]", quote_string(key));
        if before == key {
            return Err(err(format!(
                "pxf: {op}: cannot move an element before itself"
            )));
        }
        let doc = Arc::clone(&self.doc);
        let bs = self.keyed_bindings(&doc, &op, field_path)?;
        let Some(elem) = find_keyed_elem(&bs, key) else {
            return Err(err(format!("pxf: {op}: no element {}", quote_string(key))));
        };
        let mut anchor = None;
        if !before.is_empty() {
            let Some(a) = find_keyed_elem(&bs, before) else {
                return Err(err(format!(
                    "pxf: {op}: no element {} to move before",
                    quote_string(before)
                )));
            };
            if bs[a.b].anon != bs[elem.b].anon {
                return Err(err(format!(
                    "pxf: {op}: element {} and anchor {} are in different surface forms",
                    quote_string(key),
                    quote_string(before)
                )));
            }
            anchor = Some(a);
        }
        if bs[elem.b].anon {
            self.move_list_element(&bs, field_path, &elem, anchor.as_ref());
            return Ok(());
        }
        self.move_block_element(&bs, &op, field_path, &elem, anchor.as_ref())
    }

    /// Reorders a keyed-block-form element: its source, with its comment
    /// lines, is taken out and put back in at a line boundary.
    fn move_block_element(
        &mut self,
        bs: &[KeyedBinding<'_>],
        op: &str,
        field_path: &str,
        elem: &KeyedElem<'_>,
        anchor: Option<&KeyedElem<'_>>,
    ) -> Result<(), PxfError> {
        let (mut start, end, whole_lines) = self.entry_removal_span(elem.node.unwrap());
        if whole_lines {
            start = self.extend_over_leading_comments(start);
        }
        let mut text = self.src[start..end].to_vec();
        let del_key = format!("\x00keyed:mv:{field_path}\x00{}", elem.key);
        let ins_key = format!("{del_key}\x00ins");
        let eb = &bs[elem.b];

        if !whole_lines {
            if text.last() != Some(&b' ') {
                text.push(b' ');
            }
            let mut at = eb.close_off;
            if let Some(a) = anchor {
                at = a.node.unwrap().pos().offset;
            } else if at > 0 && self.src[at - 1] != b' ' && self.src[at - 1] != b'\t' {
                text.insert(0, b' ');
            }
            self.stage(SpanEdit {
                path: del_key,
                start,
                end,
                text: Vec::new(),
            });
            self.stage(SpanEdit {
                path: ins_key,
                start: at,
                end: at,
                text,
            });
            return Ok(());
        }

        if text.last() != Some(&b'\n') {
            text.push(b'\n');
        }
        let at;
        if let Some(a) = anchor {
            let a_start = a.node.unwrap().pos().offset;
            let ls = line_start_offset(&self.src, a_start);
            if !is_blank(&self.src[ls..a_start]) {
                return Err(err(format!(
                    "pxf: {op}: anchor element does not start its own line"
                )));
            }
            at = self.extend_over_leading_comments(ls);
        } else {
            let ls = line_start_offset(&self.src, eb.close_off);
            if is_blank(&self.src[ls..eb.close_off]) {
                at = ls;
            } else {
                at = eb.close_off;
                let body = text.strip_suffix(b"\n").unwrap_or(&text).to_vec();
                text = Vec::with_capacity(body.len() + 2);
                text.push(b'\n');
                text.extend_from_slice(&body);
                text.push(b'\n');
            }
        }
        self.stage(SpanEdit {
            path: del_key,
            start,
            end,
            text: Vec::new(),
        });
        self.stage(SpanEdit {
            path: ins_key,
            start: at,
            end: at,
            text,
        });
        Ok(())
    }

    /// Reorders an anonymous-form element: its value span is taken out and
    /// put back in before the anchor, or at the end of the list.
    fn move_list_element(
        &mut self,
        bs: &[KeyedBinding<'_>],
        field_path: &str,
        elem: &KeyedElem<'_>,
        anchor: Option<&KeyedElem<'_>>,
    ) {
        let eb = &bs[elem.b];
        let lv = eb.list.unwrap();
        // Already where it was asked to go: reproduce the input.
        if anchor.is_none() && elem.idx == lv.elements.len() - 1 {
            return;
        }
        if let Some(a) = anchor {
            if a.b == elem.b && a.idx == elem.idx + 1 {
                return;
            }
        }
        let bv = elem.bv.unwrap();
        let text = self.src[bv.pos.offset..bv.end.offset].to_vec();
        let del_key = format!("\x00keyed:mv:{field_path}\x00{}", elem.key);

        // A line comment would swallow what followed it on an inline
        // target, so the comment goes along only to a line of its own.
        let to = match anchor {
            Some(a) => &bs[a.b],
            None => eb,
        };
        let to_list = to.list.unwrap();
        let mut on_own_line = self.list_multiline(to_list);
        if let Some(a) = anchor {
            let p = to_list.elements[a.idx].pos().offset;
            on_own_line = on_own_line && is_blank(&self.src[line_start_offset(&self.src, p)..p]);
        }
        let mut comment = Vec::new();
        if on_own_line {
            let (cs, ce, _) = self.trailing_comment(self.list_elem_tail(lv, elem.idx));
            if cs >= 0 {
                comment = self.src[cs as usize..ce as usize].to_vec();
            }
        }

        let ins = match anchor {
            Some(a) => self.splice_list_element_before(to, a.idx, &text, &comment),
            None => self.splice_list_element_append(eb, &text, &comment),
        };
        let spans = self.list_element_removal_spans(lv, elem.idx, !on_own_line);
        self.stage_list_removal(&del_key, &spans);
        self.stage_splice(&format!("{del_key}\x00ins"), ins);
    }

    // --- Anonymous-list element surgery ---------------------------------

    /// Whether the list spans more than one line (the formatter's shape),
    /// rather than an inline `[ a, b ]`.
    fn list_multiline(&self, lv: &ListVal) -> bool {
        self.src[lv.pos.offset..lv.end.offset].contains(&b'\n')
    }

    /// The indent of the list's elements: the first element's, when it
    /// starts its line, else the list's line plus one step.
    fn list_element_indent(&self, lv: &ListVal, scope_start: usize) -> String {
        if let Some(first) = lv.elements.first() {
            let p = first.pos().offset;
            let ls = line_start_offset(&self.src, p);
            if is_blank(&self.src[ls..p]) {
                return String::from_utf8_lossy(&self.src[ls..p]).into_owned();
            }
        }
        self.line_leading_indent(scope_start) + &self.indent_step()
    }

    /// The offset of the first comma in `src[from..to]`, the gap between
    /// two list tokens, or -1. A comment in the gap is stepped over, since
    /// it may hold a comma of its own.
    fn list_comma(&self, from: usize, to: usize) -> isize {
        let src = &self.src;
        let mut i = from;
        while i < to {
            if src[i] == b',' {
                return i as isize;
            } else if src[i] == b'#' || src[i..to].starts_with(b"//") {
                while i < to && src[i] != b'\n' {
                    i += 1;
                }
            } else if src[i..to].starts_with(b"/*") {
                let Some(n) = super::find(&src[i..to], b"*/") else {
                    return -1;
                };
                i += n + 2;
            } else {
                i += 1;
            }
        }
        -1
    }

    /// The offset of the comma after element `idx` of `lv`, or -1 for a
    /// last element with no trailing comma.
    fn list_elem_comma(&self, lv: &ListVal, idx: usize) -> isize {
        let limit = if idx < lv.elements.len() - 1 {
            lv.elements[idx + 1].pos().offset
        } else {
            lv.end.offset - 1
        };
        self.list_comma(lv.elements[idx].end().offset, limit)
    }

    /// Where a trailing comment of element `idx` of `lv` would start: past
    /// its comma when that is on the element's last line, else just past
    /// the element.
    fn list_elem_tail(&self, lv: &ListVal, idx: usize) -> usize {
        let end = lv.elements[idx].end().offset;
        let c = self.list_elem_comma(lv, idx);
        if c >= 0 && !self.src[end..c as usize].contains(&b'\n') {
            return c as usize + 1;
        }
        end
    }

    /// The comment that ends the line at `off`: its span, or -1s when there
    /// is none, and the line's end, or -1 when something other than a
    /// comment follows `off` on the line. A block comment counts only when
    /// it closes on the line, as in [`Rewriter::entry_removal_span`].
    fn trailing_comment(&self, off: usize) -> (isize, isize, isize) {
        let src = &self.src;
        let mut i = off + self.trailing_list_pad(off);
        let (mut start, mut end) = (-1isize, -1isize);
        if i >= src.len() {
            // The end of input ends the line.
        } else if src[i] == b'#' || src[i..].starts_with(b"//") {
            start = i as isize;
            while i < src.len() && src[i] != b'\n' {
                i += 1;
            }
            end = i as isize;
        } else if src[i..].starts_with(b"/*") {
            if let Some(n) = super::find(&src[i..], b"*/") {
                if !src[i..i + n].contains(&b'\n') {
                    start = i as isize;
                    end = (i + n + 2) as isize;
                    i = end as usize + self.trailing_list_pad(end as usize);
                }
            }
        }
        if i >= src.len() || src[i] == b'\n' {
            return (start, end, i as isize);
        }
        (-1, -1, -1)
    }

    /// The spans to delete for element `idx` of `lv`: the element, the comma
    /// that separates it from a neighbour so the list stays well-formed,
    /// and a trailing comment on its last line unless `keep_comment`. They
    /// can be apart, with a comment on the element that stays between them,
    /// which is why this is more than one span.
    ///
    /// An element on lines of its own goes with those lines, so the lines
    /// around it survive. A last element with no trailing comma gives up
    /// the comma before it, which is then not its own: a list with a
    /// trailing comma keeps that shape.
    fn list_element_removal_spans(
        &self,
        lv: &ListVal,
        idx: usize,
        keep_comment: bool,
    ) -> Vec<(usize, usize)> {
        let src = &self.src;
        let elems = &lv.elements;
        let mut start = elems[idx].pos().offset;
        let end = elems[idx].end().offset;
        let comma = self.list_elem_comma(lv, idx);
        let tail = self.list_elem_tail(lv, idx);
        let (comment_start, _, mut line_end) = self.trailing_comment(tail);
        if keep_comment && comment_start >= 0 {
            line_end = -1;
        }

        let mut out = Vec::new();
        if comma >= tail as isize {
            // On a later line than the element.
            let c = comma as usize;
            out.push((c, c + 1 + self.trailing_list_pad(c + 1)));
        } else if comma < 0 && idx > 0 {
            let prev_end = elems[idx - 1].end().offset;
            let prev = self.list_comma(prev_end, start);
            if comment_start < 0 && prev >= 0 && !self.gap_has_comment(prev_end, start) {
                // Nothing to keep between the two elements: they close up.
                return vec![(prev_end, end + self.trailing_list_pad(end))];
            }
            if prev >= 0 {
                out.push((prev as usize, prev as usize + 1));
            }
        }

        let ls = line_start_offset(src, start);
        if line_end < 0 {
            // Shares its last line with what follows.
            out.push((start, tail + self.trailing_list_pad(tail)));
        } else if !is_blank(&src[ls..start]) {
            while start > ls && (src[start - 1] == b' ' || src[start - 1] == b'\t') {
                start -= 1;
            }
            out.push((start, line_end as usize));
        } else if (line_end as usize) < src.len() {
            out.push((ls, line_end as usize + 1));
        } else {
            out.push((ls, line_end as usize));
        }
        out
    }

    /// Whether `src[from..to]`, the gap between two list tokens, holds a
    /// comment.
    fn gap_has_comment(&self, from: usize, to: usize) -> bool {
        self.src[from..to]
            .iter()
            .any(|&c| !matches!(c, b' ' | b'\t' | b'\n' | b'\r' | b','))
    }

    /// Stages the deletion of `spans` under one identity.
    fn stage_list_removal(&mut self, path_key: &str, spans: &[(usize, usize)]) {
        for (i, &(start, end)) in spans.iter().enumerate() {
            self.stage(SpanEdit {
                path: format!("{path_key}\x00{i}"),
                start,
                end,
                text: Vec::new(),
            });
        }
    }

    /// Stages an insertion under one identity.
    fn stage_splice(&mut self, path_key: &str, s: ListSplice) {
        self.stage(SpanEdit {
            path: path_key.to_string(),
            start: s.at,
            end: s.at,
            text: s.text,
        });
        if let Some(c) = s.comma_at {
            self.stage(SpanEdit {
                path: format!("{path_key}\x00comma"),
                start: c,
                end: c,
                text: b",".to_vec(),
            });
        }
    }

    /// The count of spaces and tabs just after `off`, to tidy an inline
    /// last element's removal. Newlines are left for the bracket's line.
    fn trailing_list_pad(&self, off: usize) -> usize {
        self.src[off.min(self.src.len())..]
            .iter()
            .take_while(|&&c| c == b' ' || c == b'\t')
            .count()
    }

    /// The insertion that appends `bv` as the last element of the anonymous
    /// list of `b`.
    fn render_append_list_element(&self, b: &KeyedBinding<'_>, bv: &BlockVal) -> ListSplice {
        let text = self.render_list_element(b, bv);
        self.splice_list_element_append(b, &text, &[])
    }

    /// The insertion that places `bv` just before element `idx` of the
    /// anonymous list of `b`.
    fn render_insert_list_element_before(
        &self,
        b: &KeyedBinding<'_>,
        idx: usize,
        bv: &BlockVal,
    ) -> ListSplice {
        let text = self.render_list_element(b, bv);
        self.splice_list_element_before(b, idx, &text, &[])
    }

    /// `bv` as a list element, with no comma or newline around it, indented
    /// for the list of `b`.
    fn render_list_element(&self, b: &KeyedBinding<'_>, bv: &BlockVal) -> Vec<u8> {
        let indent = self.list_element_indent(b.list.unwrap(), b.scope_start);
        render_value(&Value::Block(bv.clone()), &indent, &self.indent_step())
    }

    /// The insertion that adds `elem_text` as the last element of the list
    /// of `b`, with `comment` after it on its line. It goes after the last
    /// element's line, so a comment there stays on that element, and after
    /// a trailing comma, which the new element then ends with too: the
    /// list keeps its shape.
    fn splice_list_element_append(
        &self,
        b: &KeyedBinding<'_>,
        elem_text: &[u8],
        comment: &[u8],
    ) -> ListSplice {
        let lv = b.list.unwrap();
        let indent = self.list_element_indent(lv, b.scope_start);
        let mut sb = Vec::new();
        if lv.elements.is_empty() {
            // An empty list: the element on its own line before the `]`.
            let base = self.line_leading_indent(b.scope_start);
            sb.push(b'\n');
            sb.extend_from_slice(indent.as_bytes());
            sb.extend_from_slice(elem_text);
            sb.push(b'\n');
            sb.extend_from_slice(base.as_bytes());
            return ListSplice {
                at: b.close_off,
                text: sb,
                comma_at: None,
            };
        }
        let last = lv.elements.len() - 1;
        let last_end = lv.elements[last].end().offset;
        let comma = self.list_elem_comma(lv, last);

        if !self.list_multiline(lv) {
            if comma >= 0 {
                sb.push(b' ');
                sb.extend_from_slice(elem_text);
                sb.push(b',');
                return ListSplice {
                    at: comma as usize + 1,
                    text: sb,
                    comma_at: None,
                };
            }
            sb.extend_from_slice(b", ");
            sb.extend_from_slice(elem_text);
            return ListSplice {
                at: last_end,
                text: sb,
                comma_at: None,
            };
        }

        let after = if comma >= 0 {
            comma as usize + 1
        } else {
            last_end
        };
        let (_, comment_end, _) = self.trailing_comment(after);
        let at = if comment_end >= 0 {
            comment_end as usize
        } else {
            after
        };
        let mut comma_at = None;
        if comma >= 0 {
        } else if comment_end >= 0 {
            comma_at = Some(last_end);
        } else {
            sb.push(b',');
        }
        sb.push(b'\n');
        sb.extend_from_slice(indent.as_bytes());
        sb.extend_from_slice(elem_text);
        if comma >= 0 {
            sb.push(b',');
        }
        if !comment.is_empty() {
            sb.push(b' ');
            sb.extend_from_slice(comment);
        }
        ListSplice {
            at,
            text: sb,
            comma_at,
        }
    }

    /// The insertion that places `elem_text` just before element `idx` of
    /// the list of `b`, with `comment` after it on its line. On a list of
    /// lines it goes above the target's own leading comments, which belong
    /// to the target.
    fn splice_list_element_before(
        &self,
        b: &KeyedBinding<'_>,
        idx: usize,
        elem_text: &[u8],
        comment: &[u8],
    ) -> ListSplice {
        let lv = b.list.unwrap();
        let indent = self.list_element_indent(lv, b.scope_start);
        let target = lv.elements[idx].pos().offset;
        let mut sb = Vec::new();
        if self.list_multiline(lv) {
            let ls = line_start_offset(&self.src, target);
            if is_blank(&self.src[ls..target]) {
                // At a line start: the new element takes the target's
                // indent, then the comma and newline, and the lines after
                // it stay as they were.
                sb.extend_from_slice(indent.as_bytes());
                sb.extend_from_slice(elem_text);
                sb.push(b',');
                if !comment.is_empty() {
                    sb.push(b' ');
                    sb.extend_from_slice(comment);
                }
                sb.push(b'\n');
                return ListSplice {
                    at: self.extend_over_leading_comments(ls),
                    text: sb,
                    comma_at: None,
                };
            }
        }
        sb.extend_from_slice(elem_text);
        sb.extend_from_slice(b", ");
        ListSplice {
            at: target,
            text: sb,
            comma_at: None,
        }
    }

    // --- shared helpers -------------------------------------------------

    /// Walks a line-start offset up over the `#` and `//` comment lines
    /// right above it, and returns the start of the topmost. A blank line,
    /// or any other line, stops the walk, so a comment block that a blank
    /// line separates from the entry (a group header) is not taken.
    fn extend_over_leading_comments(&self, mut start: usize) -> usize {
        while start > 0 {
            let ls = line_start_offset(&self.src, start - 1);
            // Go's bytes.TrimSpace: Unicode white space, as `str::trim`.
            let line = String::from_utf8_lossy(&self.src[ls..start - 1]);
            let line = line.trim();
            if line.is_empty() || !(line.starts_with('#') || line.starts_with("//")) {
                break;
            }
            start = ls;
        }
        start
    }
}

/// The element's key-field assignment (`key_field = "…"`), if it has one.
fn anon_key_assignment<'d>(entries: &'d [Entry], key_field: &str) -> Option<&'d Assignment> {
    entries.iter().find_map(|e| match e {
        Entry::Assignment(a)
            if !a.key_quoted && a.key == key_field && matches!(a.value, Value::String(_)) =>
        {
            Some(a)
        }
        _ => None,
    })
}

/// A dotted subpath and a leaf value as nested entries: `a.b.c` becomes
/// `a { b { c = v } }`. Every segment must be a bare identifier, since a
/// new element has no map context to justify a quoted key.
fn build_entry_chain(op: &str, subpath: &str, v: &Value) -> Result<Vec<Entry>, PxfError> {
    let Some(segs) = split_path_segs(subpath) else {
        return Err(err(format!(
            "pxf: {op}: invalid subpath {}: empty segment",
            quote_string(subpath)
        )));
    };
    for seg in &segs {
        if needs_quoting(seg) {
            return Err(err(format!(
                "pxf: {op}: segment {} is not a valid field name",
                quote_string(seg)
            )));
        }
    }
    let (leaf, blocks) = segs.split_last().unwrap();
    let mut e = Entry::Assignment(spanless_assignment(leaf, v.clone()));
    for seg in blocks.iter().rev() {
        e = Entry::Block(spanless_block(seg, false, vec![e]));
    }
    Ok(vec![e])
}

/// The offset just past the entry-name token at `start`: a string literal
/// (a quoted entry name) or an identifier. `None` when neither is there.
fn name_token_end(src: &[u8], start: usize) -> Option<usize> {
    if start >= src.len() {
        return None;
    }
    if src[start] == b'"' {
        return skip_dir_string(src, start);
    }
    let n = src[start..]
        .iter()
        .take_while(|&&c| is_ident_part(c))
        .count();
    (n > 0).then_some(start + n)
}

/// The offset just past the string literal at `i`, triple-quoted or not;
/// `None` when it does not close. protowire-go's `skipDirString`.
fn skip_dir_string(input: &[u8], i: usize) -> Option<usize> {
    if i + 2 < input.len() && input[i + 1] == b'"' && input[i + 2] == b'"' {
        let mut j = i + 3;
        while j + 2 < input.len() {
            if input[j] == b'"' && input[j + 1] == b'"' && input[j + 2] == b'"' {
                return Some(j + 3);
            }
            j += 1;
        }
        return None;
    }
    let mut j = i + 1;
    while j < input.len() {
        match input[j] {
            b'\\' => {
                if j + 1 >= input.len() {
                    return None;
                }
                j += 2;
            }
            b'"' => return Some(j + 1),
            b'\n' => return None,
            _ => j += 1,
        }
    }
    None
}

/// A dotted path's segments; `None` when it is empty or a segment is.
fn split_path_segs(path: &str) -> Option<Vec<&str>> {
    if path.is_empty() {
        return None;
    }
    let segs: Vec<&str> = path.split('.').collect();
    if segs.iter().any(|s| s.is_empty()) {
        return None;
    }
    Some(segs)
}
