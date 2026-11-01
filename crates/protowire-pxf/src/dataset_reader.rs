// SPDX-License-Identifier: MIT
// Copyright (c) 2026 TrendVidia, LLC.
//! Streaming consumption for the `@dataset` directive (draft §3.4.4).
//!
//! [`crate::unmarshal_full`] materializes every row of an `@dataset`
//! directive into [`crate::Presence::datasets`]. That works for small
//! datasets and breaks for the CSV-replacement workload `@dataset` was
//! designed for. [`DatasetReader`] pulls one row at a time from an
//! [`io::Read`] source; per-row arity and the v1 cell-grammar rule are
//! enforced at consume time (not deferred to end-of-input), and rows
//! are yielded in source order — both invariants the spec requires of
//! streaming consumers.
//!
//! Convenience: [`DatasetReader::scan`] reads the next row and binds its
//! cells to a fresh [`prost_reflect::DynamicMessage`]. [`bind_row`] is
//! exported for callers iterating
//! [`crate::Presence::datasets`]\[i\].rows from the materializing path.
//!
//! Mirrors the cpp port at `protowire-cpp/src/pxf/dataset_reader.cc`.

use std::collections::HashMap;
use std::io::{self, Read};

use prost_reflect::{DynamicMessage, MessageDescriptor};

use crate::ast::{DatasetRow, Directive, Value};
use crate::dataset_paths::check_column_paths;
use crate::errors::PxfError;
use crate::format::format_value_text;
use crate::limits::Limits;
use crate::parser::{parse_row, parse_with_limits};
use crate::token::Position;
use crate::{unmarshal_full, UnmarshalOptions};

/// Default cap on the @dataset header (leading directives plus the
/// `@dataset TYPE ( cols )` declaration). Real headers are tiny — a few
/// hundred bytes at most. The cap exists to fail-fast on misuse: a
/// `DatasetReader` pointed at a multi-gigabyte non-`@dataset` input
/// shouldn't run through the whole buffer looking for one.
pub const DEFAULT_HEADER_MAX_BYTES: usize = 64 * 1024;

/// Chunk size for [`io::Read`] pulls. Larger reduces syscall pressure;
/// smaller bounds per-row peak buffer occupancy. 4 KiB matches the cpp
/// reference's `kStreamPullSize` and typical row sizes.
const STREAM_PULL_SIZE: usize = 4096;

/// Streaming row reader for a single `@dataset` directive.
///
/// A `DatasetReader` is positioned at the first row after construction.
/// Iterate via the standard `for ... in` (the reader implements
/// [`Iterator`]) or call [`DatasetReader::next_row`] in a loop until it
/// returns [`None`]. Any error makes the reader sticky.
///
/// For documents containing multiple `@dataset` directives, call
/// [`DatasetReader::new`] again on [`DatasetReader::tail`].
pub struct DatasetReader<R: Read> {
    src: R,
    pending: Vec<u8>,
    src_eof: bool,
    finished: bool,
    err: Option<PxfError>,
    type_: String,
    columns: Vec<String>,
    directives: Vec<Directive>,
    limits: Limits,
    /// The row type whose column paths [`Self::scan_one`] last checked.
    paths_checked: Option<MessageDescriptor>,
}

impl<R: Read> DatasetReader<R> {
    /// Consume the leading directives and the `@dataset TYPE ( cols )`
    /// header from `src`. The reader is positioned at the first row
    /// when this returns `Ok(_)`.
    ///
    /// Returns an `Err` if the input contains no `@dataset` directive
    /// before EOF, on a header parse error, or if the header byte
    /// budget (64 KiB by default) is exceeded.
    pub fn new(src: R) -> Result<Self, PxfError> {
        Self::with_limits(src, Limits::default())
    }

    /// [`DatasetReader::new`] under per-call [`Limits`]: the bytes held
    /// while looking for a row boundary are capped at
    /// `max_message_size`, so a stream that never ends a row cannot grow
    /// the buffer without bound, and each row is parsed under the same
    /// limits (HARDENING.md § Mandatory limits).
    pub fn with_limits(src: R, limits: Limits) -> Result<Self, PxfError> {
        let mut r = Self {
            src,
            pending: Vec::new(),
            src_eof: false,
            finished: false,
            err: None,
            type_: String::new(),
            columns: Vec::new(),
            directives: Vec::new(),
            limits,
            paths_checked: None,
        };
        r.read_header()?;
        Ok(r)
    }

    /// Row message type declared by the `@dataset` header (e.g.
    /// `"trades.v1.Trade"`).
    pub fn type_name(&self) -> &str {
        &self.type_
    }

    /// Column field names declared by the `@dataset` header, in source order.
    pub fn columns(&self) -> &[String] {
        &self.columns
    }

    /// Side-channel directives (`@<name>` / `@entry` / etc., NOT `@type`
    /// or `@dataset`) that appeared before the `@dataset` header. Stable
    /// for the lifetime of the reader.
    pub fn directives(&self) -> &[Directive] {
        &self.directives
    }

    /// True once the row sequence has been exhausted.
    pub fn done(&self) -> bool {
        self.finished
    }

    /// Read the next row. Returns `None` once the table's row sequence
    /// is exhausted; after a sticky error, also returns `None`.
    pub fn next_row(&mut self) -> Option<Result<DatasetRow, PxfError>> {
        if let Some(e) = &self.err {
            return Some(Err(e.clone()));
        }
        if self.finished {
            return None;
        }
        loop {
            match find_next_row(&self.pending) {
                Err(e) => {
                    self.err = Some(e.clone());
                    return Some(Err(e));
                }
                Ok(FindRow::Found { start, end }) => {
                    let parsed = std::str::from_utf8(&self.pending[start..=end])
                        .map_err(|_| {
                            PxfError::new(Position::default(), "pxf: row is not valid UTF-8")
                        })
                        .and_then(|row| parse_row(row, self.columns.len(), self.limits));
                    // Advance past the consumed bytes whether parse
                    // succeeded or not — on failure we don't want to
                    // retry the same bad row forever.
                    self.pending.drain(..=end);
                    return Some(parsed.inspect_err(|e| self.err = Some(e.clone())));
                }
                Ok(FindRow::Done) => {
                    self.finished = true;
                    return None;
                }
                Ok(found @ (FindRow::NeedMore | FindRow::InRow)) => {
                    if self.src_eof {
                        // Only end of input between rows ends the sequence:
                        // a row whose `)` never came is an error, not the
                        // end of the rows (protowire#336).
                        if matches!(found, FindRow::InRow) {
                            let e = PxfError::new(
                                Position::default(),
                                "pxf: expected ',' or ')' in @dataset row, got end of input",
                            );
                            self.err = Some(e.clone());
                            return Some(Err(e));
                        }
                        self.finished = true;
                        return None;
                    }
                    if self.pending.len() >= self.limits.max_message_size {
                        let e = PxfError::new(
                            Position::default(),
                            format!(
                                "pxf: DatasetReader: row exceeds MaxMessageSize={} bytes without a row boundary",
                                self.limits.max_message_size
                            ),
                        );
                        self.err = Some(e.clone());
                        return Some(Err(e));
                    }
                    if let Err(e) = self.pull(STREAM_PULL_SIZE) {
                        self.err = Some(e.clone());
                        return Some(Err(e));
                    }
                }
            }
        }
    }

    /// Read the next row and bind its cells to a fresh
    /// [`DynamicMessage`] of `desc`. Returns `Ok(Some(msg))` on success,
    /// `Ok(None)` at EOF, or `Err` on parse / I/O / bind error.
    ///
    /// Named `scan_one` rather than `scan` because `DatasetReader`
    /// implements [`Iterator`], whose `scan` method would otherwise
    /// shadow this one at the call site.
    pub fn scan_one(
        &mut self,
        desc: &MessageDescriptor,
        options: UnmarshalOptions<'_>,
    ) -> Result<Option<DynamicMessage>, PxfError> {
        match self.next_row() {
            None => Ok(None),
            Some(Err(e)) => Err(e),
            Some(Ok(row)) => {
                // The column paths depend on the type and the header, never
                // on the row, so they are checked once per type.
                let checked = self.paths_checked.as_ref() == Some(desc);
                let msg = bind(desc, &self.columns, &row, options, checked)?;
                if !checked {
                    self.paths_checked = Some(desc.clone());
                }
                Ok(Some(msg))
            }
        }
    }

    /// Returns a [`Read`] that yields the bytes the reader buffered
    /// but didn't consume, followed by the remaining bytes from the
    /// underlying source. Use to chain a second `DatasetReader` for
    /// documents with multiple `@dataset` directives.
    ///
    /// MUST only be called after iteration has reported [`Self::done`].
    /// Calling earlier returns bytes the current reader still intends
    /// to consume, which will desync the next reader.
    pub fn tail(self) -> impl Read {
        std::io::Cursor::new(self.pending).chain(self.src)
    }

    // ---- internals -------------------------------------------------------

    fn pull(&mut self, n: usize) -> Result<(), PxfError> {
        if self.src_eof {
            return Ok(());
        }
        let mut buf = vec![0u8; n];
        match self.src.read(&mut buf) {
            Ok(0) => {
                self.src_eof = true;
                Ok(())
            }
            Ok(got) => {
                self.pending.extend_from_slice(&buf[..got]);
                Ok(())
            }
            Err(e) if e.kind() == io::ErrorKind::Interrupted => self.pull(n),
            Err(e) => Err(PxfError::new(
                Position::default(),
                format!("pxf: DatasetReader: read error: {}", e),
            )),
        }
    }

    fn read_header(&mut self) -> Result<(), PxfError> {
        loop {
            match scan_header_end(&self.pending) {
                Err(e) => return Err(e),
                Ok(Some(end)) => {
                    // Parse the header prefix as a (rowless) PXF
                    // document; parse_dataset_directive validates
                    // everything we care about (leading-directive
                    // shape, @type / @dataset conflict, dotted columns,
                    // etc.).
                    let header = std::str::from_utf8(&self.pending[..=end]).map_err(|_| {
                        PxfError::new(
                            Position::default(),
                            "pxf: @dataset header is not valid UTF-8",
                        )
                    })?;
                    let doc = parse_with_limits(header, self.limits)?;
                    if doc.datasets.is_empty() {
                        // Defensive — scan_header_end found @dataset but
                        // parse() disagreed.
                        return Err(PxfError::new(
                            Position::default(),
                            "pxf: no @dataset directive in stream",
                        ));
                    }
                    let tbl = &doc.datasets[0];
                    self.type_ = tbl.r#type.clone();
                    self.columns = tbl.columns.clone();
                    self.directives = doc.directives;
                    self.pending.drain(..=end);
                    return Ok(());
                }
                Ok(None) => {
                    if self.src_eof {
                        return Err(PxfError::new(
                            Position::default(),
                            "pxf: no @dataset directive in stream",
                        ));
                    }
                    if self.pending.len() >= DEFAULT_HEADER_MAX_BYTES {
                        return Err(PxfError::new(
                            Position::default(),
                            format!(
                                "pxf: @dataset header exceeds {} bytes; raise the budget or check that the input begins with `@dataset TYPE (cols)`",
                                DEFAULT_HEADER_MAX_BYTES
                            ),
                        ));
                    }
                    self.pull(STREAM_PULL_SIZE)?;
                }
            }
        }
    }
}

impl<R: Read> Iterator for DatasetReader<R> {
    type Item = Result<DatasetRow, PxfError>;
    fn next(&mut self) -> Option<Self::Item> {
        self.next_row()
    }
}

/// Bind a `@dataset` row's cells to fields of a fresh [`DynamicMessage`]
/// of `desc` by column name. `columns` and `row.cells` MUST have the
/// same length.
///
/// A column is a field of `desc`, or a dotted path to a field inside one of
/// its sub-messages. A path that names no field, goes through a repeated,
/// map or scalar field, or goes into a type written as a literal fails the
/// row, its cells empty or not (draft §3.4.4).
///
/// Cell-state semantics (mirrors draft §3.4.4):
///   - `None` cell — field absent. `(pxf.default)` applies if declared;
///     `(pxf.required)` errors if neither default nor value is present.
///   - `Some(Value::Null(_))` — present-but-null, where the field may be
///     null; elsewhere the body's error (draft § Booleans, Null, and
///     Identifier Values).
///   - any other `Some(Value::*)` — field set to that value, as the body
///     entry `c = <value>` would set it: a list or block cell included,
///     so `{}` is a present empty message.
///
/// Strategy is **format-and-reparse**: render the row as a synthetic PXF
/// body and run it through [`crate::unmarshal_full`], reusing every branch
/// of the decoder (WKT timestamps / durations, wrapper-nullability,
/// enum-by-name, `pxf.required` / `pxf.default`, oneof) instead of growing a
/// parallel Value→FieldDescriptor switch. Cells sharing a path prefix land
/// in one block, in the order its first non-empty cell appears, because two
/// blocks would bind one sub-message twice; a prefix with no non-empty cell
/// writes no block, so its sub-message stays absent. An error names the
/// column it fell in.
///
/// `skip_validate` on `options` defaults to `false` (matching
/// `unmarshal`); callers in a tight scan loop typically want to set
/// it to `true` after the descriptor has been validated once.
pub fn bind_row(
    desc: &MessageDescriptor,
    columns: &[String],
    row: &DatasetRow,
    options: UnmarshalOptions<'_>,
) -> Result<DynamicMessage, PxfError> {
    bind(desc, columns, row, options, false)
}

fn bind(
    desc: &MessageDescriptor,
    columns: &[String],
    row: &DatasetRow,
    mut options: UnmarshalOptions<'_>,
    paths_checked: bool,
) -> Result<DynamicMessage, PxfError> {
    if columns.len() != row.cells.len() {
        return Err(PxfError::new(
            row.pos,
            format!(
                "pxf: bind_row: {} columns vs {} cells",
                columns.len(),
                row.cells.len()
            ),
        ));
    }
    if !paths_checked {
        let depth = options.limits.max_nesting_depth;
        if let Some(msg) = check_column_paths(desc, columns, depth) {
            return Err(PxfError::new(row.pos, msg));
        }
    }
    let mut writer = BodyWriter {
        // The empty first line keeps an error the decoder reports at 1:1,
        // a missing required field, out of every column's span.
        body: String::from("\n"),
        line: 2,
        spans: Vec::with_capacity(row.cells.len()),
        cells: &row.cells,
    };
    writer.write_row(columns);
    let BodyWriter { body, spans, .. } = writer;
    // The row's bytes were capped as they were read, and the body it is
    // rendered to may run wider.
    options.limits.max_message_size = usize::MAX;
    match unmarshal_full(&body, desc, options) {
        Ok((msg, _)) => Ok(msg),
        Err(e) => Err(at_row(e, row.pos, columns, &spans)),
    }
}

/// Move a decode error to the row it came from and name the column it fell
/// in: the decoder read a synthetic body, so its position is a line of that
/// body. A row built by hand has no position, and keeps the body's.
fn at_row(e: PxfError, pos: Position, columns: &[String], spans: &[CellSpan]) -> PxfError {
    let msg = match spans
        .iter()
        .find(|s| s.first <= e.pos.line && e.pos.line <= s.last)
    {
        Some(s) => format!("column {}: {}", columns[s.col], e.msg),
        None => e.msg,
    };
    PxfError::new(if pos.line == 0 { e.pos } else { pos }, msg)
}

/// The lines of a row's body that a column's entry takes.
struct CellSpan {
    col: usize,
    first: usize,
    last: usize,
}

/// One entry of a row body: a column's cell, or the block of a path prefix
/// that dotted columns share.
#[derive(Default)]
struct BodyNode<'a> {
    name: &'a str,
    /// The column whose cell this is; `None` for a block.
    col: Option<usize>,
    children: Vec<BodyNode<'a>>,
    /// Index of each block among `children`, by name.
    blocks: HashMap<&'a str, usize>,
}

impl BodyNode<'_> {
    /// The column of the first cell inside this block.
    fn first_col(&self) -> usize {
        let first = &self.children[0];
        first.col.unwrap_or_else(|| first.first_col())
    }
}

struct BodyWriter<'a> {
    body: String,
    /// The number of the next line to be written.
    line: usize,
    spans: Vec<CellSpan>,
    cells: &'a [Option<Value>],
}

impl BodyWriter<'_> {
    fn write_row(&mut self, columns: &[String]) {
        let mut root = BodyNode::default();
        for (i, cell) in self.cells.iter().enumerate() {
            if cell.is_none() {
                continue;
            }
            let mut node = &mut root;
            let mut segs = columns[i].split('.').peekable();
            while let Some(seg) = segs.next() {
                if segs.peek().is_none() {
                    node.children.push(BodyNode {
                        name: seg,
                        col: Some(i),
                        ..Default::default()
                    });
                    break;
                }
                let next = match node.blocks.get(seg) {
                    Some(&next) => next,
                    None => {
                        node.blocks.insert(seg, node.children.len());
                        node.children.push(BodyNode {
                            name: seg,
                            ..Default::default()
                        });
                        node.children.len() - 1
                    }
                };
                node = &mut node.children[next];
            }
        }
        self.write_block(&root);
    }

    fn write_block(&mut self, node: &BodyNode<'_>) {
        for child in &node.children {
            if let Some(col) = child.col {
                self.write_cell(child.name, col);
                continue;
            }
            // An error on the block's own lines, a second oneof member say,
            // is laid at the first column in it. Its span comes after its
            // cells', so a cell's wins.
            let first = self.line;
            self.body.push_str(child.name);
            self.body.push_str(" {\n");
            self.line += 1;
            self.write_block(child);
            self.body.push_str("}\n");
            self.spans.push(CellSpan {
                col: child.first_col(),
                first,
                last: self.line,
            });
            self.line += 1;
        }
    }

    fn write_cell(&mut self, name: &str, col: usize) {
        let value = self.cells[col].as_ref().expect("only non-empty cells");
        let text = format_value_text(value);
        self.body.push_str(name);
        self.body.push_str(" = ");
        self.body.push_str(&text);
        self.body.push('\n');
        let last = self.line + text.matches('\n').count();
        self.spans.push(CellSpan {
            col,
            first: self.line,
            last,
        });
        self.line = last + 1;
    }
}

// ---- byte-level row scanner -------------------------------------------

enum FindRow {
    Found {
        start: usize,
        end: usize,
    },
    Done,
    NeedMore,
    /// Input ran out inside a row.
    InRow,
}

/// Locates the next `( ... )` row in `input`, skipping leading
/// whitespace + comments. Returns:
///   - `Found { start, end }` (inclusive of the `(` and `)`),
///   - `Done` at the next directive, which ends the rows,
///   - `NeedMore` when the input runs out between rows,
///   - `InRow` when it runs out inside a row,
///   - `Err` at anything else: the row sequence ends only at end of input
///     or at a directive (draft §3.4.4, "Appending"; protowire#336).
fn find_next_row(input: &[u8]) -> Result<FindRow, PxfError> {
    let n = input.len();
    let mut i = 0;
    while i < n {
        let ch = input[i];
        if ch == b' ' || ch == b'\t' || ch == b'\r' || ch == b'\n' {
            i += 1;
            continue;
        }
        let j = skip_string_or_comment(input, i)?;
        if j < 0 {
            return Ok(FindRow::NeedMore);
        }
        let j = j as usize;
        if j != i {
            i = j;
            continue;
        }
        break;
    }
    if i >= n {
        return Ok(FindRow::NeedMore);
    }
    if input[i] == b'@' {
        return Ok(FindRow::Done);
    }
    if input[i] != b'(' {
        return Err(PxfError::new(
            Position::default(),
            "pxf: @dataset directive cannot coexist with top-level field entries; expected a row or a directive (draft §3.4.4)",
        ));
    }
    match find_matching_paren_safe(input, i)? {
        Some(end) => Ok(FindRow::Found { start: i, end }),
        None => Ok(FindRow::InRow),
    }
}

/// Locates the closing `)` of the first complete `@dataset TYPE ( cols )`
/// header in `input`. Returns `Ok(Some(end))`, `Ok(None)` when more
/// bytes are needed, or `Err` on a malformed literal.
fn scan_header_end(input: &[u8]) -> Result<Option<usize>, PxfError> {
    let Some(at) = find_at_table(input)? else {
        return Ok(None);
    };
    let Some(lparen) = find_next_char(input, at + b"@dataset".len(), b'(')? else {
        return Ok(None);
    };
    find_matching_paren_safe(input, lparen)
}

fn find_at_table(input: &[u8]) -> Result<Option<usize>, PxfError> {
    let n = input.len();
    let mut i = 0;
    while i < n {
        let j = skip_string_or_comment(input, i)?;
        if j < 0 {
            return Ok(None);
        }
        let j = j as usize;
        if j != i {
            i = j;
            continue;
        }
        if input[i] == b'@' && i + 8 <= n && &input[i..i + 8] == b"@dataset" {
            let after = i + 8;
            if after == n {
                // Could be `@dataset` followed by more bytes — be conservative.
                return Ok(None);
            }
            if !is_ident_part(input[after]) {
                return Ok(Some(i));
            }
        }
        i += 1;
    }
    Ok(None)
}

fn find_next_char(input: &[u8], start: usize, ch: u8) -> Result<Option<usize>, PxfError> {
    let n = input.len();
    let mut i = start;
    while i < n {
        let j = skip_string_or_comment(input, i)?;
        if j < 0 {
            return Ok(None);
        }
        let j = j as usize;
        if j != i {
            i = j;
            continue;
        }
        if input[i] == ch {
            return Ok(Some(i));
        }
        i += 1;
    }
    Ok(None)
}

/// Find the `)` matching the `(` at `open_idx`. String / bytes-literal /
/// comment aware. Returns `Ok(Some(end))`, `Ok(None)` for incomplete
/// input, or `Err` on malformed literals inside.
fn find_matching_paren_safe(input: &[u8], open_idx: usize) -> Result<Option<usize>, PxfError> {
    let n = input.len();
    let mut depth: usize = 1;
    let mut i = open_idx + 1;
    while i < n {
        let j = skip_string_or_comment(input, i)?;
        if j < 0 {
            return Ok(None);
        }
        let j = j as usize;
        if j != i {
            i = j;
            continue;
        }
        let ch = input[i];
        if ch == b'(' {
            depth += 1;
            i += 1;
        } else if ch == b')' {
            depth -= 1;
            if depth == 0 {
                return Ok(Some(i));
            }
            i += 1;
        } else {
            i += 1;
        }
    }
    Ok(None)
}

/// Returns the byte index past a string / bytes literal / comment
/// starting at `i`. Returns `i` unchanged if `i` is not at an opener.
/// Returns `-1` when the construct is incomplete (caller pulls more).
/// Returns `Err` when the construct is malformed in a way that can't
/// be fixed by more bytes.
fn skip_string_or_comment(input: &[u8], i: usize) -> Result<i64, PxfError> {
    let n = input.len();
    if i >= n {
        return Ok(i as i64);
    }
    let ch = input[i];
    if ch == b'"' {
        if i + 2 < n && input[i + 1] == b'"' && input[i + 2] == b'"' {
            return Ok(skip_triple_string(input, i));
        }
        return skip_simple_string(input, i);
    }
    if ch == b'b' && i + 1 < n && input[i + 1] == b'"' {
        return skip_bytes_literal(input, i);
    }
    if ch == b'#' {
        return Ok(skip_line_comment(input, i + 1) as i64);
    }
    if ch == b'/' && i + 1 < n && input[i + 1] == b'/' {
        return Ok(skip_line_comment(input, i + 2) as i64);
    }
    if ch == b'/' && i + 1 < n && input[i + 1] == b'*' {
        return Ok(skip_block_comment(input, i + 2));
    }
    Ok(i as i64)
}

fn skip_simple_string(input: &[u8], i: usize) -> Result<i64, PxfError> {
    let n = input.len();
    let mut j = i + 1;
    while j < n {
        match input[j] {
            b'\\' => {
                if j + 1 >= n {
                    return Ok(-1);
                }
                j += 2;
            }
            b'"' => return Ok((j + 1) as i64),
            b'\n' => {
                return Err(PxfError::new(
                    Position::default(),
                    "pxf: unterminated string literal",
                ));
            }
            _ => j += 1,
        }
    }
    Ok(-1)
}

fn skip_triple_string(input: &[u8], i: usize) -> i64 {
    let n = input.len();
    let mut j = i + 3;
    while j + 2 < n {
        if input[j] == b'"' && input[j + 1] == b'"' && input[j + 2] == b'"' {
            return (j + 3) as i64;
        }
        j += 1;
    }
    -1
}

fn skip_bytes_literal(input: &[u8], i: usize) -> Result<i64, PxfError> {
    let n = input.len();
    let mut j = i + 2; // past `b"`
    while j < n {
        match input[j] {
            b'\\' => {
                if j + 1 >= n {
                    return Ok(-1);
                }
                j += 2;
            }
            b'"' => return Ok((j + 1) as i64),
            b'\n' => {
                return Err(PxfError::new(
                    Position::default(),
                    "pxf: unterminated bytes literal",
                ));
            }
            _ => j += 1,
        }
    }
    Ok(-1)
}

fn skip_line_comment(input: &[u8], from: usize) -> usize {
    let n = input.len();
    let mut j = from;
    while j < n && input[j] != b'\n' {
        j += 1;
    }
    j
}

fn skip_block_comment(input: &[u8], from: usize) -> i64 {
    let n = input.len();
    let mut j = from;
    while j + 1 < n {
        if input[j] == b'*' && input[j + 1] == b'/' {
            return (j + 2) as i64;
        }
        j += 1;
    }
    -1
}

fn is_ident_part(c: u8) -> bool {
    c.is_ascii_alphanumeric() || c == b'_'
}
