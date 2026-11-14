// SPDX-License-Identifier: MIT
// Copyright (c) 2026 TrendVidia, LLC.
//! Recursive-descent parser for PXF.
//!
//! Mirrors `protowire/encoding/pxf/parser.go`. Newlines and comments are
//! absorbed at the lexer-token boundary: comments accumulate in a pending
//! buffer and attach as `leading_comments` to the next entry, directive or
//! dataset row. A comment on the same line after an entry's value is that
//! entry's `trailing_comment`, and one that precedes nothing is the
//! `trailing_comments` of its block or document (protowire-go#279).

use crate::ast::{
    Assignment, BadVal, Block, BlockVal, BoolVal, BytesVal, Comment, DatasetDirective, DatasetRow,
    Directive, Document, DurationVal, Entry, FloatVal, IdentVal, IntVal, ListVal, MapEntry,
    NullVal, ProtoDirective, ProtoShape, StringVal, TimestampVal, TypeEntry, Value,
};
use crate::bigfloat::clip_literal;
use crate::dataset_paths::column_path_error;
use crate::decode::TYPE_ENTRY_MISPLACED;
use crate::errors::PxfError;
use crate::lexer::Lexer;
use crate::limits::Limits;
use crate::token::{Position, Token, TokenKind};

/// HARDENING.md `MaxNestingDepth`, re-exported from [`crate::limits`]: the
/// parser and the decoder cap `{` / `[` nesting at the same bound so
/// adversarial input can't overflow the native recursion stack regardless
/// of which entry point is used.
pub use crate::limits::MAX_NESTING_DEPTH;

/// Parse a PXF document into its AST under the default [`Limits`].
pub fn parse(input: &str) -> Result<Document, PxfError> {
    parse_with_limits(input, Limits::default())
}

/// Parse a PXF document into its AST under per-call [`Limits`]: the
/// input is refused past `max_message_size` before the first token is
/// read, nesting past `max_nesting_depth`, and a `b"…"` literal past
/// `max_bytes_literal_length`. The parser has no schema, so
/// `max_repeated_count` and `max_numeric_literal_digits` do not apply
/// here; they are the decoder's.
pub fn parse_with_limits(input: &str, limits: Limits) -> Result<Document, PxfError> {
    limits
        .check_message_size(input.len())
        .map_err(|m| PxfError::new(Position::new(1, 1), m))?;
    Parser::new(input, limits).parse_document()
}

/// Parse a PXF document in error-tolerant mode, for editor tooling: the
/// tree of what could be read, with every syntax error, in source order. A
/// port of protowire-go's `ParseTolerant`, whose recoveries it follows:
///
/// - A missing or malformed value (`key =` at the end of a line, `key`
///   alone) is a [`Value::Bad`] placeholder, so the entry, with its key and
///   position, is still in the tree.
/// - An unclosed block or list is closed at the end of input with what it
///   holds; one in a `@dataset` cell is closed at the row's `)`, so the rows
///   after it still parse.
/// - An unterminated string ends at its newline or at the end of input: a
///   newline in a simple string is taken as a literal being typed, and the
///   next line parses on.
/// - An unterminated triple-quoted string, block comment or bytes literal
///   ends at the end of input (a bytes literal at its newline).
/// - A malformed directive is skipped to the next directive or entry.
///
/// A document [`parse`] reads gives the same tree and no error. The tree
/// may hold placeholders and is meant for tooling (completion, hover,
/// diagnostics), not for decoding: use
/// [`parse`] or [`crate::unmarshal`] once the document is valid.
pub fn parse_tolerant(input: &str) -> (Document, Vec<PxfError>) {
    parse_tolerant_with_limits(input, Limits::default())
}

/// [`parse_tolerant`] under per-call [`Limits`], as [`parse_with_limits`]
/// applies them. An input past `max_message_size` gives an empty document
/// and that one error.
pub fn parse_tolerant_with_limits(input: &str, limits: Limits) -> (Document, Vec<PxfError>) {
    if let Err(m) = limits.check_message_size(input.len()) {
        return (
            Document::default(),
            vec![PxfError::new(Position::new(1, 1), m)],
        );
    }
    let mut p = Parser::unprimed(input.as_bytes(), limits, Position::with_offset(1, 1, 0));
    p.tolerant = true;
    p.lex.set_tolerant();
    p.advance();
    let doc = match p.parse_document() {
        Ok(doc) => doc,
        // Tolerant mode records every error it meets; kept should one
        // ever be returned.
        Err(e) => {
            p.errs.push(e);
            Document::default()
        }
    };
    let mut errs = p.errs;
    // Recorded in parse order, which is source order but for a lexer
    // error met during a lookahead; sorted so callers can rely on it.
    errs.sort_by_key(|e| e.pos.offset);
    (doc, errs)
}

/// Read one value with this parser from where a decoder stands: `current`
/// is the value's first token, and on return `lex` and `current` stand past
/// the value. The decoder reads what it does not bind this way (a field
/// `discard_unknown` drops, a directive's body), so skipping accepts only
/// what parsing accepts, rather than whatever balances its braces.
pub(crate) fn read_value(
    lex: &mut Lexer<'_>,
    current: &mut Token,
    depth: usize,
    limits: Limits,
) -> Result<Value, PxfError> {
    read_value_with(lex, current, depth, limits, false)
}

/// [`read_value`] for a value the decoder drops: a string in it that is not
/// UTF-8 is read as a string, since its text is never read (#170).
pub(crate) fn read_dropped_value(
    lex: &mut Lexer<'_>,
    current: &mut Token,
    depth: usize,
    limits: Limits,
) -> Result<Value, PxfError> {
    read_value_with(lex, current, depth, limits, true)
}

fn read_value_with(
    lex: &mut Lexer<'_>,
    current: &mut Token,
    depth: usize,
    limits: Limits,
    take_invalid_strings: bool,
) -> Result<Value, PxfError> {
    let mut p = Parser {
        lex: lex.clone(),
        current: std::mem::replace(current, Token::new(TokenKind::Eof, "", Position::new(1, 1))),
        pending: Vec::new(),
        prev_end: Position::new(1, 1),
        depth,
        limits,
        tolerant: false,
        errs: Vec::new(),
        take_invalid_strings,
    };
    let value = p.parse_value();
    lex.resume_after(&mut p.lex);
    *current = p.current;
    value
}

/// The error for anything after a dataset's rows that is neither a row nor
/// a directive. The streaming reader reports the same text.
pub(crate) const ERR_DATASET_BESIDE_ENTRIES: &str = "@dataset directive cannot coexist with top-level field entries; the document's payload is the @dataset rows (draft §3.4.4)";

/// A block's entries, the comments after the last of them, and where it
/// ends ([`Parser::parse_body`]).
type Body = (Vec<Entry>, Vec<Comment>, Position);

/// An entry's key, read: what every entry node holds of it.
struct EntryHead {
    pos: Position,
    key: String,
    quoted: bool,
    leading_comments: Vec<Comment>,
}

/// An entry as far as its value ([`Parser::entry_start`]).
enum EntryStart {
    /// Tolerant mode skipped a token that starts no entry.
    Skipped,
    /// An entry read whole: a block skipped past the limit, a dangling key.
    Whole(Box<Entry>),
    /// `key = ` (false) or `key: ` (true), at the value.
    Value(EntryHead, bool),
    /// `key {`, past the `{`, which stands at this position.
    Block(EntryHead, Position),
}

/// `head { body }`, once its body is read.
fn block_entry_read(
    head: EntryHead,
    body: Result<Body, PxfError>,
) -> Result<Option<Entry>, PxfError> {
    Ok(Some(block_entry(head, body?)))
}

/// `head { body }`.
fn block_entry(head: EntryHead, (entries, trailing_comments, end): Body) -> Entry {
    Entry::Block(Block {
        pos: head.pos,
        end,
        name: head.key,
        name_quoted: head.quoted,
        entries,
        leading_comments: head.leading_comments,
        trailing_comments,
    })
}

/// A block value at `pos`, once its body is read.
fn block_val_read(pos: Position, body: Result<Body, PxfError>) -> Result<Value, PxfError> {
    Ok(block_val(pos, body?))
}

/// A block value at `pos`.
fn block_val(pos: Position, (entries, trailing_comments, end): Body) -> Value {
    Value::Block(BlockVal {
        pos,
        end,
        entries,
        trailing_comments,
    })
}

pub(crate) struct Parser<'a> {
    lex: Lexer<'a>,
    current: Token,
    pending: Vec<Comment>,
    /// Where the last consumed token ended: a value's end once it is read,
    /// which tells a comment on its line from one on the next.
    prev_end: Position,
    /// Live `{` + `[` depth, incremented at each opening token and
    /// decremented at the matching close. Exceeds → reject before recursing.
    depth: usize,
    limits: Limits,
    /// protowire-go's tolerant mode, behind [`parse_tolerant`]: an error is
    /// recorded in `errs`, and the parser goes on from the nearest entry or
    /// block boundary. Off, every path is the strict parser's.
    tolerant: bool,
    errs: Vec<PxfError>,
    /// Read a string literal that is not UTF-8, with its bytes kept by the
    /// lexer, as a string: the decoder's reader of a value it drops, whose
    /// strings are never read (#170). Off, such a literal is the lexer's
    /// `Illegal` token, as it is for `parse`.
    take_invalid_strings: bool,
}

impl<'a> Parser<'a> {
    fn new(input: &'a str, limits: Limits) -> Self {
        Self::at(input.as_bytes(), limits, Position::with_offset(1, 1, 0))
    }

    /// A parser over `input`, a window of a document that starts at
    /// `start`, so positions and offsets stay document-relative
    /// (protowire-go's `newParserAt`). `input` is valid UTF-8; the
    /// streaming dataset reader parses a header or a row at a time.
    pub(crate) fn at(input: &'a [u8], limits: Limits, start: Position) -> Self {
        let mut p = Self::unprimed(input, limits, start);
        p.advance();
        p
    }

    /// [`Parser::at`] before its first token is read.
    fn unprimed(input: &'a [u8], limits: Limits, start: Position) -> Self {
        Self {
            lex: Lexer::at(input, limits.max_bytes_literal_length, start),
            current: Token::new(TokenKind::Eof, "", Position::new(1, 1)),
            pending: Vec::new(),
            prev_end: Position::new(1, 1),
            depth: 0,
            limits,
            tolerant: false,
            errs: Vec::new(),
            take_invalid_strings: false,
        }
    }

    /// A recoverable error: strict mode returns it, which ends the parse;
    /// tolerant mode records it, so the caller can recover (protowire-go's
    /// `soft`).
    fn soft(&mut self, e: PxfError) -> Result<(), PxfError> {
        if !self.tolerant {
            return Err(e);
        }
        self.errs.push(e);
        Ok(())
    }

    /// Whether a `{` or `[` at `pos` would nest past the limit. Strict mode
    /// refuses it; tolerant mode records the error and answers true, and the
    /// caller skips the whole construct.
    fn too_deep(&mut self, pos: Position) -> Result<bool, PxfError> {
        if self.depth < self.limits.max_nesting_depth {
            return Ok(false);
        }
        let e = PxfError::new(
            pos,
            format!(
                "nesting depth exceeds MaxNestingDepth={}",
                self.limits.max_nesting_depth
            ),
        );
        self.soft(e)?;
        Ok(true)
    }

    /// Consume the open token the parser stands on, and everything through
    /// its matching close (or to the end of input): tolerant recovery for a
    /// block or list too deep to descend into. Returns where the skipped
    /// construct ends: past its close, or at the end of input.
    fn skip_balanced(&mut self, open: TokenKind, close: TokenKind) -> Position {
        let mut depth = 0usize;
        loop {
            let k = self.current.kind;
            if k == open {
                depth += 1;
            } else if k == close {
                depth = depth.saturating_sub(1);
                if depth == 0 {
                    self.advance();
                    return self.prev_end;
                }
            } else if k == TokenKind::Eof {
                return self.current.pos;
            }
            self.advance();
        }
    }

    /// Whether the current token looks like the key of a new entry: a
    /// key-shaped token followed by `=`, `:` or `{`. Tolerant recovery reads
    /// it as the start of the next entry, not part of the construct being
    /// parsed.
    fn current_starts_entry(&mut self) -> bool {
        matches!(
            self.current.kind,
            TokenKind::Ident | TokenKind::String | TokenKind::Int | TokenKind::Bool
        ) && matches!(
            self.peek_kind(),
            TokenKind::Equals | TokenKind::Colon | TokenKind::LBrace
        )
    }

    /// Advance to the next construct that can start a document's part: a
    /// directive, a token that can start an entry, or the end. Tolerant
    /// recovery after a malformed directive.
    fn sync_top_level(&mut self) {
        while !matches!(
            self.current.kind,
            TokenKind::Eof
                | TokenKind::AtType
                | TokenKind::AtDirective
                | TokenKind::AtDataset
                | TokenKind::AtProto
                | TokenKind::Ident
                | TokenKind::String
                | TokenKind::Int
        ) {
            self.advance();
        }
    }

    /// Whether tolerant recovery closes the list or block being read at the
    /// current token: a `)` is in no value, so inside one it ends the
    /// `@dataset` row the value is a cell of. Strict mode never stops there;
    /// the `)` is its error.
    fn tolerant_row_close(&self) -> bool {
        self.tolerant && self.current.kind == TokenKind::RParen
    }

    fn enter(&mut self, pos: Position) -> Result<(), PxfError> {
        if self.depth >= self.limits.max_nesting_depth {
            return Err(PxfError::new(
                pos,
                format!(
                    "nesting depth exceeds MaxNestingDepth={}",
                    self.limits.max_nesting_depth
                ),
            ));
        }
        self.depth += 1;
        Ok(())
    }

    fn leave(&mut self) {
        self.depth -= 1;
    }

    /// Consume the next token, swallowing newlines and accumulating comments
    /// into the pending buffer so they can be attached to the following entry.
    fn advance(&mut self) {
        self.prev_end = self.lex.current_pos();
        loop {
            self.current = self.lex.next_token();
            if self.tolerant {
                let errs = self.lex.take_errs();
                self.errs.extend(errs);
            }
            match self.current.kind {
                TokenKind::Newline => continue,
                TokenKind::Comment => {
                    self.pending.push(Comment {
                        pos: self.current.pos,
                        text: std::mem::take(&mut self.current.value),
                    });
                    continue;
                }
                TokenKind::Illegal
                    if self.take_invalid_strings
                        && self.lex.at_invalid_string(self.current.pos) =>
                {
                    self.current.kind = TokenKind::String;
                    return;
                }
                _ => return,
            }
        }
    }

    /// protowire-go's `tokenErrMsg`: an illegal token is named by the
    /// lexer's diagnostic, any other by its kind and text.
    fn token_err_msg(&self) -> String {
        if matches!(self.current.kind, TokenKind::Illegal) {
            return self.current.value.clone();
        }
        format!("{} ({:?})", self.current.kind.name(), self.current.value)
    }

    fn flush_comments(&mut self) -> Vec<Comment> {
        std::mem::take(&mut self.pending)
    }

    /// Remove and return the text of a pending comment written after a
    /// value on the same source line (`restrict = true # deny-by-default`),
    /// as protowire-go's `takeTrailingComment` does; `end` is the value's
    /// end. A newline ends every line, so at most the first pending comment
    /// is one. Empty when that comment starts on a later line (it leads the
    /// next entry) or before the value.
    fn take_trailing_comment(&mut self, end: Position) -> String {
        match self.pending.first() {
            Some(c) if c.pos.line == end.line && c.pos.offset >= end.offset => {
                self.pending.remove(0).text
            }
            _ => String::new(),
        }
    }

    /// The token the parser stands on.
    pub(crate) fn current(&self) -> &Token {
        &self.current
    }

    pub(crate) fn parse_document(&mut self) -> Result<Document, PxfError> {
        let leading_comments = self.flush_comments();
        let mut type_url = String::new();
        let mut directives: Vec<Directive> = Vec::new();
        let mut datasets: Vec<DatasetDirective> = Vec::new();
        let mut protos: Vec<ProtoDirective> = Vec::new();
        let mut body_offset: usize = 0;

        // Top-of-document directive prelude. @type, @<name>, and @dataset may
        // interleave in any order; @type populates type_url, @<name>
        // appends to directives, @dataset appends to tables. body_offset
        // tracks the byte immediately after the last directive's last
        // token; stays 0 when no directives are present.
        let mut first_dataset_pos: Option<Position> = None;
        loop {
            match self.current.kind {
                TokenKind::AtType => {
                    self.advance(); // consume @type
                    let mut missing = !matches!(self.current.kind, TokenKind::Ident);
                    if !missing && self.tolerant && self.current_starts_entry() {
                        // A dangling `@type` before the body: an identifier
                        // followed by `=`, `:` or `{` is the first entry's
                        // key, not the type name.
                        missing = true;
                    }
                    if missing {
                        let e = PxfError::new(
                            self.current.pos,
                            format!(
                                "expected type name after @type, got {}",
                                self.current.kind.name()
                            ),
                        );
                        self.soft(e)?;
                        self.sync_top_level();
                        continue;
                    }
                    body_offset = self.current.pos.offset + self.current.value.len();
                    type_url = std::mem::take(&mut self.current.value);
                    self.advance();
                }
                // A malformed directive is skipped in tolerant mode, to the
                // next directive or entry.
                TokenKind::AtDirective => match self.parse_directive() {
                    Ok((d, end)) => {
                        directives.push(d);
                        body_offset = end;
                    }
                    Err(e) => {
                        self.soft(e)?;
                        self.sync_top_level();
                    }
                },
                TokenKind::AtDataset => match self.parse_dataset_directive() {
                    Ok((t, end)) => {
                        if first_dataset_pos.is_none() {
                            first_dataset_pos = Some(t.pos);
                        }
                        datasets.push(t);
                        body_offset = end;
                    }
                    Err(e) => {
                        self.soft(e)?;
                        self.sync_top_level();
                    }
                },
                TokenKind::AtProto => match self.parse_proto_directive() {
                    Ok((p, end)) => {
                        protos.push(p);
                        body_offset = end;
                    }
                    Err(e) => {
                        self.soft(e)?;
                        self.sync_top_level();
                    }
                },
                _ => break,
            }
        }

        // Standalone constraint (draft §3.4.4): a document with any
        // @dataset directive MUST NOT also carry top-level field entries.
        // Checked once every directive is read, as protowire-go checks
        // them: @type beside a dataset at the first dataset, an entry
        // beside one at the entry.
        if let Some(first) = first_dataset_pos {
            if !type_url.is_empty() {
                self.soft(PxfError::new(
                    first,
                    "@dataset directive cannot coexist with @type; the @dataset header declares the document's type (draft §3.4.4)",
                ))?;
            }
            if !matches!(self.current.kind, TokenKind::Eof) {
                // At the entry, as protowire-go reports it, and as the
                // streaming reader finds it.
                self.soft(PxfError::new(self.current.pos, ERR_DATASET_BESIDE_ENTRIES))?;
            }
        }

        let mut entries = Vec::new();
        while !matches!(self.current.kind, TokenKind::Eof) {
            // Top-level: only field_entry is allowed. The document represents
            // a proto message, never a map<K,V>; map_entry (`:` form) is
            // reserved for the inside of a '{ ... }' block. See
            // docs/grammar.ebnf -> document.
            // Tolerant mode gives no entry for a token it skipped.
            if let Some(entry) = self.parse_entry(false)? {
                entries.push(entry);
            }
        }
        // Comments after the last entry, directive or row.
        let trailing_comments = self.flush_comments();
        Ok(Document {
            type_url,
            directives,
            datasets,
            protos,
            body_offset,
            entries,
            leading_comments,
            trailing_comments,
        })
    }

    /// Returns the kind of the next significant token (skipping
    /// newlines and comments) without consuming it. Used by
    /// parse_directive to disambiguate prefix identifiers from body
    /// field keys.
    fn peek_kind(&mut self) -> TokenKind {
        let snap = self.lex.snapshot();
        let saved = self.current.clone();
        let saved_prev_end = self.prev_end;
        let n_comments = self.pending.len();
        // Tolerant lexing may record an error during the lookahead; it is
        // met again when the token is read.
        let n_errs = self.errs.len();
        self.advance();
        let k = self.current.kind;
        self.lex.restore(snap);
        self.current = saved;
        self.prev_end = saved_prev_end;
        self.pending.truncate(n_comments);
        self.errs.truncate(n_errs);
        k
    }

    /// Reads `@<name> *(<prefix-id>) [{ ... }]`. AT_DIRECTIVE is current
    /// on entry. Returns the directive plus the byte offset immediately
    /// past the directive's last token.
    fn parse_directive(&mut self) -> Result<(Directive, usize), PxfError> {
        let leading_comments = self.flush_comments();
        let at_pos = self.current.pos;
        let name = std::mem::take(&mut self.current.value);
        if crate::schema::is_future_reserved_directive(&name) {
            // Tolerant mode records it and reads the directive by the open
            // grammar, so the rest of the document still has a tree.
            self.soft(PxfError::new(
                at_pos,
                format!(
                    "@{} is a spec-reserved directive name with no v1 semantics (draft §3.4.6)",
                    name
                ),
            ))?;
        }
        let mut end_offset = at_pos.offset + 1 + name.len(); // `@` + name
        self.advance();

        let mut prefixes: Vec<String> = Vec::new();
        // Zero-or-more prefix identifiers; one-token lookahead
        // disambiguates an IDENT followed by `=` / `:` (body key) from
        // an IDENT followed by anything else (directive prefix).
        while matches!(self.current.kind, TokenKind::Ident) {
            let next = self.peek_kind();
            if matches!(next, TokenKind::Equals | TokenKind::Colon) {
                break;
            }
            end_offset = self.current.pos.offset + self.current.value.len();
            prefixes.push(std::mem::take(&mut self.current.value));
            self.advance();
        }

        // Back-compat: a single prefix populates the legacy `type`
        // field, matching v0.72.0's single-Type shape.
        let r#type = if prefixes.len() == 1 {
            prefixes[0].clone()
        } else {
            String::new()
        };

        let mut body: Vec<u8> = Vec::new();
        let mut has_body = false;
        if matches!(self.current.kind, TokenKind::LBrace) {
            let open = self.current.pos.offset - self.lex.base();
            // Use parse_block_val to validate inner content (string /
            // brace / comment well-formedness); then slice the raw
            // bytes between `{` and `}` from the input for body.
            self.parse_block_val()?;
            let input = self.lex.input_view();
            let close = find_matching_brace(input, open);
            if close < 0 {
                // parse_block_val succeeded so a matching brace must
                // exist — defensive belt-and-braces.
                return Err(PxfError::new(
                    at_pos,
                    format!("directive @{}: unmatched '{{'", name),
                ));
            }
            let close = close as usize;
            body = input[open + 1..close].to_vec();
            has_body = true;
            end_offset = self.lex.base() + close + 1;
        }

        Ok((
            Directive {
                pos: at_pos,
                name,
                prefixes,
                r#type,
                body,
                has_body,
                leading_comments,
            },
            end_offset,
        ))
    }

    /// Reads `@dataset <type> ( col1, col2, ... ) row*`. AT_TABLE is
    /// current on entry. Returns the table plus the byte offset
    /// immediately past the directive's last token.
    fn parse_dataset_directive(&mut self) -> Result<(DatasetDirective, usize), PxfError> {
        let leading_comments = self.flush_comments();
        let at_pos = self.current.pos;
        self.advance(); // consume @dataset

        // Optional row message type. MAY be omitted when an anonymous
        // @proto directive precedes the dataset (draft §3.4.4
        // Anonymous binding).
        let r#type = if matches!(self.current.kind, TokenKind::Ident) {
            let t = std::mem::take(&mut self.current.value);
            self.advance();
            t
        } else {
            String::new()
        };

        if !matches!(self.current.kind, TokenKind::LParen) {
            return Err(PxfError::new(
                self.current.pos,
                format!(
                    "expected '(' to start @dataset column list, got {}",
                    self.current.kind.name()
                ),
            ));
        }
        self.advance(); // consume (

        if !matches!(self.current.kind, TokenKind::Ident) {
            return Err(PxfError::new(
                self.current.pos,
                format!(
                    "@dataset column list must contain at least one field name, got {}",
                    self.current.kind.name()
                ),
            ));
        }
        let mut columns: Vec<String> = Vec::new();
        let mut column_pos: Vec<Position> = Vec::new();
        loop {
            if !matches!(self.current.kind, TokenKind::Ident) {
                return Err(PxfError::new(
                    self.current.pos,
                    format!(
                        "expected column field name, got {}",
                        self.current.kind.name()
                    ),
                ));
            }
            column_pos.push(self.current.pos);
            columns.push(std::mem::take(&mut self.current.value));
            self.advance();
            if matches!(self.current.kind, TokenKind::Comma) {
                self.advance();
                continue;
            }
            if matches!(self.current.kind, TokenKind::RParen) {
                break;
            }
            return Err(PxfError::new(
                self.current.pos,
                format!(
                    "expected ',' or ')' in @dataset column list, got {}",
                    self.current.kind.name()
                ),
            ));
        }
        if let Some((i, msg)) = column_path_error(&columns) {
            return Err(PxfError::new(column_pos[i], msg));
        }
        let mut end_offset = self.current.pos.offset + 1; // past `)`
        self.advance(); // consume )

        // Zero or more rows.
        let mut rows: Vec<DatasetRow> = Vec::new();
        while matches!(self.current.kind, TokenKind::LParen) {
            let (row, row_end) = self.parse_dataset_row(columns.len())?;
            rows.push(row);
            end_offset = row_end;
        }

        Ok((
            DatasetDirective {
                pos: at_pos,
                r#type,
                columns,
                rows,
                leading_comments,
            },
            end_offset,
        ))
    }

    /// Reads `( cell ( ',' cell )* )` with an arity check against
    /// `expected`. LPAREN is current on entry.
    ///
    /// The comments pending at the `(` and those inside the row are the
    /// row's `leading_comments`. Left pending, they would pass to the next
    /// row or directive, or to the document's end.
    pub(crate) fn parse_dataset_row(
        &mut self,
        expected: usize,
    ) -> Result<(DatasetRow, usize), PxfError> {
        let pos = self.current.pos;
        let mut leading_comments = self.flush_comments();
        self.advance(); // consume (

        let mut cells: Vec<Option<Value>> = Vec::with_capacity(expected);
        cells.push(self.parse_row_cell()?);
        while matches!(self.current.kind, TokenKind::Comma) {
            self.advance();
            cells.push(self.parse_row_cell()?);
        }
        if !matches!(self.current.kind, TokenKind::RParen) {
            return Err(PxfError::new(
                self.current.pos,
                format!(
                    "expected ',' or ')' in @dataset row, got {}",
                    self.current.kind.name()
                ),
            ));
        }
        let end_offset = self.current.pos.offset + 1;
        // Taken before the ')' is consumed: advance would add the comments
        // after it, which belong to what follows the row.
        leading_comments.append(&mut self.flush_comments());
        self.advance(); // consume )

        if cells.len() != expected {
            return Err(PxfError::new(
                pos,
                format!(
                    "@dataset row has {} cells, expected {} (column count)",
                    cells.len(),
                    expected
                ),
            ));
        }
        Ok((
            DatasetRow {
                pos,
                cells,
                leading_comments,
            },
            end_offset,
        ))
    }

    /// Reads `@proto <body>` (draft §3.4.5). AT_PROTO is current on
    /// entry. Four body shapes are lexically distinguished:
    ///
    ///   - anonymous:  `@proto { <message-body> }`
    ///   - named:      `@proto <dotted-name> { <message-body> }`
    ///   - source:     `@proto """<proto-source>"""`
    ///   - descriptor: `@proto b"<base64-FileDescriptorSet>"`
    ///
    /// For the brace-bounded shapes the body is sliced as raw bytes
    /// between `{` and the matching `}` (both exclusive); the
    /// contents are protobuf source and are NOT decoded as PXF.
    fn parse_proto_directive(&mut self) -> Result<(ProtoDirective, usize), PxfError> {
        let leading_comments = self.flush_comments();
        let at_pos = self.current.pos;
        self.advance(); // consume @proto

        match self.current.kind {
            TokenKind::LBrace => {
                let (body, end_offset) = self.capture_brace_body("@proto (anonymous form)")?;
                Ok((
                    ProtoDirective {
                        pos: at_pos,
                        shape: ProtoShape::Anonymous,
                        type_name: String::new(),
                        body,
                        leading_comments,
                    },
                    end_offset,
                ))
            }
            TokenKind::Ident => {
                let type_name = std::mem::take(&mut self.current.value);
                self.advance();
                if !matches!(self.current.kind, TokenKind::LBrace) {
                    return Err(PxfError::new(
                        self.current.pos,
                        format!(
                            "expected '{{' after @proto {}, got {}",
                            type_name,
                            self.current.kind.name()
                        ),
                    ));
                }
                let (body, end_offset) =
                    self.capture_brace_body(&format!("@proto {}", type_name))?;
                Ok((
                    ProtoDirective {
                        pos: at_pos,
                        shape: ProtoShape::Named,
                        type_name,
                        body,
                        leading_comments,
                    },
                    end_offset,
                ))
            }
            TokenKind::String => {
                let body = std::mem::take(&mut self.current.value).into_bytes();
                let end_offset = self.current.pos.offset + body.len();
                self.advance();
                Ok((
                    ProtoDirective {
                        pos: at_pos,
                        shape: ProtoShape::Source,
                        type_name: String::new(),
                        body,
                        leading_comments,
                    },
                    end_offset,
                ))
            }
            TokenKind::Bytes => {
                let raw = std::mem::take(&mut self.current.value);
                let decoded = crate::go_strconv::decode_base64_descriptor(&raw).map_err(|e| {
                    PxfError::new(
                        self.current.pos,
                        format!("@proto descriptor body: invalid base64: {e}"),
                    )
                })?;
                let end_offset = self.current.pos.offset + raw.len() + 3; // b" … "
                self.advance();
                Ok((
                    ProtoDirective {
                        pos: at_pos,
                        shape: ProtoShape::Descriptor,
                        type_name: String::new(),
                        body: decoded,
                        leading_comments,
                    },
                    end_offset,
                ))
            }
            _ => Err(PxfError::new(
                self.current.pos,
                format!(
                    "expected '{{', dotted identifier, triple-quoted string, or b\"...\" after @proto, got {}",
                    self.current.kind.name()
                ),
            )),
        }
    }

    /// Slice raw bytes between `{` and the matching `}` (both
    /// exclusive) without decoding the body as PXF entries. LBrace is
    /// current on entry. Repositions the lexer past the closing brace.
    fn capture_brace_body(&mut self, label: &str) -> Result<(Vec<u8>, usize), PxfError> {
        let open = self.current.pos.offset - self.lex.base();
        let close = find_matching_brace(self.lex.input_view(), open);
        if close < 0 {
            // At the '{', as protowire-go reports it.
            return Err(PxfError::new(
                self.current.pos,
                format!("{}: unmatched '{{'", label),
            ));
        }
        let close = close as usize;
        let body = self.lex.input_view()[open + 1..close].to_vec();
        self.lex.reposition_to(close + 1);
        self.advance();
        Ok((body, self.lex.base() + close + 1))
    }

    /// Consumes one cell of a @dataset row. Returns `None` for an empty
    /// cell (no value between two commas, or at row start/end). A cell
    /// holds any value, list and block included (draft §3.4.4; protowire#341).
    fn parse_row_cell(&mut self) -> Result<Option<Value>, PxfError> {
        match self.current.kind {
            TokenKind::Comma | TokenKind::RParen => Ok(None),
            _ => {
                // A block cell's entries flush pending comments as their
                // leading comments. Comments before the cell belong to no
                // entry of it, so set them aside: the cell keeps only the
                // comments written inside it.
                let pending = std::mem::take(&mut self.pending);
                let v = self.parse_value();
                let inside = std::mem::replace(&mut self.pending, pending);
                self.pending.extend(inside);
                Ok(Some(v?))
            }
        }
    }

    /// `allow_map_entry` gates the `:` (map-entry) form: false at document
    /// top level, true inside any '{ ... }' block.
    ///
    /// Tolerant mode gives `None` for a token that cannot start an entry,
    /// which it skips.
    /// One entry. Its key and what follows it are read by
    /// [`Self::entry_start`], and its node is built after its value, both
    /// out of line: an unoptimized build gives every temporary its own stack
    /// slot, and here each would cost every level of a nested document
    /// (#179). Each helper split out of a recursive reader this way is
    /// inlined back into it in an optimized build, which shares slots
    /// between temporaries and where the call would cost time.
    fn parse_entry(&mut self, allow_map_entry: bool) -> Result<Option<Entry>, PxfError> {
        match self.entry_start(allow_map_entry) {
            Err(e) => Err(e),
            Ok(EntryStart::Skipped) => Ok(None),
            Ok(EntryStart::Whole(entry)) => Ok(Some(*entry)),
            Ok(EntryStart::Value(head, map)) => {
                let value = self.parse_value();
                self.value_entry(head, map, value)
            }
            Ok(EntryStart::Block(head, open_pos)) => {
                let body = self.parse_body(open_pos);
                block_entry_read(head, body)
            }
        }
    }

    /// An entry as far as its value or body.
    #[cfg_attr(not(debug_assertions), inline(always))]
    fn entry_start(&mut self, allow_map_entry: bool) -> Result<EntryStart, PxfError> {
        let leading_comments = self.flush_comments();
        let pos = self.current.pos;
        let k = self.current.kind;

        // map-key = identifier / string / integer / bool (draft -01
        // §abnf-grammar; the keyword spelling landed in protowire#284). A
        // bool key takes only the ':' tail, exactly as an integer key does.
        if !matches!(
            k,
            TokenKind::Ident | TokenKind::String | TokenKind::Int | TokenKind::Bool
        ) {
            let e = PxfError::new(
                pos,
                format!(
                    "expected identifier, string, integer, or bool, got {}",
                    self.token_err_msg()
                ),
            );
            if !self.tolerant {
                return Err(e);
            }
            // Skip the token; the caller's loop tries the next one. The
            // leading comments go back, to attach to the next entry.
            self.errs.push(e);
            let mut pending = leading_comments;
            pending.append(&mut self.pending);
            self.pending = pending;
            self.advance();
            return Ok(EntryStart::Skipped);
        }
        let key_kind = k;
        let key = std::mem::take(&mut self.current.value);
        // Only a numeric literal carries a suffix. Each tail below decides
        // what one on a key means, as protowire-go's parseEntry does.
        let key_prec = std::mem::take(&mut self.current.prec);
        self.advance();

        match self.current.kind {
            TokenKind::Equals => {
                // `=` denotes a field assignment on a proto message: the key
                // is an identifier, or a string — a quoted entry name, valid
                // only in a keyed repeated field's block, which the schema
                // layer checks (draft -01 §3.13). Integer and bool keys are
                // only valid with `:`.
                if !matches!(key_kind, TokenKind::Ident | TokenKind::String) {
                    // Tolerant mode keeps it as an assignment.
                    self.soft(non_name_assign_error(pos, key_kind, &key, &key_prec))?;
                }
                self.advance();
                Ok(EntryStart::Value(
                    EntryHead {
                        pos,
                        key,
                        quoted: matches!(key_kind, TokenKind::String),
                        leading_comments,
                    },
                    false,
                ))
            }
            TokenKind::Colon => {
                // Map entry. Only allowed inside a '{ ... }' block, never at
                // document top level.
                if !allow_map_entry {
                    // Tolerant mode keeps it as a map entry.
                    self.soft(top_level_map_entry_error(pos))?;
                }
                // map-key = identifier / string / integer / bool: an integer
                // key takes no precision suffix (draft -01 ABNF,
                // protowire#406). MapEntry.key has nowhere to keep one.
                if !key_prec.is_empty() {
                    self.soft(PxfError::new(
                        pos,
                        format!(
                            "map key {}: a key cannot carry a precision suffix (p{})",
                            key,
                            clip_literal(&key_prec)
                        ),
                    ))?;
                }
                self.advance();
                Ok(EntryStart::Value(
                    EntryHead {
                        pos,
                        key,
                        quoted: matches!(key_kind, TokenKind::String),
                        leading_comments,
                    },
                    true,
                ))
            }
            TokenKind::LBrace => {
                // `{ ... }` denotes a submessage field, or a named entry of a
                // keyed repeated field; same identifier-or-string rule as `=`.
                if !matches!(key_kind, TokenKind::Ident | TokenKind::String) {
                    self.soft(non_name_block_error(pos, key_kind, &key, &key_prec))?;
                }
                let open_pos = self.current.pos;
                let head = EntryHead {
                    pos,
                    key,
                    quoted: matches!(key_kind, TokenKind::String),
                    leading_comments,
                };
                if self.too_deep(open_pos)? {
                    // Too deep to descend: skip the whole block, keep the
                    // entry.
                    let end = self.skip_balanced(TokenKind::LBrace, TokenKind::RBrace);
                    return Ok(EntryStart::Whole(Box::new(block_entry(
                        head,
                        (Vec::new(), Vec::new(), end),
                    ))));
                }
                self.advance();
                Ok(EntryStart::Block(head, open_pos))
            }
            other => {
                self.soft(PxfError::new(
                    self.current.pos,
                    format!(
                        "expected '=', ':', or '{{' after {:?}, got {}",
                        key,
                        other.name()
                    ),
                ))?;
                // A dangling key, the shape completion fires on: kept as an
                // assignment with a placeholder just past the key, where the
                // value belongs. The current token, most likely the next
                // entry's key, is left.
                let at = self.prev_end;
                Ok(EntryStart::Whole(Box::new(Entry::Assignment(Assignment {
                    pos,
                    end: at,
                    key,
                    key_quoted: false,
                    value: Value::Bad(BadVal { pos: at, end: at }),
                    leading_comments,
                    trailing_comment: String::new(),
                }))))
            }
        }
    }

    /// `head = value` or `head: value`, once its value is read.
    #[cfg_attr(not(debug_assertions), inline(always))]
    fn value_entry(
        &mut self,
        head: EntryHead,
        map: bool,
        value: Result<Value, PxfError>,
    ) -> Result<Option<Entry>, PxfError> {
        let value = value?;
        Ok(Some(if map {
            self.map_entry(head, value)
        } else {
            self.assignment(head, value)
        }))
    }

    /// `head = value`, with the comment that trails it.
    #[cfg_attr(not(debug_assertions), inline(always))]
    fn assignment(&mut self, head: EntryHead, value: Value) -> Entry {
        let end = value.end();
        let trailing_comment = self.take_trailing_comment(end);
        Entry::Assignment(Assignment {
            pos: head.pos,
            end,
            key: head.key,
            key_quoted: head.quoted,
            value,
            leading_comments: head.leading_comments,
            trailing_comment,
        })
    }

    /// `head: value`, with the comment that trails it.
    #[cfg_attr(not(debug_assertions), inline(always))]
    fn map_entry(&mut self, head: EntryHead, value: Value) -> Entry {
        let end = value.end();
        let trailing_comment = self.take_trailing_comment(end);
        Entry::MapEntry(MapEntry {
            pos: head.pos,
            end,
            key: head.key,
            quoted: head.quoted,
            value,
            leading_comments: head.leading_comments,
            trailing_comment,
        })
    }

    /// Reads one value. Each value ends where its last token does, which
    /// `advance` leaves in `prev_end`; a placeholder spans nothing. A list
    /// or block is read here and anything else out of line, so the frames
    /// a nested value stacks stay small (#179).
    fn parse_value(&mut self) -> Result<Value, PxfError> {
        // Neither opener starts an entry, so the tolerant check for a
        // missing value, in parse_scalar_value, never meets one.
        match self.current.kind {
            TokenKind::LBracket => self.parse_list(),
            TokenKind::LBrace => self.parse_block_val(),
            _ => self.parse_scalar_value(),
        }
    }

    /// A value that is not a list or block: a single token.
    #[cfg_attr(not(debug_assertions), inline(always))]
    fn parse_scalar_value(&mut self) -> Result<Value, PxfError> {
        let pos = self.current.pos;

        if self.tolerant && self.current_starts_entry() {
            // The current token is the next entry's key, and this value is
            // missing (a dangling `key =`). It is left, and the placeholder
            // and the error go just past the separator, where the value
            // belongs.
            let at = self.prev_end;
            self.errs.push(PxfError::new(at, "missing value"));
            return Ok(Value::Bad(BadVal { pos: at, end: at }));
        }

        match self.current.kind {
            TokenKind::String => {
                let value = std::mem::take(&mut self.current.value);
                self.advance();
                Ok(Value::String(StringVal {
                    pos,
                    end: self.prev_end,
                    value,
                }))
            }
            TokenKind::Int => {
                let raw = std::mem::take(&mut self.current.value);
                let prec = std::mem::take(&mut self.current.prec);
                self.advance();
                Ok(Value::Int(IntVal {
                    pos,
                    end: self.prev_end,
                    raw,
                    prec,
                }))
            }
            TokenKind::Float => {
                let raw = std::mem::take(&mut self.current.value);
                let prec = std::mem::take(&mut self.current.prec);
                self.advance();
                Ok(Value::Float(FloatVal {
                    pos,
                    end: self.prev_end,
                    raw,
                    prec,
                }))
            }
            TokenKind::Bool => {
                let value = self.current.value == "true";
                self.advance();
                Ok(Value::Bool(BoolVal {
                    pos,
                    end: self.prev_end,
                    value,
                }))
            }
            TokenKind::Bytes => {
                // The strict lexer took it, so one of the four encodings
                // decodes it; the tolerant lexer leaves the check here.
                let decoded = match crate::go_strconv::decode_base64_lenient(&self.current.value) {
                    Ok(decoded) => decoded,
                    Err(e) if self.tolerant => {
                        self.errs
                            .push(PxfError::new(pos, format!("invalid base64: {e}")));
                        self.advance();
                        return Ok(Value::Bad(BadVal { pos, end: pos }));
                    }
                    Err(_) => Vec::new(),
                };
                self.advance();
                Ok(Value::Bytes(BytesVal {
                    pos,
                    end: self.prev_end,
                    value: decoded,
                }))
            }
            TokenKind::Timestamp => {
                let raw = std::mem::take(&mut self.current.value);
                self.advance();
                Ok(Value::Timestamp(TimestampVal {
                    pos,
                    end: self.prev_end,
                    raw,
                }))
            }
            TokenKind::Duration => {
                let raw = std::mem::take(&mut self.current.value);
                self.advance();
                Ok(Value::Duration(DurationVal {
                    pos,
                    end: self.prev_end,
                    raw,
                }))
            }
            TokenKind::Null => {
                self.advance();
                Ok(Value::Null(NullVal {
                    pos,
                    end: self.prev_end,
                }))
            }
            TokenKind::Ident => {
                let name = std::mem::take(&mut self.current.value);
                self.advance();
                Ok(Value::Ident(IdentVal {
                    pos,
                    end: self.prev_end,
                    name,
                }))
            }
            // An illegal token is reported as the lexer's own diagnostic, as
            // the decoder reports it at every value position and as
            // protowire-go's parseValue does: "expected value" adds nothing
            // to "invalid duration: 5seconds".
            kind => {
                let e = if kind == TokenKind::Illegal {
                    PxfError::new(pos, self.current.value.clone())
                } else {
                    PxfError::new(pos, format!("expected value, got {}", self.token_err_msg()))
                };
                if !self.tolerant {
                    return Err(e);
                }
                self.errs.push(e);
                // A closer is left for the construct it closes; a `)` closes
                // the `@dataset` row around an unclosed cell.
                if !matches!(
                    kind,
                    TokenKind::Eof | TokenKind::RBrace | TokenKind::RBracket | TokenKind::RParen
                ) {
                    self.advance();
                }
                Ok(Value::Bad(BadVal { pos, end: pos }))
            }
        }
    }

    fn parse_list(&mut self) -> Result<Value, PxfError> {
        let pos = self.current.pos;
        if self.depth >= self.limits.max_nesting_depth {
            return self.list_past_limit(pos);
        }
        self.enter(pos)?;
        self.advance(); // consume [
        let mut elements = Vec::new();
        while !matches!(self.current.kind, TokenKind::RBracket | TokenKind::Eof)
            && !self.tolerant_row_close()
        {
            let before = self.current.pos.offset;
            let elem = self.parse_value();
            if !self.push_element(&mut elements, before, elem)? {
                break;
            }
        }
        self.list_close(pos, elements)
    }

    /// A list at `pos` past the limit: refused, or in tolerant mode skipped
    /// whole.
    fn list_past_limit(&mut self, pos: Position) -> Result<Value, PxfError> {
        self.too_deep(pos)?;
        let end = self.skip_balanced(TokenKind::LBracket, TokenKind::RBracket);
        Ok(Value::List(ListVal {
            pos,
            end,
            elements: Vec::new(),
        }))
    }

    /// Adds a list element read from `before`, and its comma; false if the
    /// list ends at it.
    #[cfg_attr(not(debug_assertions), inline(always))]
    fn push_element(
        &mut self,
        elements: &mut Vec<Value>,
        before: usize,
        elem: Result<Value, PxfError>,
    ) -> Result<bool, PxfError> {
        let elem = elem?;
        if self.tolerant && self.current.pos.offset == before && matches!(elem, Value::Bad(_)) {
            // The value recovered without consuming anything (a `}`
            // that closes an enclosing block, a token that starts the
            // next entry): leave the list, so the construct around it
            // can recover.
            return Ok(false);
        }
        elements.push(elem);
        if matches!(self.current.kind, TokenKind::Comma) {
            self.advance();
        }
        Ok(true)
    }

    /// A list's `]`, and the list opened at `pos`.
    #[cfg_attr(not(debug_assertions), inline(always))]
    fn list_close(&mut self, pos: Position, elements: Vec<Value>) -> Result<Value, PxfError> {
        if !matches!(self.current.kind, TokenKind::RBracket) {
            // Tolerant mode closes the list here, with what it holds.
            self.soft(PxfError::new(
                self.current.pos,
                format!("expected ']', got {}", self.current.kind.name()),
            ))?;
            self.leave();
            return Ok(Value::List(ListVal {
                pos,
                end: self.current.pos,
                elements,
            }));
        }
        self.advance();
        self.leave();
        Ok(Value::List(ListVal {
            pos,
            end: self.prev_end,
            elements,
        }))
    }

    fn parse_block_val(&mut self) -> Result<Value, PxfError> {
        let pos = self.current.pos;
        if self.depth >= self.limits.max_nesting_depth {
            return self.block_past_limit(pos);
        }
        self.advance(); // consume {
        let body = self.parse_body(pos);
        block_val_read(pos, body)
    }

    /// A block value at `pos` past the limit: refused, or in tolerant mode
    /// skipped whole.
    fn block_past_limit(&mut self, pos: Position) -> Result<Value, PxfError> {
        self.too_deep(pos)?;
        let end = self.skip_balanced(TokenKind::LBrace, TokenKind::RBrace);
        Ok(block_val(pos, (Vec::new(), Vec::new(), end)))
    }

    /// Reads a block's entries up to and through its `}`. Returns them, the
    /// comments after the last of them, before the `}`, and where the block
    /// ends: past its `}`, or where tolerant mode closed it.
    fn parse_body(&mut self, open_pos: Position) -> Result<Body, PxfError> {
        self.enter(open_pos)?;
        let mut entries = Vec::new();
        while !matches!(self.current.kind, TokenKind::RBrace | TokenKind::Eof)
            && !self.tolerant_row_close()
        {
            // Tolerant mode gives no entry for what it skipped. Inside a
            // '{ ... }' block both forms are accepted; the schema layer
            // disambiguates submessage vs map<K,V>.
            let entry = if matches!(self.current.kind, TokenKind::AtType) {
                self.parse_type_entry(entries.is_empty())
            } else {
                self.parse_entry(true)
            };
            if let Some(entry) = entry? {
                entries.push(entry);
            }
        }
        self.body_close(entries)
    }

    /// A block's `}`: its entries, the comments before the `}`, and where
    /// the block ends.
    #[cfg_attr(not(debug_assertions), inline(always))]
    fn body_close(&mut self, entries: Vec<Entry>) -> Result<Body, PxfError> {
        // Taken before the '}' is consumed: advance would add the comments
        // after it, which belong to what follows the block.
        let trailing = self.flush_comments();
        let end = self.block_end()?;
        Ok((entries, trailing, end))
    }

    /// A block's `}`, and where the block ends.
    #[cfg_attr(not(debug_assertions), inline(always))]
    fn block_end(&mut self) -> Result<Position, PxfError> {
        if !matches!(self.current.kind, TokenKind::RBrace) {
            if !self.tolerant {
                return Err(PxfError::new(
                    self.current.pos,
                    format!("expected '}}', got {}", self.current.kind.name()),
                ));
            }
            // An unclosed block is closed at the end of input, or at the
            // `)` of the `@dataset` row it is a cell of, with what it holds.
            let msg = if self.current.kind == TokenKind::RParen {
                "unclosed block: expected '}' before ')'"
            } else {
                "unclosed block: expected '}' before end of input"
            };
            self.errs.push(PxfError::new(self.current.pos, msg));
            self.leave();
            return Ok(self.current.pos);
        }
        self.advance();
        self.leave();
        Ok(self.prev_end)
    }

    /// Reads `@type = "<type-url>"`, the type entry of a
    /// `google.protobuf.Any` block (draft -01 "Any Blocks"). `first` says
    /// whether it opens its block, the only place it may stand. A parser
    /// can't tell which blocks are Any, so it admits one in any block and
    /// the schema layer refuses the rest.
    ///
    /// Tolerant mode records a misplaced or malformed type entry, skips what
    /// is there of it, and gives `None`.
    fn parse_type_entry(&mut self, first: bool) -> Result<Option<Entry>, PxfError> {
        let leading_comments = self.flush_comments();
        let pos = self.current.pos;
        if !first {
            let e = PxfError::new(pos, TYPE_ENTRY_MISPLACED);
            return self.type_entry_failed(e, leading_comments);
        }
        self.advance(); // consume @type
        if !matches!(self.current.kind, TokenKind::Equals) {
            let e = PxfError::new(
                self.current.pos,
                format!("expected '=' after @type, got {}", self.token_err_msg()),
            );
            return self.type_entry_failed(e, leading_comments);
        }
        self.advance();
        if !matches!(self.current.kind, TokenKind::String) {
            let e = PxfError::new(
                self.current.pos,
                format!(
                    "expected a string type URL after @type =, got {}",
                    self.token_err_msg()
                ),
            );
            return self.type_entry_failed(e, leading_comments);
        }
        let url = std::mem::take(&mut self.current.value);
        self.advance();
        let end = self.prev_end;
        let trailing_comment = self.take_trailing_comment(end);
        Ok(Some(Entry::TypeEntry(TypeEntry {
            pos,
            end,
            url,
            leading_comments,
            trailing_comment,
        })))
    }

    /// A type entry's error: strict mode returns it. Tolerant mode records
    /// it, gives the leading comments back to the next entry, and skips what
    /// is left of the entry, `@type`, `=` and the string, as far as each is
    /// there.
    fn type_entry_failed(
        &mut self,
        e: PxfError,
        leading_comments: Vec<Comment>,
    ) -> Result<Option<Entry>, PxfError> {
        if !self.tolerant {
            return Err(e);
        }
        self.errs.push(e);
        let mut pending = leading_comments;
        pending.append(&mut self.pending);
        self.pending = pending;
        for k in [TokenKind::AtType, TokenKind::Equals, TokenKind::String] {
            if self.current.kind == k {
                self.advance();
            }
        }
        Ok(None)
    }
}

/// An integer or bool key before `=`: protowire-go's `nonNameAssignError`.
/// The parser and both decoders give it whatever the schema and options say,
/// `discard_unknown` included, since no field has such a name.
pub(crate) fn non_name_assign_error(
    pos: Position,
    kind: TokenKind,
    key: &str,
    prec: &str,
) -> PxfError {
    PxfError::new(
        pos,
        format!(
            "field assignment with '=' requires an identifier or string key, got {} ({:?}); use ':' for map entries",
            kind.name(),
            written_key(key, prec)
        ),
    )
}

/// An integer or bool key before `{`: protowire-go's `nonNameBlockError`.
pub(crate) fn non_name_block_error(
    pos: Position,
    kind: TokenKind,
    key: &str,
    prec: &str,
) -> PxfError {
    PxfError::new(
        pos,
        format!(
            "submessage block requires an identifier or string key, got {} ({:?})",
            kind.name(),
            written_key(key, prec)
        ),
    )
}

/// A `:` entry at the top level of a document: protowire-go's
/// `topLevelMapEntryError`, from the parser and the decoders alike.
pub(crate) fn top_level_map_entry_error(pos: Position) -> PxfError {
    PxfError::new(
        pos,
        "map entry (':' form) is only allowed inside a '{ … }' block; use '=' for top-level field assignments",
    )
}

/// A key as written: an integer with its precision suffix, shortened as
/// protowire-go's `writtenKey` shortens it.
fn written_key(key: &str, prec: &str) -> String {
    if prec.is_empty() {
        return key.to_string();
    }
    format!("{}p{}", key, clip_literal(prec))
}

/// Returns the byte index of the `}` that matches the `{` at
/// `open_offset`, or `-1` on unterminated input. Mirrors the lexer's
/// string / comment handling so braces inside literals don't confuse the
/// brace count. protowire-go's `findMatchingBrace`: one [`BraceMatch`]
/// scan over the whole input.
pub(crate) fn find_matching_brace(input: &[u8], open_offset: usize) -> i64 {
    let mut m = BraceMatch::new();
    match m.scan(input, open_offset + 1) {
        (i, BraceState::Closed) => i as i64,
        _ => -1,
    }
}

/// [`find_matching_brace`]'s scan, resumable: fed a buffer that grows, it
/// picks up where the previous call stopped, so the dataset header scan
/// finds the end of an `@proto` body read in many pieces in time linear in
/// its size, with the rule `parse` uses (protowire-go's `braceMatch`).
#[derive(Debug, Clone, Copy)]
pub(crate) struct BraceMatch {
    depth: usize,
    mode: BraceMode,
    /// In a simple string, after a backslash, which escapes the next byte.
    esc: bool,
    /// The quotes just seen in a triple-quoted string, or 1 after a '*' in
    /// a block comment.
    run: u8,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum BraceMode {
    Code,
    String,
    TripleString,
    Bytes,
    LineComment,
    BlockComment,
}

/// What [`BraceMatch::scan`] found.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum BraceState {
    /// The input ended first; scan again from the offset returned.
    Open,
    /// The offset returned is the matching '}'.
    Closed,
    /// The body can never close: a simple string or bytes literal met a
    /// newline.
    Failed,
}

impl BraceMatch {
    /// A match just past the opening '{'.
    pub(crate) fn new() -> Self {
        Self {
            depth: 1,
            mode: BraceMode::Code,
            esc: false,
            run: 0,
        }
    }

    /// Advance the match over `input` from offset `i`. It stops before a
    /// byte whose meaning depends on bytes not yet in `input` (a '"' that
    /// may open `"""`, a 'b' or '/' that may open a literal or a comment),
    /// so scanning again from the offset it returns once more input
    /// arrives gives the answer one call over all of it gives.
    pub(crate) fn scan(&mut self, input: &[u8], mut i: usize) -> (usize, BraceState) {
        while i < input.len() {
            let ch = input[i];
            match self.mode {
                BraceMode::String => {
                    if self.esc {
                        self.esc = false;
                    } else if ch == b'\\' {
                        self.esc = true;
                    } else if ch == b'"' {
                        self.mode = BraceMode::Code;
                    } else if ch == b'\n' {
                        return (i, BraceState::Failed);
                    }
                    i += 1;
                    continue;
                }
                BraceMode::TripleString => {
                    if ch != b'"' {
                        self.run = 0;
                    } else {
                        self.run += 1;
                        if self.run == 3 {
                            self.mode = BraceMode::Code;
                            self.run = 0;
                        }
                    }
                    i += 1;
                    continue;
                }
                BraceMode::Bytes => {
                    match ch {
                        b'"' => self.mode = BraceMode::Code,
                        b'\n' => return (i, BraceState::Failed),
                        _ => {}
                    }
                    i += 1;
                    continue;
                }
                BraceMode::LineComment => {
                    if ch == b'\n' {
                        self.mode = BraceMode::Code;
                    }
                    i += 1;
                    continue;
                }
                BraceMode::BlockComment => {
                    if ch == b'/' && self.run == 1 {
                        self.mode = BraceMode::Code;
                        self.run = 0;
                    } else if ch == b'*' {
                        self.run = 1;
                    } else {
                        self.run = 0;
                    }
                    i += 1;
                    continue;
                }
                BraceMode::Code => {}
            }
            match ch {
                b'{' => self.depth += 1,
                b'}' => {
                    self.depth -= 1;
                    if self.depth == 0 {
                        return (i, BraceState::Closed);
                    }
                }
                b'"' => {
                    if i + 2 >= input.len() {
                        return (i, BraceState::Open);
                    }
                    if input[i + 1] == b'"' && input[i + 2] == b'"' {
                        self.mode = BraceMode::TripleString;
                        i += 2;
                    } else {
                        self.mode = BraceMode::String;
                    }
                }
                b'#' => self.mode = BraceMode::LineComment,
                b'b' | b'/' => {
                    if i + 1 >= input.len() {
                        return (i, BraceState::Open);
                    }
                    let next = input[i + 1];
                    let mode = match (ch, next) {
                        (b'b', b'"') => Some(BraceMode::Bytes),
                        (b'/', b'/') => Some(BraceMode::LineComment),
                        (b'/', b'*') => Some(BraceMode::BlockComment),
                        _ => None,
                    };
                    if let Some(m) = mode {
                        self.mode = m;
                        i += 1;
                    }
                }
                _ => {}
            }
            i += 1;
        }
        (i, BraceState::Open)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn assignment(e: &Entry) -> &Assignment {
        match e {
            Entry::Assignment(a) => a,
            other => panic!("expected Assignment, got {:?}", other),
        }
    }

    fn block(e: &Entry) -> &Block {
        match e {
            Entry::Block(b) => b,
            other => panic!("expected Block, got {:?}", other),
        }
    }

    fn map_entry(e: &Entry) -> &MapEntry {
        match e {
            Entry::MapEntry(m) => m,
            other => panic!("expected MapEntry, got {:?}", other),
        }
    }

    fn list(v: &Value) -> &ListVal {
        match v {
            Value::List(l) => l,
            other => panic!("expected ListVal, got {:?}", other),
        }
    }

    fn block_val(v: &Value) -> &BlockVal {
        match v {
            Value::Block(b) => b,
            other => panic!("expected BlockVal, got {:?}", other),
        }
    }

    // ---------------- empty / whitespace ----------------

    #[test]
    fn empty_input_empty_document() {
        let doc = parse("").unwrap();
        assert_eq!(doc.type_url, "");
        assert!(doc.entries.is_empty());
        assert!(doc.leading_comments.is_empty());
    }

    #[test]
    fn whitespace_only_empty_document() {
        let doc = parse("\n\n   \n\t\n").unwrap();
        assert!(doc.entries.is_empty());
    }

    // ---------------- @type directive ----------------

    #[test]
    fn at_type_captures_the_type_name() {
        let doc = parse("@type infra.v1.ServerConfig").unwrap();
        assert_eq!(doc.type_url, "infra.v1.ServerConfig");
        assert!(doc.entries.is_empty());
    }

    #[test]
    fn at_type_then_entries() {
        let doc = parse("@type pkg.M\nname = \"x\"").unwrap();
        assert_eq!(doc.type_url, "pkg.M");
        assert_eq!(doc.entries.len(), 1);
        let a = assignment(&doc.entries[0]);
        assert_eq!(a.key, "name");
    }

    #[test]
    fn at_type_missing_identifier_is_pxferror() {
        let err = parse("@type =").unwrap_err();
        assert!(err.msg.contains("expected type name after @type"));
    }

    // ---------------- scalar assignments ----------------

    #[test]
    fn scalar_string() {
        let doc = parse("hostname = \"web-01\"").unwrap();
        let a = assignment(&doc.entries[0]);
        assert_eq!(a.key, "hostname");
        match &a.value {
            Value::String(s) => assert_eq!(s.value, "web-01"),
            other => panic!("expected String, got {:?}", other),
        }
    }

    #[test]
    fn scalar_int() {
        let doc = parse("port = 8443").unwrap();
        let a = assignment(&doc.entries[0]);
        match &a.value {
            Value::Int(i) => assert_eq!(i.raw, "8443"),
            other => panic!("expected Int, got {:?}", other),
        }
    }

    #[test]
    fn scalar_negative_int() {
        let doc = parse("delta = -42").unwrap();
        let a = assignment(&doc.entries[0]);
        match &a.value {
            Value::Int(i) => assert_eq!(i.raw, "-42"),
            other => panic!("expected Int, got {:?}", other),
        }
    }

    #[test]
    fn scalar_float() {
        let doc = parse("ratio = 0.85").unwrap();
        let a = assignment(&doc.entries[0]);
        match &a.value {
            Value::Float(f) => assert_eq!(f.raw, "0.85"),
            other => panic!("expected Float, got {:?}", other),
        }
    }

    #[test]
    fn scalar_bool() {
        let t = parse("enabled = true").unwrap();
        match &assignment(&t.entries[0]).value {
            Value::Bool(b) => assert!(b.value),
            other => panic!("expected Bool, got {:?}", other),
        }
        let f = parse("enabled = false").unwrap();
        match &assignment(&f.entries[0]).value {
            Value::Bool(b) => assert!(!b.value),
            other => panic!("expected Bool, got {:?}", other),
        }
    }

    #[test]
    fn scalar_null() {
        let doc = parse("email = null").unwrap();
        match &assignment(&doc.entries[0]).value {
            Value::Null(_) => {}
            other => panic!("expected Null, got {:?}", other),
        }
    }

    #[test]
    fn scalar_ident_enum_value() {
        let doc = parse("status = STATUS_SERVING").unwrap();
        match &assignment(&doc.entries[0]).value {
            Value::Ident(i) => assert_eq!(i.name, "STATUS_SERVING"),
            other => panic!("expected Ident, got {:?}", other),
        }
    }

    #[test]
    fn scalar_timestamp_keeps_raw() {
        let doc = parse("created_at = 2024-01-15T10:30:00Z").unwrap();
        match &assignment(&doc.entries[0]).value {
            Value::Timestamp(t) => assert_eq!(t.raw, "2024-01-15T10:30:00Z"),
            other => panic!("expected Timestamp, got {:?}", other),
        }
    }

    #[test]
    fn scalar_duration_keeps_raw() {
        let doc = parse("timeout = 1h30m").unwrap();
        match &assignment(&doc.entries[0]).value {
            Value::Duration(d) => assert_eq!(d.raw, "1h30m"),
            other => panic!("expected Duration, got {:?}", other),
        }
    }

    // ---------------- bytes literals ----------------

    #[test]
    fn bytes_padded_base64_decodes_to_vec() {
        let doc = parse("raw = b\"SGVsbG8=\"").unwrap();
        match &assignment(&doc.entries[0]).value {
            Value::Bytes(b) => assert_eq!(b.value, vec![72, 101, 108, 108, 111]),
            other => panic!("expected Bytes, got {:?}", other),
        }
    }

    #[test]
    fn bytes_raw_unpadded_base64_decodes_correctly() {
        let doc = parse("raw = b\"SGVsbG8\"").unwrap();
        match &assignment(&doc.entries[0]).value {
            Value::Bytes(b) => assert_eq!(b.value, vec![72, 101, 108, 108, 111]),
            other => panic!("expected Bytes, got {:?}", other),
        }
    }

    // ---------------- nested blocks ----------------

    #[test]
    fn block_tls_with_assignments() {
        let src = "tls {\n  cert_file = \"/etc/ssl/cert.pem\"\n  verify    = true\n}";
        let doc = parse(src).unwrap();
        assert_eq!(doc.entries.len(), 1);
        let b = block(&doc.entries[0]);
        assert_eq!(b.name, "tls");
        assert_eq!(b.entries.len(), 2);
        assert_eq!(assignment(&b.entries[0]).key, "cert_file");
    }

    #[test]
    fn block_nested_two_levels() {
        let src = "outer {\n  inner {\n    leaf = 1\n  }\n}";
        let doc = parse(src).unwrap();
        let outer = block(&doc.entries[0]);
        let inner = block(&outer.entries[0]);
        assert_eq!(inner.name, "inner");
        assert_eq!(assignment(&inner.entries[0]).key, "leaf");
    }

    // ---------------- lists ----------------

    #[test]
    fn list_scalar_with_commas() {
        let doc = parse("tags = [\"a\", \"b\", \"c\"]").unwrap();
        let a = assignment(&doc.entries[0]);
        let l = list(&a.value);
        assert_eq!(l.elements.len(), 3);
        let strings: Vec<&str> = l
            .elements
            .iter()
            .map(|v| match v {
                Value::String(s) => s.value.as_str(),
                _ => panic!("expected String"),
            })
            .collect();
        assert_eq!(strings, vec!["a", "b", "c"]);
    }

    #[test]
    fn list_scalar_optional_commas_newline_separated() {
        let src = "tags = [\n  \"a\"\n  \"b\"\n  \"c\"\n]";
        let doc = parse(src).unwrap();
        let a = assignment(&doc.entries[0]);
        assert_eq!(list(&a.value).elements.len(), 3);
    }

    #[test]
    fn list_of_inline_blocks() {
        let src = "endpoints = [\n  { path = \"/api\" }\n  { path = \"/health\" }\n]";
        let doc = parse(src).unwrap();
        let a = assignment(&doc.entries[0]);
        let l = list(&a.value);
        assert_eq!(l.elements.len(), 2);
        match &l.elements[0] {
            Value::Block(_) => {}
            other => panic!("expected BlockVal, got {:?}", other),
        }
    }

    // ---------------- maps ----------------

    #[test]
    fn map_string_keyed() {
        let src = "labels = {\n  env: \"prod\"\n  team: \"platform\"\n}";
        let doc = parse(src).unwrap();
        let a = assignment(&doc.entries[0]);
        let b = block_val(&a.value);
        assert_eq!(b.entries.len(), 2);
        let m0 = map_entry(&b.entries[0]);
        assert_eq!(m0.key, "env");
        match &m0.value {
            Value::String(_) => {}
            other => panic!("expected String, got {:?}", other),
        }
    }

    #[test]
    fn map_quoted_string_keys() {
        let src = "labels = {\n  \"key with space\": \"v\"\n}";
        let doc = parse(src).unwrap();
        let a = assignment(&doc.entries[0]);
        let b = block_val(&a.value);
        assert_eq!(map_entry(&b.entries[0]).key, "key with space");
    }

    #[test]
    fn map_int_keyed() {
        let src = "codes = {\n  404: \"Not Found\"\n  500: \"Internal\"\n}";
        let doc = parse(src).unwrap();
        let a = assignment(&doc.entries[0]);
        let b = block_val(&a.value);
        assert_eq!(map_entry(&b.entries[0]).key, "404");
        assert_eq!(map_entry(&b.entries[1]).key, "500");
    }

    // ---------------- comment attachment ----------------

    #[test]
    fn comments_top_of_document_attach_to_doc_leading() {
        let src = "# leading 1\n# leading 2\nname = \"x\"";
        let doc = parse(src).unwrap();
        assert_eq!(doc.leading_comments.len(), 2);
        assert_eq!(doc.leading_comments[0].text, "# leading 1");
        let a = assignment(&doc.entries[0]);
        assert!(a.leading_comments.is_empty());
    }

    #[test]
    fn comments_after_at_type_attach_to_first_entry() {
        let src = "@type pkg.M\n# header comment\nname = \"x\"";
        let doc = parse(src).unwrap();
        assert!(doc.leading_comments.is_empty());
        let a = assignment(&doc.entries[0]);
        assert_eq!(a.leading_comments.len(), 1);
        assert_eq!(a.leading_comments[0].text, "# header comment");
    }

    #[test]
    fn comments_before_at_type_land_in_doc_leading() {
        let src = "# top of file\n@type pkg.M\nname = \"x\"";
        let doc = parse(src).unwrap();
        assert_eq!(doc.leading_comments.len(), 1);
        assert_eq!(doc.leading_comments[0].text, "# top of file");
    }

    #[test]
    fn comments_inside_block_attach_to_next_entry() {
        let src = "outer {\n  # before leaf\n  leaf = 1\n}";
        let doc = parse(src).unwrap();
        let outer = block(&doc.entries[0]);
        let leaf = assignment(&outer.entries[0]);
        assert_eq!(leaf.leading_comments.len(), 1);
    }

    // ---------------- error positions ----------------

    #[test]
    fn error_expected_after_key() {
        let err = parse("name xyz").unwrap_err();
        assert!(
            err.msg.contains("expected '=', ':', or '{'"),
            "msg: {}",
            err.msg
        );
    }

    #[test]
    fn error_missing_closing_brace() {
        let err = parse("outer {\n  leaf = 1\n").unwrap_err();
        assert!(err.msg.contains("expected '}'"), "msg: {}", err.msg);
    }

    #[test]
    fn error_missing_closing_bracket() {
        let err = parse("tags = [1, 2").unwrap_err();
        assert!(err.msg.contains("expected ']'"), "msg: {}", err.msg);
    }

    #[test]
    fn error_includes_line_col() {
        let err = parse("\n\nname xyz").unwrap_err();
        let s = format!("{}", err);
        assert!(s.starts_with("3:"), "expected 3:..., got {}", s);
    }

    // ---------------- HARDENING.md §Recursion ----------------

    #[test]
    fn deep_nesting_at_limit_is_accepted() {
        // 100 levels of `{` is exactly at the cap and must parse.
        let mut src = String::from("root ");
        for _ in 0..99 {
            src.push_str("{ child ");
        }
        src.push_str("{ leaf = 1 ");
        for _ in 0..100 {
            src.push('}');
        }
        parse(&src).unwrap();
    }

    #[test]
    fn deep_nesting_past_limit_is_rejected() {
        // 200 levels of `{` must reject with the depth error, not crash.
        let mut src = String::from("root ");
        for _ in 0..200 {
            src.push_str("{ child ");
        }
        src.push_str("{ leaf = 1 ");
        for _ in 0..201 {
            src.push('}');
        }
        let err = parse(&src).unwrap_err();
        assert!(
            err.msg.contains("MaxNestingDepth"),
            "expected depth error, got: {}",
            err.msg
        );
    }

    #[test]
    fn deep_nesting_extreme_does_not_overflow_stack() {
        // The crash case from issue #1: 100 000 nested `{`. Must reject
        // cleanly — recursive descent would otherwise SIGABRT here.
        let mut src = String::from("root ");
        for _ in 0..100_000 {
            src.push_str("{ child ");
        }
        src.push_str("{ leaf = 1 ");
        for _ in 0..100_001 {
            src.push('}');
        }
        let err = parse(&src).unwrap_err();
        assert!(err.msg.contains("MaxNestingDepth"));
    }

    #[test]
    fn deep_list_nesting_past_limit_is_rejected() {
        // `[` nesting also counts.
        let mut src = String::from("xs = ");
        for _ in 0..200 {
            src.push('[');
        }
        for _ in 0..200 {
            src.push(']');
        }
        let err = parse(&src).unwrap_err();
        assert!(
            err.msg.contains("MaxNestingDepth"),
            "expected depth error, got: {}",
            err.msg
        );
    }

    // ---------------- end-to-end ----------------

    #[test]
    fn end_to_end_pxf_readme_sample() {
        let src = "@type infra.v1.ServerConfig\n\
                   \n\
                   hostname = \"web-01.prod.example.com\"\n\
                   port     = 8443\n\
                   enabled  = true\n\
                   status   = STATUS_SERVING\n\
                   \n\
                   # Well-known type literals\n\
                   created_at = 2024-01-15T10:30:00Z\n\
                   timeout    = 30s\n\
                   \n\
                   # Nested messages use block syntax\n\
                   tls {\n\
                     cert_file = \"/etc/ssl/cert.pem\"\n\
                     key_file  = \"/etc/ssl/key.pem\"\n\
                     verify    = true\n\
                   }\n\
                   \n\
                   # Repeated fields use list syntax\n\
                   tags = [\"production\", \"us-east\", \"frontend\"]\n\
                   \n\
                   # Maps use : for key-value pairs\n\
                   labels = {\n\
                     env: \"production\"\n\
                     team: \"platform\"\n\
                     \"hello world\": \"quoted keys supported\"\n\
                   }\n\
                   \n\
                   # Repeated messages\n\
                   endpoints = [\n\
                     {\n\
                       path = \"/api/v1/users\"\n\
                       method = \"GET\"\n\
                     }\n\
                     {\n\
                       path = \"/health\"\n\
                       method = \"GET\"\n\
                     }\n\
                   ]\n\
                   \n\
                   # Wrapper type sugar\n\
                   nullable_name = \"present\"\n";
        let doc = parse(src).unwrap();
        assert_eq!(doc.type_url, "infra.v1.ServerConfig");
        assert!(doc.entries.len() > 5);

        // Spot-check by name.
        let mut hostname = false;
        let mut tls = false;
        let mut tags_is_list = false;
        let mut labels_is_block_val = false;
        for e in &doc.entries {
            match e {
                Entry::Assignment(a) => {
                    if a.key == "hostname" {
                        hostname = true;
                    }
                    if a.key == "tags" {
                        tags_is_list = matches!(a.value, Value::List(_));
                    }
                    if a.key == "labels" {
                        labels_is_block_val = matches!(a.value, Value::Block(_));
                    }
                }
                Entry::Block(b) => {
                    if b.name == "tls" {
                        tls = true;
                    }
                }
                Entry::MapEntry(_) | Entry::TypeEntry(_) => {}
            }
        }
        assert!(hostname);
        assert!(tls);
        assert!(tags_is_list);
        assert!(labels_is_block_val);
    }
}
