// SPDX-License-Identifier: MIT
// Copyright (c) 2026 TrendVidia, LLC.
//! Streaming consumption for the `@dataset` directive (draft §3.4.4).
//!
//! [`crate::unmarshal_full`] materializes every row of an `@dataset`
//! directive into [`crate::Presence::datasets`]. That works for small
//! datasets and breaks for the CSV-replacement workload `@dataset` was
//! designed for. [`DatasetReader`] pulls one row at a time from an
//! [`io::Read`] source, with working-set memory bounded by the size of the
//! largest row.
//!
//! The stream yields exactly the rows [`crate::parse`] reads, with its
//! positions and comments, or fails with exactly its error, however the
//! input is cut into reads (draft §3.4.4: streaming and materializing APIs
//! produce the same row sequence). It finds rows with the lexer `parse`
//! uses, resuming where the previous read left it, so a row costs time
//! linear in its size however many reads it arrives in, and parses each
//! row with `parse`'s parser at its document position. A port of
//! protowire-go's `dataset_stream.go` (#117).
//!
//! Convenience: [`DatasetReader::scan_one`] reads the next row and binds
//! its cells to a fresh [`prost_reflect::DynamicMessage`]. [`bind_row`] is
//! exported for callers iterating
//! [`crate::Presence::datasets`]\[i\].rows from the materializing path.

use std::collections::HashMap;
use std::io::{self, Read};

use prost_reflect::{DynamicMessage, MessageDescriptor};

use crate::ast::{DatasetRow, Directive, Document, Value};
use crate::dataset_paths::check_column_paths;
use crate::errors::PxfError;
use crate::format::format_value;
use crate::lexer::{is_ident_part, Lexer};
use crate::limits::Limits;
use crate::parser::{BraceMatch, BraceState, Parser, ERR_DATASET_BESIDE_ENTRIES};
use crate::token::{Position, Token, TokenKind};
use crate::{unmarshal_full, UnmarshalOptions};

/// Default cap on the @dataset header (leading directives plus the
/// `@dataset TYPE ( cols )` declaration). Real headers are tiny — a few
/// hundred bytes at most. The cap exists to fail-fast on misuse: a
/// `DatasetReader` pointed at a multi-gigabyte non-`@dataset` input
/// shouldn't run through the whole buffer looking for one.
pub const DEFAULT_HEADER_MAX_BYTES: usize = 64 * 1024;

/// Chunk size for [`io::Read`] pulls. Larger reduces syscall pressure;
/// smaller bounds per-row peak buffer occupancy.
const STREAM_PULL_SIZE: usize = 4096;

/// The length under which a waiting token is lexed again whatever bytes
/// arrive. What may still change a token this short is a byte or two of
/// the lexer's lookahead, and lexing a few bytes again costs nothing.
const SHORT_WAIT: usize = 8;

/// Streaming row reader for a single `@dataset` directive.
///
/// A `DatasetReader` is positioned at the first row after construction.
/// Iterate via the standard `for ... in` (the reader implements
/// [`Iterator`]) or call [`DatasetReader::next_row`] in a loop until it
/// returns [`None`]. Any error makes the reader sticky.
///
/// For documents containing multiple `@dataset` directives, read the next
/// table from [`DatasetReader::tail`] with [`DatasetReader::from_tail`],
/// which reports its rows and errors at their document positions.
pub struct DatasetReader<R: Read> {
    src: R,
    /// Bytes pulled from `src`; those before `head` are consumed, and
    /// every offset below is into `pending[head..]`, the buffer.
    pending: Vec<u8>,
    head: usize,
    /// One read's worth of space, allocated once: a read fills it and only
    /// the bytes it returned are appended, so a reader returning a byte at
    /// a time costs a byte per read, not a buffer.
    scratch: Vec<u8>,
    /// The buffer is valid UTF-8 up to `valid`, a character boundary, and
    /// at `invalid` holds bytes that can never be: the lexer and parser see
    /// only the valid prefix.
    valid: usize,
    invalid: Option<usize>,
    src_eof: bool,
    finished: bool,
    err: Option<PxfError>,
    /// The iterator has yielded `err`, and ends.
    err_yielded: bool,
    type_: String,
    columns: Vec<String>,
    directives: Vec<Directive>,
    limits: Limits,
    /// Document position of the buffer's first byte, so rows and errors
    /// carry `parse`'s positions.
    base: Position,
    /// The header and row scans resume where the previous pull left them,
    /// so a header or a row is lexed once however many reads it arrives
    /// in. `scan_pos` is the start of the first token not yet known to be
    /// complete; `row_start` is the current row's `(`, if the scan is in
    /// one.
    scan_pos: usize,
    row_start: Option<usize>,
    hdr: HeaderScan,
    /// While a token runs past the buffer's end, the scan does not lex it
    /// again until bytes arrive that could end it (`may_end_token`):
    /// `wait_at` is its start and `wait_from` the buffer length already
    /// checked.
    waiting: bool,
    wait_at: usize,
    wait_from: usize,
    /// The bytes the header and row scans have lexed, for the tests that
    /// hold them linear in the input.
    scanned: usize,
    /// Built on another reader's tail, so it starts mid-document.
    chained: bool,
    /// The row type whose column paths [`Self::scan_one`] last checked.
    paths_checked: Option<MessageDescriptor>,
}

/// The bytes a [`DatasetReader`] buffered but did not consume, then the
/// rest of its source, with the document position they start at. Read the
/// next table with [`DatasetReader::from_tail`]; a reader built on it with
/// [`DatasetReader::new`] counts positions from the tail's start.
pub struct DatasetTail<R: Read> {
    inner: io::Chain<io::Cursor<Vec<u8>>, R>,
    at: Position,
}

impl<R: Read> Read for DatasetTail<R> {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        self.inner.read(buf)
    }
}

/// Where the header scan stands: whether it has read the `@dataset`
/// token, and how far into an `@proto` directive whose body is raw
/// protobuf source, which `parse` skips by matching braces rather than
/// lexing it.
#[derive(Clone, Copy)]
struct HeaderScan {
    in_dataset: bool,
    /// 1 after `@proto`, 2 after its type name, else 0.
    proto: u8,
    /// The scan is matching an `@proto` body's braces.
    in_body: bool,
    body: BraceMatch,
}

/// What [`DatasetReader::scan_row`] found.
#[derive(Clone, Copy, PartialEq, Eq)]
enum RowScan {
    /// The input ends before the scan could decide.
    NeedMore,
    /// `buf[start..end]` is a complete `( ... )` row.
    Row,
    /// The row at start cannot close well: a stray `(` or a token the lexer
    /// rejects. Parse it for the error.
    Bad,
    /// A directive starts at start; the row sequence is over.
    End,
    /// Start holds something that is neither a row nor a directive.
    NotRow,
    /// The scan needs bytes that are not valid UTF-8.
    Utf8,
}

impl<R: Read> DatasetReader<R> {
    /// Consume the leading directives and the `@dataset TYPE ( cols )`
    /// header from `src`. The reader is positioned at the first row
    /// when this returns `Ok(_)`.
    ///
    /// Returns an `Err` if the input is a well-formed document with no
    /// `@dataset` directive (`pxf: no @dataset directive in stream`), the
    /// error [`crate::parse`] returns for a malformed one (a header cut
    /// short included), a read failure, or if the header byte budget
    /// (64 KiB) is exceeded.
    pub fn new(src: R) -> Result<Self, PxfError> {
        Self::with_limits(src, Limits::default())
    }

    /// [`DatasetReader::new`] under per-call [`Limits`]: the bytes held
    /// while looking for a row boundary are capped at
    /// `max_message_size`, so a stream that never ends a row cannot grow
    /// the buffer without bound, and the header and every row are parsed
    /// under the same limits, as [`crate::parse_with_limits`] parses them
    /// (HARDENING.md § Mandatory limits).
    pub fn with_limits(src: R, limits: Limits) -> Result<Self, PxfError> {
        Self::build(src, limits, Position::with_offset(1, 1, 0), false)
    }

    fn build(src: R, limits: Limits, base: Position, chained: bool) -> Result<Self, PxfError> {
        let mut r = Self {
            src,
            pending: Vec::new(),
            head: 0,
            scratch: vec![0; STREAM_PULL_SIZE],
            valid: 0,
            invalid: None,
            src_eof: false,
            finished: false,
            err: None,
            err_yielded: false,
            type_: String::new(),
            columns: Vec::new(),
            directives: Vec::new(),
            limits,
            base,
            scan_pos: 0,
            row_start: None,
            hdr: HeaderScan {
                in_dataset: false,
                proto: 0,
                in_body: false,
                body: BraceMatch::new(),
            },
            waiting: false,
            wait_at: 0,
            wait_from: 0,
            scanned: 0,
            chained,
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
    /// or `@dataset`) that appeared before the `@dataset` header, each with
    /// the comments before it, as [`crate::parse`] files them. Stable for
    /// the lifetime of the reader.
    pub fn directives(&self) -> &[Directive] {
        &self.directives
    }

    /// True once the row sequence has been exhausted.
    pub fn done(&self) -> bool {
        self.finished
    }

    /// Read the next row. Returns `None` once the table's row sequence
    /// is exhausted: at the end of the input, or before the next directive
    /// (read it from [`Self::tail`]). Anything else that is not a
    /// well-formed row, a row cut short at the end of the input included,
    /// is an error, and it is the error [`crate::parse`] reports for the
    /// same input, at the same document position. An error is sticky, as
    /// protowire-go's `Next` is: after one, every later call returns the
    /// same error. The [`Iterator`] impl yields it once and then ends.
    pub fn next_row(&mut self) -> Option<Result<DatasetRow, PxfError>> {
        if let Some(e) = &self.err {
            return Some(Err(e.clone()));
        }
        if self.finished {
            return None;
        }
        if let Some((row, end)) = self.try_whole_row() {
            self.consume(end);
            return Some(Ok(row));
        }
        loop {
            let (kind, start, end) = self.scan_row();
            match kind {
                RowScan::Row => {
                    let row = self.parse_row(end);
                    self.consume(end);
                    return Some(row.map_err(|e| self.fail(e)));
                }
                // Parse the rest of the buffer, not just through the token
                // that stopped the scan: the lexer may reject a token before
                // its end (an oversized bytes literal), and cut there, it
                // would read as unterminated instead.
                RowScan::Bad => return Some(Err(self.fail_row())),
                RowScan::End => {
                    // The newlines and comments before the directive stay
                    // in the buffer, so the tail carries them: `parse` gives
                    // them to that directive as its leading comments.
                    self.finished = true;
                    return None;
                }
                RowScan::NotRow => {
                    let e =
                        PxfError::new(self.pos_at(start.unwrap_or(0)), ERR_DATASET_BESIDE_ENTRIES);
                    return Some(Err(self.fail(e)));
                }
                RowScan::Utf8 => return Some(Err(self.fail_utf8())),
                RowScan::NeedMore => {}
            }
            // At the end of the source, only whitespace and comments may
            // remain; anything else is a row cut short.
            if self.src_eof {
                if self.valid < self.buf().len() {
                    return Some(Err(self.fail_utf8()));
                }
                if start.is_some() {
                    return Some(Err(self.fail_row()));
                }
                self.consume(self.buf().len());
                self.finished = true;
                return None;
            }
            if let Err(e) = self.pull(STREAM_PULL_SIZE) {
                return Some(Err(self.fail(e)));
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

    /// The bytes the reader buffered but didn't consume, followed by the
    /// remaining bytes from the underlying source, with the document
    /// position they start at. Read the next table of a document with
    /// several `@dataset` directives with [`DatasetReader::from_tail`].
    ///
    /// MUST only be called after iteration has reported [`Self::done`].
    /// Calling earlier returns bytes the current reader still intends
    /// to consume, which will desync the next reader.
    pub fn tail(self) -> DatasetTail<R> {
        DatasetTail {
            inner: io::Cursor::new(self.pending[self.head..].to_vec()).chain(self.src),
            at: self.base,
        }
    }

    // ---- internals -------------------------------------------------------

    /// The unconsumed bytes.
    fn buf(&self) -> &[u8] {
        &self.pending[self.head..]
    }

    /// The fast path for a row already in the buffer: the row parser runs
    /// on the buffer directly, so the row is lexed once. If it parses a
    /// complete row, every token in it ended before its `)`, so the row is
    /// the one `parse` reads; it returns that row and the offset past it.
    /// Anything else returns `None`, and `scan_row` finds the boundary. It
    /// is tried once per row, before the scan starts on it, so a row that
    /// spans many reads is still lexed in linear time.
    fn try_whole_row(&self) -> Option<(DatasetRow, usize)> {
        let at = self.pos_at(self.scan_pos);
        let mut p = Parser::at(&self.buf()[self.scan_pos..self.valid], self.limits, at);
        if !matches!(p.current().kind, TokenKind::LParen) {
            return None;
        }
        let (row, end_offset) = p.parse_dataset_row(self.columns.len()).ok()?;
        Some((row, self.scan_pos + end_offset - at.offset))
    }

    /// Record `e` as the reader's sticky error and return it.
    fn fail(&mut self, e: PxfError) -> PxfError {
        self.err = Some(e.clone());
        e
    }

    fn fail_row(&mut self) -> PxfError {
        let e = self.row_error();
        self.fail(e)
    }

    /// The input is not UTF-8 where the scan needs it next. `parse` takes a
    /// `&str`, so it has no such input to agree with.
    fn fail_utf8(&mut self) -> PxfError {
        let e = PxfError::new(self.pos_at(self.valid), "input is not valid UTF-8");
        self.fail(e)
    }

    /// The document position of `buf[i]`.
    fn pos_at(&self, i: usize) -> Position {
        let mut p = self.base;
        for &ch in &self.buf()[..i] {
            if ch == b'\n' {
                p.line += 1;
                p.column = 1;
            } else {
                p.column += 1;
            }
        }
        p.offset += i;
        p
    }

    /// Drop the first `n` buffered bytes, advancing `base` past them.
    fn consume(&mut self, n: usize) {
        self.base = self.pos_at(n);
        self.head += n;
        self.valid -= n;
        self.invalid = self.invalid.map(|i| i - n);
        self.scan_pos = self.scan_pos.saturating_sub(n);
        self.row_start = self.row_start.and_then(|r| r.checked_sub(n));
        self.wait_at = self.wait_at.saturating_sub(n);
        self.wait_from = self.wait_from.saturating_sub(n);
    }

    /// Parse the row that ends at `buf[end]` with the parser `parse` uses,
    /// at its document position. The parser starts at `buf[0]`: the scan
    /// skipped only newlines and comments before the row's `(`, and those
    /// comments are the row's leading comments, as `parse` files them.
    fn parse_row(&self, end: usize) -> Result<DatasetRow, PxfError> {
        let mut p = Parser::at(&self.buf()[..end], self.limits, self.base);
        if !matches!(p.current().kind, TokenKind::LParen) {
            // `parse` reports any token here, a lexical error included, as a
            // top-level entry beside the dataset.
            return Err(PxfError::new(p.current().pos, ERR_DATASET_BESIDE_ENTRIES));
        }
        p.parse_dataset_row(self.columns.len()).map(|(row, _)| row)
    }

    /// Why the row being scanned is malformed. The scan has seen every
    /// token from the row's `(` up to the one that stopped it, and no `)`
    /// that closes the row, so the row parser fails on the buffer at the
    /// token where `parse` fails on the whole document.
    fn row_error(&self) -> PxfError {
        match self.parse_row(self.valid) {
            Err(e) => e,
            // Unreachable: the buffer has no closing ')'.
            Ok(_) => PxfError::new(self.pos_at(self.valid), "unexpected end of input"),
        }
    }

    /// Read up to `n` bytes from `src` into the buffer. Sets `src_eof` when
    /// `src` is exhausted. Fails only on a read failure, or when the buffer
    /// holds more than `max_message_size` bytes without a row boundary.
    fn pull(&mut self, n: usize) -> Result<(), PxfError> {
        if self.src_eof {
            return Ok(());
        }
        // Drop consumed bytes once they are half the buffer, so the copy
        // costs time linear in the input.
        if self.head > 0 && self.head * 2 >= self.pending.len() {
            self.pending.drain(..self.head);
            self.head = 0;
        }
        let n = n.min(self.scratch.len());
        let got = loop {
            match self.src.read(&mut self.scratch[..n]) {
                Ok(got) => break got,
                Err(e) if e.kind() == io::ErrorKind::Interrupted => continue,
                Err(e) => return Err(PxfError::new(Position::default(), e.to_string())),
            }
        };
        self.pending.extend_from_slice(&self.scratch[..got]);
        if got == 0 {
            self.src_eof = true;
            return Ok(());
        }
        self.validate();
        let held = self.buf().len();
        if held > self.limits.max_message_size {
            return Err(PxfError::new(
                Position::default(),
                format!(
                    "dataset input holds {} bytes without a row boundary, MaxMessageSize={}",
                    held, self.limits.max_message_size
                ),
            ));
        }
        Ok(())
    }

    /// Extend the valid UTF-8 prefix over the bytes just read. A character
    /// cut by the read stays outside it until its rest arrives.
    fn validate(&mut self) {
        if self.invalid.is_some() {
            return;
        }
        let from = self.valid;
        match std::str::from_utf8(&self.buf()[from..]) {
            Ok(_) => self.valid = self.buf().len(),
            Err(e) => {
                self.valid = from + e.valid_up_to();
                if e.error_len().is_some() {
                    self.invalid = Some(self.valid);
                }
            }
        }
    }

    /// The scan needs the bytes at `valid`, which can never be UTF-8.
    fn utf8_stuck(&self) -> bool {
        self.invalid.is_some() || (self.src_eof && self.valid < self.buf().len())
    }

    /// Pull bytes until the closing `)` of the `@dataset` column list, then
    /// parse what precedes it, the leading directives included, as `parse`
    /// parses a document.
    fn read_header(&mut self) -> Result<(), PxfError> {
        loop {
            let (end, found, bad) = self.scan_header();
            if let Some(bad) = bad {
                // The lexer rejected a token, or an @proto body can never
                // close, before the header ended. Every token before it is
                // complete, so `parse` fails at or before it.
                self.parse_header(self.valid)?;
                // Unreachable: `parse` reads what the scan read.
                return Err(PxfError::new(self.pos_at(bad.pos.offset), bad.value));
            }
            if found {
                // Parse the header prefix as a (rowless) PXF document, which
                // validates the leading directives, the @type / @dataset
                // conflict, the column paths, etc.
                let mut doc = self.parse_header(end)?;
                if doc.datasets.is_empty() {
                    // Should not happen: the scan found an @dataset.
                    return Err(PxfError::new(
                        Position::default(),
                        "pxf: no @dataset directive in stream",
                    ));
                }
                let ds = doc.datasets.swap_remove(0);
                self.type_ = ds.r#type;
                self.columns = ds.columns;
                if self.chained
                    && !doc.leading_comments.is_empty()
                    && doc
                        .directives
                        .first()
                        .is_some_and(|d| d.pos.offset < ds.pos.offset)
                {
                    // A reader on a tail starts mid-document, where the
                    // comments before a directive are its own, as `parse`
                    // files them; at a document's start they are the
                    // document's.
                    let mut lead = std::mem::take(&mut doc.leading_comments);
                    lead.append(&mut doc.directives[0].leading_comments);
                    doc.directives[0].leading_comments = lead;
                }
                self.directives = doc.directives;
                self.consume(end);
                return Ok(());
            }
            if self.src_eof || self.utf8_stuck() {
                // The input ended without a complete header. A malformed
                // document (a header cut short, a literal that never
                // closes) reports what `parse` reports for it; only a
                // well-formed one is a document without a dataset.
                self.parse_header(self.valid)?;
                if self.utf8_stuck() {
                    return Err(self.fail_utf8());
                }
                return Err(PxfError::new(
                    Position::default(),
                    "pxf: no @dataset directive in stream",
                ));
            }
            if self.buf().len() >= DEFAULT_HEADER_MAX_BYTES {
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

    /// Parse `buf[..end]`, which starts at the reader's base, as a
    /// document, the way `parse` does.
    fn parse_header(&self, end: usize) -> Result<Document, PxfError> {
        Parser::at(&self.buf()[..end], self.limits, self.base).parse_document()
    }

    /// Lex the token at `buf[from..]` and return it with the offset just
    /// past it, and whether it is decided: `false` when more input could
    /// still change it (the lexer looked for a byte at or past the end), or
    /// there is no token yet. Token offsets are into the buffer. This is the
    /// lexer `parse` uses, and it says itself when it ran out of input, so
    /// the scan and `parse` never disagree about where a token ends.
    fn next_token(&mut self, from: usize) -> (Token, usize, bool) {
        let mut lx = Lexer::at(
            &self.pending[self.head + from..self.head + self.valid],
            self.limits.max_bytes_literal_length,
            Position::with_offset(1, 1, from),
        );
        let tok = lx.next_token();
        let end = from + lx.input_pos();
        self.scanned += end - from;
        let decided = (self.src_eof && !self.utf8_stuck())
            || (!matches!(tok.kind, TokenKind::Eof) && !lx.saw_end());
        (tok, end, decided)
    }

    /// Whether a scan may lex on: no token is waiting, the source has
    /// ended, or the bytes read since the waiting token was last lexed
    /// could end it.
    fn resume(&mut self) -> bool {
        if self.waiting && !self.src_eof && !self.utf8_stuck() && !self.may_end_token() {
            self.wait_from = self.valid;
            return false;
        }
        self.waiting = false;
        true
    }

    /// Record `tok`, which `next_token` could not decide, as the waiting
    /// token. The scan resumes at its start.
    fn wait(&mut self, tok: &Token) {
        self.waiting = true;
        self.wait_at = tok.pos.offset;
        self.wait_from = self.valid;
        self.scan_pos = tok.pos.offset;
    }

    /// Find the next row with the lexer, resuming at `scan_pos`: a token
    /// known complete is never lexed again, so a row costs time linear in
    /// its size however many reads it arrives in. Between rows it skips
    /// newlines and comments; `(` starts a row, a directive ends the
    /// sequence, and anything else is not a row. Inside a row it stops at
    /// the `)` that closes it, at a `(` (no value holds one), or at a token
    /// the lexer rejects. Returns what it found, the row's or entry's
    /// start, and the end of a complete row.
    fn scan_row(&mut self) -> (RowScan, Option<usize>, usize) {
        if !self.resume() {
            return (RowScan::NeedMore, self.row_start, 0);
        }
        loop {
            let (tok, tok_end, decided) = self.next_token(self.scan_pos);
            if !decided {
                if self.utf8_stuck() {
                    return (RowScan::Utf8, self.row_start, 0);
                }
                self.wait(&tok);
                return (RowScan::NeedMore, self.row_start, 0);
            }
            if matches!(tok.kind, TokenKind::Eof) {
                // Only at the end of the source (next_token).
                return (RowScan::NeedMore, self.row_start, 0);
            }
            if matches!(tok.kind, TokenKind::Newline | TokenKind::Comment) {
                self.scan_pos = tok_end;
                continue;
            }
            let at = tok.pos.offset;
            let Some(start) = self.row_start else {
                match tok.kind {
                    TokenKind::LParen => {
                        self.row_start = Some(at);
                        self.scan_pos = tok_end;
                        continue;
                    }
                    TokenKind::AtType
                    | TokenKind::AtDirective
                    | TokenKind::AtDataset
                    | TokenKind::AtProto => return (RowScan::End, Some(at), 0),
                    _ => return (RowScan::NotRow, Some(at), 0),
                }
            };
            match tok.kind {
                TokenKind::RParen => {
                    self.row_start = None;
                    self.scan_pos = tok_end;
                    return (RowScan::Row, Some(start), tok_end);
                }
                TokenKind::LParen | TokenKind::Illegal => {
                    return (RowScan::Bad, Some(start), tok_end)
                }
                _ => {}
            }
            self.scan_pos = tok_end;
        }
    }

    /// Whether the bytes read since the waiting token was last lexed could
    /// end it: a closing quote for a string (one not escaped by an odd run
    /// of backslashes) or a newline, which a simple string cannot hold,
    /// `"""` for a triple-quoted one, a quote or newline for a bytes
    /// literal, or enough characters that the lexer refuses it, a newline
    /// for a line comment, `*/` for a block comment, a byte that ends a run
    /// or a comment opener for a timestamp, and for any other token a byte
    /// that cannot continue an identifier, number or duration. Each of
    /// those does end the token, so a token is lexed again only once it is
    /// short or decided, and the scan stays linear. It only decides when
    /// the lexer looks again, never what the token is.
    fn may_end_token(&self) -> bool {
        let buf = &self.buf()[..self.valid];
        let (at, from) = (self.wait_at, self.wait_from);
        if from - at < SHORT_WAIT {
            return true;
        }
        let tok = &buf[at..];
        let fresh = &buf[from..];
        let contains = |hay: &[u8], needle: &[u8]| hay.windows(needle.len()).any(|w| w == needle);
        if tok.starts_with(b"\"\"\"") {
            return contains(&buf[(from - 2).max(at + 3)..], b"\"\"\"");
        }
        if tok[0] == b'"' {
            for (i, &ch) in fresh.iter().enumerate() {
                if ch == b'\n' {
                    return true; // a simple string cannot hold one
                }
                if ch == b'"' {
                    let mut n = 0;
                    let mut j = from + i;
                    while j > at + 1 && buf[j - 1] == b'\\' {
                        n += 1;
                        j -= 1;
                    }
                    if n % 2 == 0 {
                        return true;
                    }
                }
            }
            return false;
        }
        if tok.starts_with(b"b\"") {
            // The lexer checks the cap before each character it reads past
            // the opening quote, the last one included.
            let read = buf.len() - 1 - (at + 2);
            return fresh.iter().any(|&c| c == b'"' || c == b'\n')
                || read / 4 * 3 > self.limits.max_bytes_literal_length;
        }
        if tok[0] == b'#' || tok.starts_with(b"//") {
            return fresh.contains(&b'\n');
        }
        if tok.starts_with(b"/*") {
            return contains(&buf[(from - 1).max(at + 2)..], b"*/");
        }
        if tok[..4].iter().all(u8::is_ascii_digit) && tok[4] == b'-' {
            // A timestamp: it runs to a byte that ends a run, or to `//` or
            // `/*`, whose '/' may be the last byte checked before.
            let mut i = (from - 1).max(at);
            while i < buf.len() {
                if ends_run(buf[i])
                    || (buf[i] == b'/'
                        && i + 1 < buf.len()
                        && (buf[i + 1] == b'/' || buf[i + 1] == b'*'))
                {
                    return true;
                }
                i += 1;
            }
            return false;
        }
        fresh
            .iter()
            .any(|&ch| !(is_ident_part(ch) || ch == b'+' || ch == b'-' || ch >= 0x80))
    }

    /// Find the end of the first `@dataset TYPE ( cols )` header with the
    /// lexer: the offset just past the `)` that closes the column list, the
    /// first `)` after an `@dataset` token. Returns `found = false` when the
    /// input ends first, and the token the lexer rejected, or an `@proto`
    /// body that can never close, before the header ended. Like `scan_row`
    /// it resumes at `scan_pos`.
    fn scan_header(&mut self) -> (usize, bool, Option<Token>) {
        if !self.hdr.in_body && !self.resume() {
            return (0, false, None);
        }
        loop {
            if self.hdr.in_body {
                let valid = self.valid;
                let (i, st) = self
                    .hdr
                    .body
                    .scan(&self.pending[self.head..self.head + valid], self.scan_pos);
                self.scanned += i - self.scan_pos;
                self.scan_pos = i;
                match st {
                    BraceState::Open => return (0, false, None),
                    BraceState::Failed => {
                        return (
                            0,
                            false,
                            Some(Token::new(
                                TokenKind::Illegal,
                                "@proto: unmatched '{'",
                                Position::with_offset(1, 1, i),
                            )),
                        )
                    }
                    BraceState::Closed => {}
                }
                self.hdr.in_body = false;
                self.scan_pos += 1; // the closing '}'
                continue;
            }
            let (tok, tok_end, decided) = self.next_token(self.scan_pos);
            if !decided {
                if !self.utf8_stuck() {
                    self.wait(&tok);
                }
                return (0, false, None);
            }
            let h = &mut self.hdr;
            match tok.kind {
                // Only at the end of the source (next_token).
                TokenKind::Eof => return (0, false, None),
                TokenKind::Illegal => return (0, false, Some(tok)),
                // `parse` skips these between @proto, its name and '{'.
                TokenKind::Newline | TokenKind::Comment => {}
                _ if h.in_dataset => {
                    if matches!(tok.kind, TokenKind::RParen) {
                        self.scan_pos = tok_end;
                        return (tok_end, true, None);
                    }
                }
                TokenKind::AtDataset => {
                    h.in_dataset = true;
                    h.proto = 0;
                }
                TokenKind::AtProto => h.proto = 1,
                TokenKind::Ident if h.proto == 1 => h.proto = 2,
                TokenKind::LBrace if h.proto > 0 => {
                    h.proto = 0;
                    h.in_body = true;
                    h.body = BraceMatch::new();
                }
                _ => h.proto = 0,
            }
            self.scan_pos = tok_end;
        }
    }
}

impl<R: Read> DatasetReader<DatasetTail<R>> {
    /// Read the next table of a document from another reader's
    /// [`DatasetReader::tail`]. Its rows and errors carry their document
    /// positions, and the comments before its leading directives are
    /// theirs, as [`crate::parse`] reads the whole document.
    pub fn from_tail(tail: DatasetTail<R>) -> Result<Self, PxfError> {
        Self::from_tail_with_limits(tail, Limits::default())
    }

    /// [`DatasetReader::from_tail`] under per-call [`Limits`].
    pub fn from_tail_with_limits(tail: DatasetTail<R>, limits: Limits) -> Result<Self, PxfError> {
        let at = tail.at;
        Self::build(tail, limits, at, true)
    }
}

/// A byte that ends a run of characters (a number, timestamp, duration or
/// identifier): protowire-go's `endsRun`.
fn ends_run(ch: u8) -> bool {
    matches!(
        ch,
        b' ' | b'\n' | b'\t' | b'\r' | b',' | b']' | b'}' | b')' | b'#'
    )
}

/// Rows until the end of the table. A row error is yielded once, and the
/// iterator ends after it: `next_row` keeps returning the error, as
/// protowire-go's `Next` does, which in a `for` loop that logs it and
/// carries on would never end (#132).
impl<R: Read> Iterator for DatasetReader<R> {
    type Item = Result<DatasetRow, PxfError>;
    fn next(&mut self) -> Option<Self::Item> {
        if self.err_yielded {
            return None;
        }
        let row = self.next_row();
        if matches!(row, Some(Err(_))) {
            self.err_yielded = true;
        }
        row
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
///
/// An error with no position is not a decode error: a schema violation, or
/// a validator's refusal, which also carries its report. It is returned
/// as it is, as protowire-go's `atRow` returns any error but its `*Error`.
fn at_row(e: PxfError, pos: Position, columns: &[String], spans: &[CellSpan]) -> PxfError {
    if !e.has_position() {
        return e;
    }
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
        let text = format_value(value);
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

#[cfg(test)]
mod tests {
    //! The scan's cost and its waiting, from protowire-go v1.17.0's
    //! `dataset_scan_linear_test.go`. These count the bytes the scan lexes
    //! (`scanned`) instead of timing it, so a slow machine cannot fail them.
    use super::*;
    use crate::parse_with_limits;
    use std::io::Cursor;

    struct OneByte<R>(R);

    impl<R: Read> Read for OneByte<R> {
        fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
            if buf.is_empty() {
                return Ok(0);
            }
            self.0.read(&mut buf[..1])
        }
    }

    /// A stream that stays open: it returns its writes one read each, then
    /// fails every later read, as a reader blocked on input would never
    /// return. A reader that reads past the writes was waiting for bytes
    /// that may never come.
    struct Open {
        writes: Vec<Vec<u8>>,
        next: usize,
    }

    impl Read for Open {
        fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
            let Some(w) = self.writes.get(self.next) else {
                return Err(io::Error::other("blocked: the stream is still open"));
            };
            assert!(w.len() <= buf.len());
            buf[..w.len()].copy_from_slice(w);
            self.next += 1;
            Ok(w.len())
        }
    }

    fn src(input: &str, one_byte: bool) -> Box<dyn Read> {
        let c = Cursor::new(input.as_bytes().to_vec());
        if one_byte {
            Box::new(OneByte(c))
        } else {
            Box::new(c)
        }
    }

    #[test]
    fn row_scan_is_linear() {
        type Shape = (&'static str, fn(usize) -> String);
        let shapes: [Shape; 9] = [
            // Many short tokens, no closing ')'.
            ("short tokens", |n| "1 ".repeat(n / 2)),
            // One token that never ends: each read used to re-lex it whole.
            ("unterminated string", |n| format!("\"{}", "a".repeat(n))),
            ("string of escaped quotes", |n| {
                format!("\"{}", "a\\\"".repeat(n / 3))
            }),
            ("unterminated triple string", |n| {
                format!("\"\"\"{}", "a\"\"b".repeat(n / 4))
            }),
            ("one long number", |n| "7".repeat(n)),
            ("line comment", |n| format!("1 # {}", "x".repeat(n))),
            ("unterminated block comment", |n| {
                format!("1 /* {}", "/ * x".repeat(n / 5))
            }),
            // A timestamp runs to a byte that ends a run; '=' and '/' are
            // not one, and must not make the scan lex it again.
            ("one long timestamp", |n| {
                format!("2026-10-23T{}", "0=/".repeat(n / 3))
            }),
            ("one long identifier", |n| {
                format!("a{}", "b.".repeat(n / 2))
            }),
        ];
        // protowire-go's sizes under -race: the count is what is checked,
        // and a quadratic scan already lexes about 8M bytes for a 256 KiB
        // row in 4 KiB reads.
        for (name, body) in shapes {
            for size in [256 << 10, 512 << 10] {
                for one_byte in [false, true] {
                    let input = format!("@dataset T (a)\n({}", body(size));
                    let mut tr = DatasetReader::new(src(&input, one_byte))
                        .unwrap_or_else(|e| panic!("{name}: {e}"));
                    tr.scanned = 0; // the row, not the header
                    assert!(
                        matches!(tr.next_row(), Some(Err(_))),
                        "{name}: an unclosed row was accepted"
                    );
                    // A token cut by a read is lexed again once; short
                    // tokens in one-byte reads come to about 1.5x.
                    let limit = 2 * input.len();
                    assert!(
                        tr.scanned <= limit,
                        "{name}, {size} bytes, one-byte reads {one_byte}: scanned {} bytes of {} (limit {limit})",
                        tr.scanned,
                        input.len()
                    );
                }
            }
        }
    }

    /// The header scan resumes too: a header near the 64 KiB budget, in
    /// one-byte reads, would otherwise be lexed from its start every read.
    #[test]
    fn header_scan_is_linear() {
        const N: usize = 60 << 10;
        for (name, header) in [
            (
                "short tokens",
                format!("@x {{ {}}}\n", "a = 1 ".repeat(N / 6)),
            ),
            (
                "one long string",
                format!("@x {{ k = \"{}\" }}\n", "a".repeat(N)),
            ),
            (
                "@proto body",
                format!("@proto {{ {}}}\n", "string a = 1; ".repeat(N / 14)),
            ),
        ] {
            let input = format!("{header}@dataset T (a)\n(1)\n");
            let tr =
                DatasetReader::new(src(&input, true)).unwrap_or_else(|e| panic!("{name}: {e}"));
            let limit = 2 * input.len();
            assert!(
                tr.scanned <= limit,
                "{name}: scanned {} bytes of {} (limit {limit})",
                tr.scanned,
                input.len()
            );
        }
    }

    /// A row whose ')' is the last byte read is returned at once: the scan
    /// must not wait for bytes that may never come (a live, append-only
    /// stream). Every byte of the header and row ends a read.
    #[test]
    fn complete_row_is_not_held_back() {
        for row in [
            "(1)",
            "(\"\")",
            "(\"a\\\"b\")",
            "(abc)",
            "(2026-10-23T00:00:00Z)",
            "(5s)",
            "(-1.5e3)",
            "(b\"QQ==\")",
            "({ k = [1, 2] })",
            "(\"\"\"\nx\n\"\"\")",
            "(1 /* c */)",
            "(1h30m)",
            "(1\u{b5}s)",
            "(-inf)",
            "(\"\\x41\\U0001F600\")",
            "(aaaaaaaaaaaa)",
        ] {
            let all = format!("@dataset T (a)\n{row}");
            let writes = all.bytes().map(|b| vec![b]).collect();
            let mut tr = DatasetReader::new(Open { writes, next: 0 })
                .unwrap_or_else(|e| panic!("{row}: {e}"));
            match tr.next_row() {
                Some(Ok(_)) => {}
                other => panic!("{row}: the complete row was not returned while the stream stayed open: {other:?}"),
            }
        }
    }

    /// A row the bytes read so far already show to be malformed fails at
    /// once, with `parse`'s error for those bytes, while the stream stays
    /// open.
    #[test]
    fn decided_error_is_not_held_back() {
        let limits = Limits {
            max_bytes_literal_length: 4,
            ..Limits::default()
        };
        for writes in [
            vec!["(~"],
            vec!["(1, ", "~"],
            vec!["(aaaaaaaaaaaa", "~"],
            vec!["(1h", ".", "~"],
            // The cap refuses a bytes literal before its closing quote.
            vec!["(b\"", "QUFBQUFBQUFBQUFBQUFB"],
            vec!["(b\"QUFB", "QUFBQUFBQUFBQUFB"],
        ] {
            let input = format!("@dataset T (a)\n{}", writes.concat());
            let want = parse_with_limits(&input, limits)
                .expect_err("parse accepted it")
                .to_string();
            let mut all = vec![b"@dataset T (a)\n".to_vec()];
            all.extend(writes.iter().map(|w| w.as_bytes().to_vec()));
            let mut tr = DatasetReader::with_limits(
                Open {
                    writes: all,
                    next: 0,
                },
                limits,
            )
            .unwrap();
            match tr.next_row() {
                Some(Err(e)) => assert_eq!(e.to_string(), want, "{writes:?}"),
                other => panic!("{writes:?}: got {other:?}, want parse's {want}"),
            }
        }
    }
}
