// SPDX-License-Identifier: MIT
// Copyright (c) 2026 TrendVidia, LLC.
//! Streaming consumption for the `@table` directive (draft §3.4.4).
//!
//! [`crate::unmarshal_full`] materializes every row of an `@table`
//! directive into [`crate::Presence::tables`]. That works for small
//! datasets and breaks for the CSV-replacement workload `@table` was
//! designed for. [`TableReader`] pulls one row at a time from an
//! [`io::Read`] source; per-row arity and the v1 cell-grammar rule are
//! enforced at consume time (not deferred to end-of-input), and rows
//! are yielded in source order — both invariants the spec requires of
//! streaming consumers.
//!
//! Convenience: [`TableReader::scan`] reads the next row and binds its
//! cells to a fresh [`prost_reflect::DynamicMessage`]. [`bind_row`] is
//! exported for callers iterating
//! [`crate::Presence::tables`]\[i\].rows from the materializing path.
//!
//! Mirrors the cpp port at `protowire-cpp/src/pxf/table_reader.cc`.

use std::io::{self, Read};

use prost_reflect::{DynamicMessage, MessageDescriptor};

use crate::ast::{Directive, TableRow, Value};
use crate::errors::PxfError;
use crate::parser::parse;
use crate::token::Position;
use crate::{unmarshal, UnmarshalOptions};

/// Default cap on the @table header (leading directives plus the
/// `@table TYPE ( cols )` declaration). Real headers are tiny — a few
/// hundred bytes at most. The cap exists to fail-fast on misuse: a
/// `TableReader` pointed at a multi-gigabyte non-`@table` input
/// shouldn't run through the whole buffer looking for one.
pub const DEFAULT_HEADER_MAX_BYTES: usize = 64 * 1024;

/// Chunk size for [`io::Read`] pulls. Larger reduces syscall pressure;
/// smaller bounds per-row peak buffer occupancy. 4 KiB matches the cpp
/// reference's `kStreamPullSize` and typical row sizes.
const STREAM_PULL_SIZE: usize = 4096;

/// Streaming row reader for a single `@table` directive.
///
/// A `TableReader` is positioned at the first row after construction.
/// Iterate via the standard `for ... in` (the reader implements
/// [`Iterator`]) or call [`TableReader::next_row`] in a loop until it
/// returns [`None`]. Any error makes the reader sticky.
///
/// For documents containing multiple `@table` directives, call
/// [`TableReader::new`] again on [`TableReader::tail`].
pub struct TableReader<R: Read> {
    src: R,
    pending: Vec<u8>,
    src_eof: bool,
    finished: bool,
    err: Option<PxfError>,
    type_: String,
    columns: Vec<String>,
    directives: Vec<Directive>,
}

impl<R: Read> TableReader<R> {
    /// Consume the leading directives and the `@table TYPE ( cols )`
    /// header from `src`. The reader is positioned at the first row
    /// when this returns `Ok(_)`.
    ///
    /// Returns an `Err` if the input contains no `@table` directive
    /// before EOF, on a header parse error, or if the header byte
    /// budget (64 KiB by default) is exceeded.
    pub fn new(src: R) -> Result<Self, PxfError> {
        let mut r = Self {
            src,
            pending: Vec::new(),
            src_eof: false,
            finished: false,
            err: None,
            type_: String::new(),
            columns: Vec::new(),
            directives: Vec::new(),
        };
        r.read_header()?;
        Ok(r)
    }

    /// Row message type declared by the `@table` header (e.g.
    /// `"trades.v1.Trade"`).
    pub fn type_name(&self) -> &str {
        &self.type_
    }

    /// Column field names declared by the `@table` header, in source order.
    pub fn columns(&self) -> &[String] {
        &self.columns
    }

    /// Side-channel directives (`@<name>` / `@entry` / etc., NOT `@type`
    /// or `@table`) that appeared before the `@table` header. Stable
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
    pub fn next_row(&mut self) -> Option<Result<TableRow, PxfError>> {
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
                    // Parse the row by handing a synthetic
                    // `@table _.Row (c1,c2,...) <rowBytes>` to the AST
                    // parser, reusing parse_table_row's arity check and
                    // v1 cell-grammar enforcement.
                    let row_bytes = &self.pending[start..=end];
                    let synthetic = build_synthetic_row(&self.columns, row_bytes);
                    let parsed = parse(&synthetic);
                    // Advance past the consumed bytes whether parse
                    // succeeded or not — on failure we don't want to
                    // retry the same bad row forever.
                    self.pending.drain(..=end);
                    match parsed {
                        Ok(doc) => {
                            if doc.tables.is_empty() || doc.tables[0].rows.is_empty() {
                                let e = PxfError::new(
                                    Position::default(),
                                    "pxf: TableReader: synthetic row parse produced no row",
                                );
                                self.err = Some(e.clone());
                                return Some(Err(e));
                            }
                            // Take the first (and only) row from the
                            // synthetic doc.
                            let mut tables = doc.tables;
                            let mut rows = std::mem::take(&mut tables[0].rows);
                            return Some(Ok(rows.swap_remove(0)));
                        }
                        Err(e) => {
                            self.err = Some(e.clone());
                            return Some(Err(e));
                        }
                    }
                }
                Ok(FindRow::Done) => {
                    self.finished = true;
                    return None;
                }
                Ok(FindRow::NeedMore) => {
                    if self.src_eof {
                        self.finished = true;
                        return None;
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
    /// Named `scan_one` rather than `scan` because `TableReader`
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
            Some(Ok(row)) => Ok(Some(bind_row(desc, &self.columns, &row, options)?)),
        }
    }

    /// Returns a [`Read`] that yields the bytes the reader buffered
    /// but didn't consume, followed by the remaining bytes from the
    /// underlying source. Use to chain a second `TableReader` for
    /// documents with multiple `@table` directives.
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
                format!("pxf: TableReader: read error: {}", e),
            )),
        }
    }

    fn read_header(&mut self) -> Result<(), PxfError> {
        loop {
            match scan_header_end(&self.pending) {
                Err(e) => return Err(e),
                Ok(Some(end)) => {
                    // Parse the header prefix as a (rowless) PXF
                    // document; parse_table_directive validates
                    // everything we care about (leading-directive
                    // shape, @type / @table conflict, dotted columns,
                    // etc.).
                    let header = std::str::from_utf8(&self.pending[..=end]).map_err(|_| {
                        PxfError::new(Position::default(), "pxf: @table header is not valid UTF-8")
                    })?;
                    let doc = parse(header)?;
                    if doc.tables.is_empty() {
                        // Defensive — scan_header_end found @table but
                        // parse() disagreed.
                        return Err(PxfError::new(
                            Position::default(),
                            "pxf: no @table directive in stream",
                        ));
                    }
                    let tbl = &doc.tables[0];
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
                            "pxf: no @table directive in stream",
                        ));
                    }
                    if self.pending.len() >= DEFAULT_HEADER_MAX_BYTES {
                        return Err(PxfError::new(
                            Position::default(),
                            format!(
                                "pxf: @table header exceeds {} bytes; raise the budget or check that the input begins with `@table TYPE (cols)`",
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

impl<R: Read> Iterator for TableReader<R> {
    type Item = Result<TableRow, PxfError>;
    fn next(&mut self) -> Option<Self::Item> {
        self.next_row()
    }
}

/// Bind a `@table` row's cells to fields of a fresh [`DynamicMessage`]
/// of `desc` by column name. `columns` and `row.cells` MUST have the
/// same length.
///
/// Cell-state semantics (mirrors draft §3.4.4):
///   - `None` cell — field absent. `(pxf.default)` applies if declared;
///     `(pxf.required)` errors if neither default nor value is present.
///   - `Some(Value::Null(_))` — field cleared, per draft §3.9.
///   - any other `Some(Value::*)` — field set to that value.
///
/// Strategy is **format-and-reparse**: render the row as a synthetic
/// PXF body (`<col> = <val>` per non-None cell) and run it through
/// [`crate::unmarshal`]. This mirrors `protowire-cpp`'s `BindRow` and
/// reuses every branch of the existing decoder (WKT timestamps /
/// durations, wrapper-nullability, enum-by-name, `pxf.required` /
/// `pxf.default`, oneof) instead of growing a parallel
/// Value→FieldDescriptor switch.
///
/// `skip_validate` on `options` defaults to `false` (matching
/// `unmarshal`); callers in a tight scan loop typically want to set
/// it to `true` after the descriptor has been validated once.
pub fn bind_row(
    desc: &MessageDescriptor,
    columns: &[String],
    row: &TableRow,
    options: UnmarshalOptions<'_>,
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
    let mut body = String::new();
    for (col, cell) in columns.iter().zip(row.cells.iter()) {
        let Some(v) = cell else { continue };
        body.push_str(col);
        body.push_str(" = ");
        cell_to_pxf(v, &mut body, row.pos)?;
        body.push('\n');
    }
    unmarshal(&body, desc, options)
}

fn cell_to_pxf(v: &Value, out: &mut String, pos: Position) -> Result<(), PxfError> {
    match v {
        Value::Null(_) => out.push_str("null"),
        Value::String(s) => {
            out.push('"');
            for ch in s.value.chars() {
                if ch == '"' || ch == '\\' {
                    out.push('\\');
                }
                out.push(ch);
            }
            out.push('"');
        }
        Value::Int(v) => out.push_str(&v.raw),
        Value::Float(v) => out.push_str(&v.raw),
        Value::Bool(v) => out.push_str(if v.value { "true" } else { "false" }),
        Value::Bytes(v) => {
            out.push_str("b\"");
            out.push_str(&encode_base64(&v.value));
            out.push('"');
        }
        Value::Ident(v) => out.push_str(&v.name),
        Value::Timestamp(v) => out.push_str(&v.raw),
        Value::Duration(v) => out.push_str(&v.raw),
        _ => {
            // List / Block — rejected at row-parse time, so unreachable
            // in practice.
            return Err(PxfError::new(
                pos,
                "pxf: bind_row: unexpected cell variant (v1 @table cells are scalar-shaped)",
            ));
        }
    }
    Ok(())
}

fn encode_base64(bytes: &[u8]) -> String {
    const ALPH: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(bytes.len().div_ceil(3) * 4);
    let chunks = bytes.chunks_exact(3);
    let rem = chunks.remainder();
    for c in chunks {
        let n = ((c[0] as u32) << 16) | ((c[1] as u32) << 8) | (c[2] as u32);
        out.push(ALPH[((n >> 18) & 63) as usize] as char);
        out.push(ALPH[((n >> 12) & 63) as usize] as char);
        out.push(ALPH[((n >> 6) & 63) as usize] as char);
        out.push(ALPH[(n & 63) as usize] as char);
    }
    match rem.len() {
        1 => {
            let n = (rem[0] as u32) << 16;
            out.push(ALPH[((n >> 18) & 63) as usize] as char);
            out.push(ALPH[((n >> 12) & 63) as usize] as char);
            out.push('=');
            out.push('=');
        }
        2 => {
            let n = ((rem[0] as u32) << 16) | ((rem[1] as u32) << 8);
            out.push(ALPH[((n >> 18) & 63) as usize] as char);
            out.push(ALPH[((n >> 12) & 63) as usize] as char);
            out.push(ALPH[((n >> 6) & 63) as usize] as char);
            out.push('=');
        }
        _ => {}
    }
    out
}

fn build_synthetic_row(columns: &[String], row_bytes: &[u8]) -> String {
    let mut s = String::with_capacity(row_bytes.len() + 64);
    s.push_str("@table _.Row (");
    for (i, c) in columns.iter().enumerate() {
        if i > 0 {
            s.push(',');
        }
        s.push_str(c);
    }
    s.push_str(")\n");
    // row_bytes is guaranteed UTF-8 because the upstream input was
    // string-shaped (the byte scanner only operates on already-
    // validated PXF). Defensive fallback: lossy conversion.
    s.push_str(std::str::from_utf8(row_bytes).unwrap_or(""));
    s.push('\n');
    s
}

// ---- byte-level row scanner -------------------------------------------

enum FindRow {
    Found { start: usize, end: usize },
    Done,
    NeedMore,
}

/// Locates the next `( ... )` row in `input`, skipping leading
/// whitespace + comments. Returns:
///   - `Found { start, end }` (inclusive of the `(` and `)`),
///   - `Done` when the next significant byte is NOT `(` (end of rows),
///   - `NeedMore` when the input runs out mid-scan.
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
    if input[i] != b'(' {
        return Ok(FindRow::Done);
    }
    match find_matching_paren_safe(input, i)? {
        Some(end) => Ok(FindRow::Found { start: i, end }),
        None => Ok(FindRow::NeedMore),
    }
}

/// Locates the closing `)` of the first complete `@table TYPE ( cols )`
/// header in `input`. Returns `Ok(Some(end))`, `Ok(None)` when more
/// bytes are needed, or `Err` on a malformed literal.
fn scan_header_end(input: &[u8]) -> Result<Option<usize>, PxfError> {
    let Some(at) = find_at_table(input)? else {
        return Ok(None);
    };
    let Some(lparen) = find_next_char(input, at + b"@table".len(), b'(')? else {
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
        if input[i] == b'@' && i + 6 <= n && &input[i..i + 6] == b"@table" {
            let after = i + 6;
            if after == n {
                // Could be `@table` followed by more bytes — be conservative.
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
