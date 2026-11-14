// SPDX-License-Identifier: MIT
// Copyright (c) 2026 TrendVidia, LLC.
//! Format-preserving edits to a PXF document, ported line for line from
//! protowire-go's `encoding/pxf/rewrite.go` (#131). The keyed-collection
//! editors, from `rewrite_keyed.go`, are in the `keyed` submodule.

mod keyed;

use std::collections::HashMap;
use std::sync::Arc;

use crate::ast::{Block, Document, Entry, Value};
use crate::errors::PxfError;
use crate::format::{map_key_quoted, needs_quoting, quote_string, Formatter};
use crate::parser::parse;
use crate::token::Position;

/// Applies targeted, format-preserving edits to a PXF document, as
/// protowire-go's `Rewriter`. Unlike [`crate::format()`], which normalizes
/// the whole document, a `Rewriter` splices replacement bytes into the
/// original source. Everything outside the edited spans round-trips byte
/// for byte: comments, blank lines, key order, indentation, and how numbers
/// and strings were spelled. This is the write path for tools that
/// machine-edit documents a person also edits by hand: settings writers,
/// property panels, manifest updaters.
///
/// ```
/// use protowire_pxf::{parse, Entry, Rewriter};
///
/// let mut r = Rewriter::new("server {\n  port = 8080 # keep me\n  debug = true\n}\n")?;
/// let doc = parse("x = 9090")?;
/// let Entry::Assignment(a) = &doc.entries[0] else { unreachable!() };
/// r.set("server.port", &a.value)?;
/// r.remove("server.debug")?;
/// assert_eq!(r.bytes()?, b"server {\n  port = 9090 # keep me\n}\n");
/// # Ok::<(), protowire_pxf::PxfError>(())
/// ```
///
/// A field is addressed by a dotted path. Each segment names an
/// [`crate::Assignment`] key, a [`Block`] name or a [`crate::MapEntry`] key
/// in the enclosing scope, and the first match wins. A key that itself
/// holds a dot cannot be addressed. Edits are computed against the document
/// as it was parsed: staging an edit does not make new fields addressable
/// by later calls, and two `set`s that both create the same missing block
/// create it twice (stage one `set` with a [`crate::BlockVal`] value instead).
///
/// A keyed collection (draft -01 §3.13) has its own editors, which take the
/// element's key as one atom, since an entry name may hold dots and need not
/// be identifier-shaped: [`Rewriter::set_keyed`], [`Rewriter::remove_keyed`],
/// [`Rewriter::insert_keyed_element`], [`Rewriter::remove_keyed_element`],
/// [`Rewriter::rename_keyed_element`] and [`Rewriter::move_keyed_element`].
/// They search every binding of a field bound more than once (§3.13
/// concatenation). They cover the anonymous list form (`f = [ { k = "x" } ]`)
/// once its key field is named with [`Rewriter::keyed_by_field`]; the
/// keyed-block form needs no declaration.
///
/// [`Rewriter::document`] gives the parsed document as an `Arc`, so a
/// caller can hold one of its nodes while passing it to
/// [`Rewriter::replace_value`] or [`Rewriter::append_entry`].
#[derive(Debug, Clone)]
pub struct Rewriter {
    src: Vec<u8>,
    doc: Arc<Document>,
    edits: Vec<SpanEdit>,
    /// Gives each [`Rewriter::append_entry`] its own edit identity, so
    /// appends into one block accumulate, where `set`s of one path replace
    /// each other.
    insert_seq: usize,
    /// The `(pxf.key)` field declared for the keyed collection at a dotted
    /// path by [`Rewriter::keyed_by_field`]. It lets the keyed editors
    /// address a collection in the anonymous list form, where the key is
    /// the key field's value rather than an entry name.
    key_fields: HashMap<String, String>,
}

/// Replaces `src[start..end]` with `text`. `start == end` is a pure
/// insertion, and an empty `text` a pure deletion.
#[derive(Debug, Clone)]
struct SpanEdit {
    path: String,
    start: usize,
    end: usize,
    text: Vec<u8>,
}

/// A position-less error with protowire-go's text.
fn err(msg: String) -> PxfError {
    PxfError::new(Position::default(), msg)
}

impl Rewriter {
    /// Parses `src` with [`crate::parse`] and returns a `Rewriter` over it.
    /// The source must be valid PXF: a broken document has no stable spans
    /// to splice into. Use [`crate::parse_tolerant`] for read-side tooling
    /// on broken buffers.
    pub fn new(src: &str) -> Result<Self, PxfError> {
        let doc = parse(src)?;
        Ok(Self {
            src: src.as_bytes().to_vec(),
            doc: Arc::new(doc),
            edits: Vec::new(),
            insert_seq: 0,
            key_fields: HashMap::new(),
        })
    }

    /// The parsed document that paths are resolved against. It is the
    /// original source's, not the staged edits'.
    pub fn document(&self) -> Arc<Document> {
        Arc::clone(&self.doc)
    }

    /// Stages an upsert of the field at `path` to `v`. When the path
    /// resolves to an existing assignment or map entry, only that entry's
    /// value span is replaced: the key, separator, alignment and any
    /// trailing comment stay. When the path, or a tail of it, does not
    /// exist, the missing chain is inserted at the end of the deepest
    /// existing block, indented like its siblings.
    ///
    /// Setting a path that resolves to a [`Block`] is an error, since
    /// replacing a block whole would drop the comments inside it. Set its
    /// fields one by one, or remove it and set a [`crate::BlockVal`].
    ///
    /// A `Rewriter` works on syntax alone and has no schema. An inserted
    /// leaf uses the `:` map form when a sibling is a map entry and the `=`
    /// field form otherwise, so the first entry inserted into an empty map
    /// block is written with `=`. Seed an empty map block with a
    /// [`crate::BlockVal`] of map entries instead.
    ///
    /// Setting the same path again replaces the edit staged for it.
    pub fn set(&mut self, path: &str, v: &Value) -> Result<(), PxfError> {
        if contains_bad_val(v) {
            return Err(err(format!(
                "pxf: Set {path}: cannot write a BadVal placeholder"
            )));
        }
        let doc = Arc::clone(&self.doc);
        let t = resolve_in(&doc.entries, None, path)?;
        self.set_resolved(&format!("Set {path}"), path, &t, v)
    }

    /// Stages the value edit for a resolved target: the existing entry's
    /// value span replaced, or the missing chain inserted. `op` prefixes
    /// error messages (`Set <path>`); `path_key` is the edit's identity, and
    /// edits with the same one replace each other.
    pub(crate) fn set_resolved(
        &mut self,
        op: &str,
        path_key: &str,
        t: &Target<'_>,
        v: &Value,
    ) -> Result<(), PxfError> {
        if let Some(e) = t.entry {
            let Some(old) = entry_value(e) else {
                return Err(err(format!(
                    "pxf: {op}: path addresses a block; set its fields individually, or Remove it and Set a BlockVal"
                )));
            };
            let (start, end) = (old.pos().offset, old.end().offset);
            let indent = self.line_indent_at(e.pos().offset);
            let step = self.step_for(t.container_entries);
            let text = render_value(v, &indent, &step);
            self.stage(SpanEdit {
                path: path_key.to_string(),
                start,
                end,
                text,
            });
            return Ok(());
        }
        self.stage_insert(op, path_key, t, v)
    }

    /// Stages the removal of the entry at `path`. When the entry stands
    /// alone on its lines, the whole lines go, a trailing comment on its
    /// last line included. The comment lines above it stay, since they may
    /// describe the group around it. When the entry shares a line with
    /// something else, only its span goes. Removing a path that does not
    /// exist is an error.
    pub fn remove(&mut self, path: &str) -> Result<(), PxfError> {
        let doc = Arc::clone(&self.doc);
        let t = resolve_in(&doc.entries, None, path)?;
        let Some(e) = t.entry else {
            return Err(err(format!("pxf: Remove {path}: no such entry")));
        };
        let (start, end, _) = self.entry_removal_span(e);
        self.stage(SpanEdit {
            path: path.to_string(),
            start,
            end,
            text: Vec::new(),
        });
        Ok(())
    }

    /// The span removing `e` deletes: when the entry stands alone on its
    /// lines, the whole lines with a trailing comment on the last one
    /// (`true`); otherwise its span and the spaces after it.
    pub(crate) fn entry_removal_span(&self, e: &Entry) -> (usize, usize, bool) {
        let src = &self.src;
        let start = e.pos().offset;
        let mut end = e.end().offset;

        let ls = line_start_offset(src, start);
        if is_blank(&src[ls..start]) {
            // The entry starts its line: try to take the rest of the line.
            let mut le = end;
            while le < src.len() && (src[le] == b' ' || src[le] == b'\t') {
                le += 1;
            }
            if le < src.len()
                && (src[le] == b'#'
                    || (src[le] == b'/' && le + 1 < src.len() && src[le + 1] == b'/'))
            {
                while le < src.len() && src[le] != b'\n' {
                    le += 1;
                }
            } else if le + 1 < src.len() && src[le] == b'/' && src[le + 1] == b'*' {
                // A trailing block comment is taken only when it closes on
                // the same line. A multi-line /* ... */ is not the entry's
                // own trailing comment, so only the span is removed.
                if let Some(close) = find(&src[le..], b"*/") {
                    let through = le + close + 2;
                    if !src[le..through].contains(&b'\n') {
                        le = through;
                        while le < src.len() && (src[le] == b' ' || src[le] == b'\t') {
                            le += 1;
                        }
                    }
                }
            }
            if le >= src.len() || src[le] == b'\n' {
                if le < src.len() {
                    le += 1; // the newline
                }
                return (ls, le, true);
            }
        }
        // The entry shares its lines with something else: remove its span
        // and the spaces between it and what follows.
        while end < src.len() && (src[end] == b' ' || src[end] == b'\t') {
            end += 1;
        }
        (start, end, false)
    }

    /// Stages the replacement of an existing value's source span with the
    /// text of `with`. [`Rewriter::set`] finds a value by its first-match
    /// path; `replace_value` edits the very node the caller holds from
    /// [`Rewriter::document`], so it can change one of several siblings
    /// with the same key, which no path singles out.
    ///
    /// `old` must be a value parsed from this `Rewriter`'s source, which
    /// carries the span to replace. A value built by hand or a
    /// [`crate::BadVal`] has none and is refused. A multi-line
    /// replacement's later lines are indented to `old`'s line, as `set`
    /// does. Replacing the same node again replaces the staged edit.
    pub fn replace_value(&mut self, old: &Value, with: &Value) -> Result<(), PxfError> {
        if contains_bad_val(with) {
            return Err(err(
                "pxf: ReplaceValue: cannot write a BadVal placeholder".to_string()
            ));
        }
        let (start, end) = (old.pos().offset, old.end().offset);
        if start >= end {
            return Err(err(
                "pxf: ReplaceValue: old value has no source span (not a parsed value, or a BadVal)"
                    .to_string(),
            ));
        }
        let text = render_value(with, &self.line_leading_indent(start), &self.indent_step());
        self.set_span(start, end, &text)
    }

    /// Stages the replacement of `src[start..end]` with `text`: the raw
    /// edit beneath [`Rewriter::replace_value`] and [`Rewriter::set`].
    /// `start == end` is a pure insertion, and an empty `text` a pure
    /// deletion. An edit staged here still gets the overlap check and the
    /// reparse in [`Rewriter::bytes`]. Staging another edit over the same
    /// span replaces this one.
    pub fn set_span(&mut self, start: usize, end: usize, text: &[u8]) -> Result<(), PxfError> {
        if end < start || end > self.src.len() {
            return Err(err(format!(
                "pxf: SetSpan: span [{start}:{end}) out of range for source of length {}",
                self.src.len()
            )));
        }
        self.stage(SpanEdit {
            path: span_path(start, end),
            start,
            end,
            text: text.to_vec(),
        });
        Ok(())
    }

    /// Stages the insertion of `e` as the last entry of `block`, formatted
    /// like the block's entries. A multi-line block gets a new line before
    /// its `}`, indented like its entries (or one level in from the block
    /// when it has none). An inline block (`foo { ... }`, `foo { }`
    /// included) gets the entry before its `}` on the same line. `block`
    /// must be a node of this `Rewriter`'s [`Rewriter::document`]. Appends
    /// into one block accumulate in call order.
    pub fn append_entry(&mut self, block: &Block, e: &Entry) -> Result<(), PxfError> {
        if entry_contains_bad_val(e) {
            return Err(err(
                "pxf: AppendEntry: cannot write a BadVal placeholder".to_string()
            ));
        }
        let scope_start = block.pos.offset;
        let close_off = match block.end.offset.checked_sub(1) {
            Some(c) if c >= scope_start && c < self.src.len() && self.src[c] == b'}' => c,
            _ => {
                return Err(err(format!(
                    "pxf: AppendEntry: block {} has no source span (not a parsed block)",
                    quote_string(&block.name)
                )))
            }
        };
        let (insert_at, text) = self.render_append_entry(scope_start, close_off, &block.entries, e);
        self.stage_insert_edit(insert_at, text);
        Ok(())
    }

    /// The insertion that appends `e` as the last entry of the scope whose
    /// entry starts at `scope_start` and whose `}` is at `close_off`,
    /// formatted like its entries. [`Rewriter::append_entry`] and the keyed
    /// editors stage it under their own edit identities.
    pub(crate) fn render_append_entry(
        &self,
        scope_start: usize,
        close_off: usize,
        siblings: &[Entry],
        e: &Entry,
    ) -> (usize, Vec<u8>) {
        let src = &self.src;
        if !src[scope_start..close_off].contains(&b'\n') {
            // Inline block `{ ... }` (or `{}`): ` <entry> ` before the `}`.
            let mut f = Formatter::new("  ");
            if close_off > 0 && src[close_off - 1] != b' ' && src[close_off - 1] != b'\t' {
                f.out.push(' ');
            }
            write_entry_inline(&mut f, e);
            f.out.push(' ');
            return (close_off, f.out.into_bytes());
        }

        // Multi-line block: a whole line just above the `}`, indented like
        // the entries. A nested body steps in by the document's own indent
        // width, not a fixed two spaces.
        let step = self.step_for(siblings);
        let indent = if siblings.is_empty() {
            // No sibling shows the indent: the block's own line plus one
            // step, rather than child_indent's two-space guess.
            self.line_leading_indent(scope_start) + &step
        } else {
            self.child_indent(siblings, Some(scope_start))
        };
        let mut insert_at = close_off;
        let mut out = Vec::new();
        let ls = line_start_offset(src, close_off);
        if is_blank(&src[ls..close_off]) {
            insert_at = ls;
        } else {
            // The `}` shares a line with the last entry: break the line.
            out.push(b'\n');
        }
        out.extend_from_slice(&render_entry_lines(e, &indent, &step));
        out.push(b'\n');
        (insert_at, out)
    }

    /// Stages a zero-width insertion of `text` at `off` under an identity
    /// of its own, so insertions at one offset are all kept, in call order.
    pub(crate) fn stage_insert_edit(&mut self, off: usize, text: Vec<u8>) {
        self.insert_seq += 1;
        self.stage(SpanEdit {
            path: format!("\x00insert#{}", self.insert_seq),
            start: off,
            end: off,
            text,
        });
    }

    /// Applies the staged edits and returns the rewritten document. Edits
    /// must not overlap (a `remove` of a block and a `set` inside it, say);
    /// overlapping edits are an error. As a safety net for writer bugs, the
    /// result is parsed again before it is returned, and a rewrite that
    /// gives invalid PXF is an error, with the parse error as its source.
    /// The original source is never changed.
    pub fn bytes(&self) -> Result<Vec<u8>, PxfError> {
        if self.edits.is_empty() {
            return Ok(self.src.clone());
        }
        let mut edits: Vec<&SpanEdit> = self.edits.iter().collect();
        edits.sort_by_key(|e| e.start);
        for w in edits.windows(2) {
            if w[0].end > w[1].start {
                return Err(err(format!(
                    "pxf: conflicting edits: {} overlaps {}",
                    w[0].path, w[1].path
                )));
            }
        }
        let mut out = Vec::with_capacity(self.src.len() + 64);
        let mut last = 0;
        for e in &edits {
            out.extend_from_slice(&self.src[last..e.start]);
            out.extend_from_slice(&e.text);
            last = e.end;
        }
        out.extend_from_slice(&self.src[last..]);
        let reparsed = match std::str::from_utf8(&out) {
            Ok(text) => parse(text).map(|_| ()),
            Err(e) => Err(PxfError::new(
                Position::default(),
                format!("invalid UTF-8 at byte {}", e.valid_up_to()),
            )),
        };
        if let Err(e) = reparsed {
            return Err(PxfError::with_source(
                Position::default(),
                format!("pxf: rewrite produced an invalid document: {e}"),
                Arc::new(e),
            ));
        }
        Ok(out)
    }

    /// Records an edit, replacing one staged for the same path.
    fn stage(&mut self, e: SpanEdit) {
        if let Some(old) = self.edits.iter_mut().find(|old| old.path == e.path) {
            *old = e;
            return;
        }
        self.edits.push(e);
    }

    /// Stages the insertion of the missing tail of a path, at the end of
    /// the deepest existing scope. `op` prefixes error messages; `path_key`
    /// is the edit's identity.
    pub(crate) fn stage_insert(
        &mut self,
        op: &str,
        path_key: &str,
        t: &Target<'_>,
        v: &Value,
    ) -> Result<(), PxfError> {
        // The segments that create blocks must be bare identifiers; only
        // the leaf may be quoted, and only in the `:` map form.
        let (leaf, blocks) = t.missing.split_last().expect("a target misses a segment");
        for seg in blocks {
            if needs_quoting(seg) {
                return Err(err(format!(
                    "pxf: {op}: segment {} is not a valid block name",
                    quote_string(seg)
                )));
            }
        }
        let map_form = has_map_entry(t.container_entries);
        if needs_quoting(leaf) && !map_form {
            return Err(err(format!(
                "pxf: {op}: key {} needs a map (':') context",
                quote_string(leaf)
            )));
        }
        let step = self.step_for(t.container_entries);

        // The scope to insert into: the container block, up to its `}`, or
        // the end of the document at the top level.
        let (close_off, scope_start) = match t.container {
            None => (self.src.len() as isize, None),
            Some(c) => (c.close_off, Some(c.pos)),
        };
        if scope_start.is_some()
            && (close_off < 0
                || close_off as usize >= self.src.len()
                || self.src[close_off as usize] != b'}')
        {
            return Err(err(format!(
                "pxf: {op}: internal error: container span does not end at '}}'"
            )));
        }
        let close_off = close_off as usize;

        if let Some(scope_start) = scope_start {
            if !self.src[scope_start..close_off].contains(&b'\n') {
                // Inline block `{ ... }` (or `{}`): ` key = value ` just
                // before the `}`.
                let mut out = Vec::new();
                if close_off > 0
                    && self.src[close_off - 1] != b' '
                    && self.src[close_off - 1] != b'\t'
                {
                    out.push(b' ');
                }
                out.extend_from_slice(&render_chain(&t.missing, "", &step, map_form, v, true));
                out.push(b' ');
                self.stage(SpanEdit {
                    path: path_key.to_string(),
                    start: close_off,
                    end: close_off,
                    text: out,
                });
                return Ok(());
            }
        }

        // Multi-line scope: whole lines just above the `}` (at the top
        // level, at the end of input), indented like the siblings.
        let indent = self.sibling_indent(t);
        let mut insert_at = close_off;
        let mut out = Vec::new();
        if scope_start.is_some() {
            let ls = line_start_offset(&self.src, close_off);
            if is_blank(&self.src[ls..close_off]) {
                insert_at = ls;
            } else {
                // The `}` shares a line with the last entry: break the line.
                out.push(b'\n');
            }
        } else if self.src.last().is_some_and(|&c| c != b'\n') {
            out.push(b'\n');
        }
        out.extend_from_slice(&render_chain(
            &t.missing, &indent, &step, map_form, v, false,
        ));
        out.push(b'\n');
        self.stage(SpanEdit {
            path: path_key.to_string(),
            start: insert_at,
            end: insert_at,
            text: out,
        });
        Ok(())
    }

    /// The indent of a line inserted into the target's container: that of
    /// the container's first entry when it starts its line, else the
    /// container's own indent plus one level (none at the top level).
    fn sibling_indent(&self, t: &Target<'_>) -> String {
        self.child_indent(t.container_entries, t.container.map(|c| c.pos))
    }

    /// The indent for a new child of a block: that of the first child that
    /// starts its line, else the block's own indent plus one level.
    /// `container_pos` is `None` at the top level, which is not indented.
    pub(crate) fn child_indent(&self, entries: &[Entry], container_pos: Option<usize>) -> String {
        if let Some(first) = entries.first() {
            let first = first.pos().offset;
            let ls = line_start_offset(&self.src, first);
            if is_blank(&self.src[ls..first]) {
                return ascii(&self.src[ls..first]);
            }
        }
        match container_pos {
            None => String::new(),
            Some(p) => self.line_indent_at(p) + "  ",
        }
    }

    /// The indent step for content inserted among `entries`: a nested
    /// block's among them first (a sibling's body is the closest
    /// reference), then the document's, then two spaces.
    pub(crate) fn step_for(&self, entries: &[Entry]) -> String {
        self.scan_indent_step(entries)
            .unwrap_or_else(|| self.indent_step())
    }

    /// The document's indent step, from a scan of the whole document, or
    /// two spaces when nothing shows it.
    pub(crate) fn indent_step(&self) -> String {
        self.scan_indent_step(&self.doc.entries)
            .unwrap_or_else(|| "  ".to_string())
    }

    /// The whitespace that the first own-line child of the first nested
    /// block (or block value) in `entries` adds over that block's line:
    /// the document's indent step. `None` when no nested block on lines of
    /// its own shows one.
    fn scan_indent_step(&self, entries: &[Entry]) -> Option<String> {
        for e in entries {
            let kids: &[Entry] = match e {
                Entry::Block(b) => &b.entries,
                _ => match entry_value(e) {
                    Some(Value::Block(bv)) => &bv.entries,
                    _ => &[],
                },
            };
            if kids.is_empty() {
                continue;
            }
            let base = self.line_leading_indent(e.pos().offset);
            let first = kids[0].pos().offset;
            let ls = line_start_offset(&self.src, first);
            if is_blank(&self.src[ls..first]) {
                let ci = ascii(&self.src[ls..first]);
                if ci.len() > base.len() && ci.starts_with(&base) {
                    return Some(ci[base.len()..].to_string());
                }
            }
            if let Some(s) = self.scan_indent_step(kids) {
                return Some(s);
            }
        }
        None
    }

    /// The whitespace before `off` on its line, or empty when anything
    /// else precedes `off` there.
    pub(crate) fn line_indent_at(&self, off: usize) -> String {
        let ls = line_start_offset(&self.src, off);
        if is_blank(&self.src[ls..off]) {
            return ascii(&self.src[ls..off]);
        }
        String::new()
    }

    /// The leading whitespace of the line holding `off`, whatever else
    /// precedes `off` there. Aligns the later lines of a value spliced in
    /// after its key.
    pub(crate) fn line_leading_indent(&self, off: usize) -> String {
        let ls = line_start_offset(&self.src, off);
        let mut i = ls;
        while i < off && (self.src[i] == b' ' || self.src[i] == b'\t') {
            i += 1;
        }
        ascii(&self.src[ls..i])
    }
}

/// The end of a block value, as a signed offset so a missing one is below
/// zero.
fn block_val_end(v: &Value) -> isize {
    match v {
        Value::Block(bv) => bv.end.offset as isize,
        _ => -1,
    }
}

/// Spaces and tabs, as text.
fn ascii(b: &[u8]) -> String {
    String::from_utf8_lossy(b).into_owned()
}

/// The identity of an edit staged by span. The NUL keeps it apart from
/// every dotted path.
pub(crate) fn span_path(start: usize, end: usize) -> String {
    format!("\x00span:{start}:{end}")
}

/// The scope a path resolved into, as the two offsets an insertion needs:
/// where the entry owning it starts, and where its `}` is (negative when it
/// has none). protowire-go keeps the entry, and builds a synthetic one over
/// an anonymous list element's block value; this port keeps the offsets.
#[derive(Debug, Clone, Copy)]
pub(crate) struct Container {
    pub(crate) pos: usize,
    pub(crate) close_off: isize,
}

impl Container {
    /// The scope of a block, or of an assignment or map entry holding a
    /// block value.
    pub(crate) fn of(e: &Entry) -> Self {
        let close = match e {
            Entry::Block(b) => b.end.offset as isize - 1,
            Entry::Assignment(a) => block_val_end(&a.value) - 1,
            Entry::MapEntry(m) => block_val_end(&m.value) - 1,
            Entry::TypeEntry(_) => -1,
        };
        Self {
            pos: e.pos().offset,
            close_off: close,
        }
    }
}

/// What a dotted path resolves to: an entry, or the deepest existing scope
/// and the segments missing below it.
pub(crate) struct Target<'d> {
    /// The entry, when the whole path resolved.
    pub(crate) entry: Option<&'d Entry>,
    /// The innermost resolved ancestor: a [`Block`], or an assignment or
    /// map entry holding a [`crate::BlockVal`]. `None` is the top level.
    pub(crate) container: Option<Container>,
    /// The entries the last lookup ran in.
    pub(crate) container_entries: &'d [Entry],
    /// The segments that did not resolve; empty when `entry` is set.
    pub(crate) missing: Vec<String>,
}

/// Resolves a dotted path in a scope: the top level, or one keyed
/// element's body for the keyed editors. `container` is the entry that
/// owns the scope (`None` at the top level).
pub(crate) fn resolve_in<'d>(
    mut entries: &'d [Entry],
    mut container: Option<Container>,
    path: &str,
) -> Result<Target<'d>, PxfError> {
    let segs: Vec<&str> = path.split('.').collect();
    if segs.iter().any(|s| s.is_empty()) {
        return Err(err(format!(
            "pxf: invalid path {}: empty segment",
            quote_string(path)
        )));
    }
    for (i, seg) in segs.iter().enumerate() {
        let Some(e) = find_entry(entries, seg) else {
            return Ok(Target {
                entry: None,
                container,
                container_entries: entries,
                missing: segs[i..].iter().map(|s| s.to_string()).collect(),
            });
        };
        if i == segs.len() - 1 {
            return Ok(Target {
                entry: Some(e),
                container,
                container_entries: entries,
                missing: Vec::new(),
            });
        }
        entries = match e {
            Entry::Block(b) => &b.entries,
            _ => match entry_value(e) {
                Some(Value::Block(bv)) => &bv.entries,
                _ => {
                    return Err(err(format!(
                        "pxf: path {}: segment {} is a scalar field, not a block",
                        quote_string(path),
                        quote_string(seg)
                    )))
                }
            },
        };
        container = Some(Container::of(e));
    }
    unreachable!("a path has at least one segment")
}

/// The key of an assignment or map entry, or the name of a block, that a
/// path segment names.
pub(crate) fn entry_key(e: &Entry) -> &str {
    match e {
        Entry::Assignment(a) => &a.key,
        Entry::MapEntry(m) => &m.key,
        Entry::Block(b) => &b.name,
        Entry::TypeEntry(_) => "",
    }
}

/// The value of a leaf entry; `None` for a [`Block`], which has entries.
pub(crate) fn entry_value(e: &Entry) -> Option<&Value> {
    match e {
        Entry::Assignment(a) => Some(&a.value),
        Entry::MapEntry(m) => Some(&m.value),
        _ => None,
    }
}

/// The first of `entries` whose key or block name is `key`.
pub(crate) fn find_entry<'d>(entries: &'d [Entry], key: &str) -> Option<&'d Entry> {
    entries.iter().find(|e| entry_key(e) == key)
}

/// Whether `v` is or holds a placeholder anywhere.
pub(crate) fn contains_bad_val(v: &Value) -> bool {
    match v {
        Value::Bad(_) => true,
        Value::List(l) => l.elements.iter().any(contains_bad_val),
        Value::Block(b) => b.entries.iter().any(entry_contains_bad_val),
        _ => false,
    }
}

/// Whether `e` holds a placeholder anywhere, nested blocks included.
pub(crate) fn entry_contains_bad_val(e: &Entry) -> bool {
    match e {
        Entry::Assignment(a) => contains_bad_val(&a.value),
        Entry::MapEntry(m) => contains_bad_val(&m.value),
        Entry::Block(b) => b.entries.iter().any(entry_contains_bad_val),
        Entry::TypeEntry(_) => false,
    }
}

/// The missing tail of a path: a block opened for each segment but the
/// last, then `leaf = value` (or `leaf: value` in the map form).
/// `base_indent` is the first line's indent, and each nested line adds one
/// `step`. Inline, everything is on one line and neither is used.
pub(crate) fn render_chain(
    missing: &[String],
    base_indent: &str,
    step: &str,
    map_form: bool,
    v: &Value,
    inline: bool,
) -> Vec<u8> {
    let mut sb = String::new();
    // Only the leaf's own container decides `=` or `:`; a block made here
    // is a message, so its leaf uses `=`.
    let leaf_map = map_form && missing.len() == 1;
    let (leaf, blocks) = missing.split_last().expect("a chain has a leaf");
    for (i, seg) in blocks.iter().enumerate() {
        if !inline {
            sb.push_str(&indent_at(base_indent, step, i));
        }
        sb.push_str(seg);
        sb.push_str(if inline { " { " } else { " {\n" });
    }
    let depth = missing.len() - 1;
    if !inline {
        sb.push_str(&indent_at(base_indent, step, depth));
    }
    if leaf_map && needs_quoting(leaf) {
        sb.push_str(&quote_string(leaf));
    } else {
        sb.push_str(leaf);
    }
    sb.push_str(if leaf_map { ": " } else { " = " });
    let mut out = sb.into_bytes();
    out.extend_from_slice(&render_value(v, &indent_at(base_indent, step, depth), step));
    for i in (0..depth).rev() {
        if inline {
            out.extend_from_slice(b" }");
        } else {
            out.push(b'\n');
            out.extend_from_slice(indent_at(base_indent, step, i).as_bytes());
            out.push(b'}');
        }
    }
    out
}

/// `base` plus `depth` levels of `step`.
fn indent_at(base: &str, step: &str, depth: usize) -> String {
    let mut s = String::from(base);
    for _ in 0..depth {
        s.push_str(step);
    }
    s
}

/// `v` as the formatter writes it, with its later lines prefixed by
/// `line_prefix`, so a multi-line value lines up under the entry it is
/// spliced into. A nested body steps in by `step` a level, the document's
/// indent rather than a fixed two spaces.
pub(crate) fn render_value(v: &Value, line_prefix: &str, step: &str) -> Vec<u8> {
    let mut f = Formatter::new(step);
    f.format_value(v, 0);
    let mut out = f.out;
    if !line_prefix.is_empty() && out.contains('\n') {
        out = out.replace('\n', &format!("\n{line_prefix}"));
    }
    out.into_bytes()
}

/// One entry for a multi-line block: as the formatter writes it, nested
/// blocks indented by `step` a level, each line prefixed by
/// `base_indent`, and no final newline (the caller writes the breaks).
pub(crate) fn render_entry_lines(e: &Entry, base_indent: &str, step: &str) -> Vec<u8> {
    let mut f = Formatter::new(step);
    f.format_entries(std::slice::from_ref(e), 0);
    let out = f.out.strip_suffix('\n').unwrap_or(&f.out);
    if base_indent.is_empty() {
        return out.as_bytes().to_vec();
    }
    format!(
        "{base_indent}{}",
        out.replace('\n', &format!("\n{base_indent}"))
    )
    .into_bytes()
}

/// One entry on one line, for an inline block: `a = 1`, `"k": "v"`,
/// `n { a = 1 }`.
pub(crate) fn write_entry_inline(f: &mut Formatter<'_>, e: &Entry) {
    match e {
        Entry::Assignment(a) => {
            if a.key_quoted {
                f.out.push_str(&quote_string(&a.key));
            } else {
                f.out.push_str(&a.key);
            }
            f.out.push_str(" = ");
            f.format_value(&a.value, 0);
        }
        Entry::MapEntry(m) => {
            if map_key_quoted(m) {
                f.out.push_str(&quote_string(&m.key));
            } else {
                f.out.push_str(&m.key);
            }
            f.out.push_str(": ");
            f.format_value(&m.value, 0);
        }
        Entry::TypeEntry(t) => {
            f.out.push_str("@type = ");
            f.out.push_str(&quote_string(&t.url));
        }
        Entry::Block(b) => {
            if b.name_quoted {
                f.out.push_str(&quote_string(&b.name));
            } else {
                f.out.push_str(&b.name);
            }
            f.out.push_str(" {");
            for c in &b.entries {
                f.out.push(' ');
                write_entry_inline(f, c);
            }
            f.out.push_str(" }");
        }
    }
}

/// Whether any of `entries` is a `:` map entry, which decides the
/// separator of an inserted leaf.
pub(crate) fn has_map_entry(entries: &[Entry]) -> bool {
    entries.iter().any(|e| matches!(e, Entry::MapEntry(_)))
}

/// The offset of the first byte of the line holding `off`.
pub(crate) fn line_start_offset(src: &[u8], off: usize) -> usize {
    let mut i = off;
    while i > 0 && src[i - 1] != b'\n' {
        i -= 1;
    }
    i
}

/// Whether `b` is all spaces and tabs.
pub(crate) fn is_blank(b: &[u8]) -> bool {
    b.iter().all(|&c| c == b' ' || c == b'\t')
}

/// The offset of the first `needle` in `hay`.
pub(crate) fn find(hay: &[u8], needle: &[u8]) -> Option<usize> {
    hay.windows(needle.len()).position(|w| w == needle)
}
