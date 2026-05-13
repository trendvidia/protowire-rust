// SPDX-License-Identifier: MIT
// Copyright (c) 2026 TrendVidia, LLC.
//! Schema-bound PXF decoder.
//!
//! Slice D1: scalars, enums, nested messages, repeated lists, oneof.
//! Slice D2: maps + well-known types (Timestamp/Duration/wrappers).
//! Slice D3: `google.protobuf.Any` sugar via a pluggable [`TypeResolver`].
//! Mirrors Go's fused single-pass path in `protowire-go/encoding/pxf/decode_fast.go`
//! (`unmarshalDirect`) and the TS port's `pxf/decode.ts`. There is no separate
//! AST-walking slow path — the lexer drives the descriptor walk in lockstep
//! and writes straight into a `prost_reflect::DynamicMessage`.
//!
//! The `Result`-tracking `unmarshal_full` (required/default/_null) lands in D4.

use prost::Message as _;
use prost_reflect::{
    DescriptorPool, DynamicMessage, FieldDescriptor, Kind, MapKey, MessageDescriptor,
    OneofDescriptor, ReflectMessage, Value,
};
use std::collections::HashMap;

use crate::annotations::{find_null_mask_field, get_default, is_required};
use crate::ast::{
    BoolVal as AstBoolVal, BytesVal as AstBytesVal, DatasetDirective, DatasetRow, Directive,
    DurationVal as AstDurationVal, FloatVal as AstFloatVal, IdentVal as AstIdentVal,
    IntVal as AstIntVal, NullVal as AstNullVal, ProtoDirective, ProtoShape,
    StringVal as AstStringVal, TimestampVal as AstTimestampVal, Value as AstValue,
};
use crate::errors::PxfError;
use crate::lexer::Lexer;
use crate::parser::MAX_NESTING_DEPTH;
use crate::result::Presence;
use crate::token::{Position, Token, TokenKind};

/// Resolves `google.protobuf.Any` type URLs to message descriptors. Mirrors the
/// Go interface of the same name. The URL prefix (`type.googleapis.com/…`) is
/// the implementation's responsibility — strip it as appropriate.
pub trait TypeResolver {
    fn find_message_by_url(&self, url: &str) -> Option<MessageDescriptor>;
}

/// A [`TypeResolver`] backed by a [`DescriptorPool`]. Strips the URL prefix
/// (everything up to and including the last `/`) before lookup, matching the
/// `type.googleapis.com/<typeName>` convention used by `anyPack`.
pub struct PoolResolver<'a>(pub &'a DescriptorPool);

impl<'a> TypeResolver for PoolResolver<'a> {
    fn find_message_by_url(&self, url: &str) -> Option<MessageDescriptor> {
        let name = url.rsplit_once('/').map_or(url, |(_, n)| n);
        self.0.get_message_by_name(name)
    }
}

/// Options controlling [`unmarshal`] behavior.
#[derive(Default, Clone, Copy)]
pub struct UnmarshalOptions<'a> {
    /// Silently skip fields not declared in the schema instead of erroring.
    pub discard_unknown: bool,
    /// Resolves type URLs for `google.protobuf.Any` fields. When `Some`, Any
    /// fields use sugar syntax (`@type = "..."` plus inline fields). When
    /// `None`, Any fields decode as regular messages with `type_url` and
    /// `value` fields.
    pub type_resolver: Option<&'a dyn TypeResolver>,
    /// Skip the per-call schema reserved-name check (draft §3.13).
    /// Callers that have already validated their descriptors (typically
    /// via [`crate::validate_descriptor`] in a one-time codegen or
    /// registry-load pass) can set this to bypass the per-call recheck.
    pub skip_validate: bool,
}

/// Decode PXF text into a fresh [`DynamicMessage`] for `desc`.
pub fn unmarshal(
    data: &str,
    desc: &MessageDescriptor,
    options: UnmarshalOptions<'_>,
) -> Result<DynamicMessage, PxfError> {
    let (msg, _) = unmarshal_inner(data, desc, options, false)?;
    Ok(msg)
}

/// Decode PXF text into a fresh [`DynamicMessage`] *and* return field-presence
/// metadata. Differs from [`unmarshal`] in that it:
///
/// - tracks which dotted paths were explicitly set, set to null, or absent;
/// - validates `(pxf.required) = true` fields and errors when absent;
/// - applies `(pxf.default) = "…"` strings to absent (non-null) fields;
/// - mirrors null state into the root message's `_null` `FieldMask`, if one
///   is declared.
pub fn unmarshal_full(
    data: &str,
    desc: &MessageDescriptor,
    options: UnmarshalOptions<'_>,
) -> Result<(DynamicMessage, Presence), PxfError> {
    let (msg, presence) = unmarshal_inner(data, desc, options, true)?;
    let presence = presence.expect("presence requested");
    Ok((msg, presence))
}

fn unmarshal_inner(
    data: &str,
    desc: &MessageDescriptor,
    options: UnmarshalOptions<'_>,
    track_presence: bool,
) -> Result<(DynamicMessage, Option<Presence>), PxfError> {
    if !options.skip_validate {
        let vs = crate::schema::validate_descriptor(desc);
        if let Some(msg) = crate::schema::as_validation_error_message(&vs) {
            return Err(PxfError::new(Position::default(), msg));
        }
    }
    let mut decoder = Decoder::new(
        data,
        options.discard_unknown,
        options.type_resolver,
        track_presence,
    );
    decoder.advance();
    decoder.consume_directives()?;

    let mut msg = DynamicMessage::new(desc.clone());
    decoder.decode_fields(&mut msg, false)?;

    let presence = if track_presence {
        let null_mask_fd = find_null_mask_field(desc);
        let presence = decoder.into_presence();
        post_decode(&mut msg, &presence, null_mask_fd.as_ref(), "")?;
        // Mirror null paths into the root's _null FieldMask, if any.
        if let Some(null_mask_fd) = null_mask_fd {
            let mut paths: Vec<String> = presence.null_paths().map(|s| s.to_string()).collect();
            if !paths.is_empty() {
                paths.sort();
                let inner_desc = match null_mask_fd.kind() {
                    Kind::Message(m) => m,
                    _ => unreachable!("null mask field is FieldMask"),
                };
                let mut fm = DynamicMessage::new(inner_desc);
                let paths_fd = fm
                    .descriptor()
                    .get_field_by_name("paths")
                    .expect("FieldMask.paths");
                fm.set_field(
                    &paths_fd,
                    Value::List(paths.into_iter().map(Value::String).collect()),
                );
                msg.set_field(&null_mask_fd, Value::Message(fm));
            }
        }
        Some(presence)
    } else {
        None
    };
    Ok((msg, presence))
}

struct Decoder<'a> {
    lex: Lexer<'a>,
    current: Token,
    discard_unknown: bool,
    type_resolver: Option<&'a dyn TypeResolver>,
    presence: Option<Presence>,
    path_prefix: String,
    /// Live `{` + `[` depth, mirrors the parser's counter. Capped at
    /// [`MAX_NESTING_DEPTH`] per HARDENING.md §Recursion. Threaded across
    /// `decode_fields` / `decode_list_inline` / `decode_map_inline` /
    /// `decode_any_inner` so adversarial deep input rejects before the
    /// recursive descent overflows the native stack.
    depth: usize,
}

impl<'a> Decoder<'a> {
    fn new(
        input: &'a str,
        discard_unknown: bool,
        type_resolver: Option<&'a dyn TypeResolver>,
        track_presence: bool,
    ) -> Self {
        Self {
            lex: Lexer::new(input),
            current: Token::new(TokenKind::Eof, "", Position::new(1, 1)),
            discard_unknown,
            type_resolver,
            presence: if track_presence {
                Some(Presence::new())
            } else {
                None
            },
            path_prefix: String::new(),
            depth: 0,
        }
    }

    fn enter(&mut self) -> Result<(), PxfError> {
        if self.depth >= MAX_NESTING_DEPTH {
            return Err(self.err(format!(
                "nesting depth exceeds MaxNestingDepth ({})",
                MAX_NESTING_DEPTH
            )));
        }
        self.depth += 1;
        Ok(())
    }

    fn leave(&mut self) {
        self.depth -= 1;
    }

    fn into_presence(self) -> Presence {
        self.presence.expect("presence not requested")
    }

    fn mark_present(&mut self, fd: &FieldDescriptor) {
        if let Some(p) = self.presence.as_mut() {
            p.mark_present(format!("{}{}", self.path_prefix, fd.name()));
        }
    }

    fn mark_null(&mut self, fd: &FieldDescriptor) {
        if let Some(p) = self.presence.as_mut() {
            p.mark_null(format!("{}{}", self.path_prefix, fd.name()));
        }
    }

    fn advance(&mut self) {
        loop {
            self.current = self.lex.next_token();
            if !matches!(self.current.kind, TokenKind::Comment | TokenKind::Newline) {
                return;
            }
        }
    }

    /// peek_kind: one-token lookahead without consumption. Used by
    /// the directive-prefix disambiguator in consume_directives.
    fn peek_kind(&mut self) -> TokenKind {
        let snap = self.lex.snapshot();
        let saved = self.current.clone();
        self.advance();
        let k = self.current.kind;
        self.lex.restore(snap);
        self.current = saved;
        k
    }

    /// consume_directives drains any leading `@type` / `@<name>` /
    /// `@dataset` directives, leaving `self.current` at the first body
    /// token. PR 1 of the v0.72-v0.75 catch-up discards directive
    /// contents in the direct decoder; semantics (Presence accessors,
    /// DatasetReader, bind_row) arrive in later PRs.
    ///
    /// Enforces the standalone constraint (draft §3.4.4): a document
    /// containing any `@dataset` directive MUST NOT also carry `@type`
    /// or top-level field entries.
    /// Parse one scalar @dataset cell token at `self.current` into an
    /// AST `Value`. Mirrors the scalar branches of the AST parser's
    /// `parse_value`. List / block tokens are rejected by the caller
    /// before this is invoked.
    fn parse_scalar_cell_value(&mut self) -> Result<AstValue, PxfError> {
        let pos = self.current.pos;
        let value = std::mem::take(&mut self.current.value);
        let kind = self.current.kind;
        let out = match kind {
            TokenKind::String => AstValue::String(AstStringVal { pos, value }),
            TokenKind::Int => AstValue::Int(AstIntVal { pos, raw: value }),
            TokenKind::Float => AstValue::Float(AstFloatVal { pos, raw: value }),
            TokenKind::Bool => AstValue::Bool(AstBoolVal {
                pos,
                value: value == "true",
            }),
            TokenKind::Bytes => {
                let bytes = decode_base64(&value).unwrap_or_default();
                AstValue::Bytes(AstBytesVal { pos, value: bytes })
            }
            TokenKind::Timestamp => AstValue::Timestamp(AstTimestampVal { pos, raw: value }),
            TokenKind::Duration => AstValue::Duration(AstDurationVal { pos, raw: value }),
            TokenKind::Null => AstValue::Null(AstNullVal { pos }),
            TokenKind::Ident => AstValue::Ident(AstIdentVal { pos, name: value }),
            _ => {
                return Err(PxfError::new(
                    pos,
                    format!("unsupported @dataset cell value: {}", kind),
                ));
            }
        };
        self.advance();
        Ok(out)
    }

    fn consume_directives(&mut self) -> Result<(), PxfError> {
        let mut saw_type = false;
        let mut has_dataset = false;
        let mut first_dataset_pos = Position::new(1, 1);
        loop {
            match self.current.kind {
                TokenKind::AtType => {
                    if has_dataset {
                        return Err(
                            self.err("@dataset directive cannot coexist with @type (draft §3.4.4)")
                        );
                    }
                    saw_type = true;
                    self.advance(); // consume @type
                    if !matches!(self.current.kind, TokenKind::Ident | TokenKind::String) {
                        return Err(self.err(format!(
                            "expected type name after @type, got {}",
                            self.current.kind
                        )));
                    }
                    self.advance();
                }
                TokenKind::AtDirective => {
                    let at_pos = self.current.pos;
                    let name = std::mem::take(&mut self.current.value);
                    if crate::schema::is_future_reserved_directive(&name) {
                        return Err(PxfError::new(
                            at_pos,
                            format!(
                                "@{} is a spec-reserved directive name with no v1 semantics (draft §3.4.6)",
                                name
                            ),
                        ));
                    }
                    let mut prefixes: Vec<String> = Vec::new();
                    self.advance(); // consume @<name>
                                    // Zero-or-more prefix identifiers with lookahead.
                    while matches!(self.current.kind, TokenKind::Ident) {
                        let next = self.peek_kind();
                        if matches!(next, TokenKind::Equals | TokenKind::Colon) {
                            break;
                        }
                        prefixes.push(std::mem::take(&mut self.current.value));
                        self.advance();
                    }
                    // Back-compat: single prefix populates legacy `type`.
                    let r#type = if prefixes.len() == 1 {
                        prefixes[0].clone()
                    } else {
                        String::new()
                    };
                    let mut body: Vec<u8> = Vec::new();
                    let mut has_body = false;
                    // Optional inline block — slice raw bytes by walking
                    // brace depth at the token level. Strings / comments
                    // are handled by the lexer, so brace tokens here are
                    // always real braces.
                    if matches!(self.current.kind, TokenKind::LBrace) {
                        let open = self.current.pos.offset;
                        let mut depth: usize = 1;
                        self.advance();
                        while depth > 0 && !matches!(self.current.kind, TokenKind::Eof) {
                            match self.current.kind {
                                TokenKind::LBrace => depth += 1,
                                TokenKind::RBrace => {
                                    depth -= 1;
                                    if depth == 0 {
                                        let close = self.current.pos.offset;
                                        body = self.lex.input_view()[open + 1..close].to_vec();
                                        has_body = true;
                                        self.advance();
                                        break;
                                    }
                                }
                                _ => {}
                            }
                            self.advance();
                        }
                        if depth != 0 {
                            return Err(self.err("unmatched '{' in directive block"));
                        }
                    }
                    if let Some(p) = self.presence.as_mut() {
                        p.add_directive(Directive {
                            pos: at_pos,
                            name,
                            prefixes,
                            r#type,
                            body,
                            has_body,
                            leading_comments: Vec::new(),
                        });
                    }
                }
                TokenKind::AtDataset => {
                    if saw_type {
                        return Err(
                            self.err("@dataset directive cannot coexist with @type (draft §3.4.4)")
                        );
                    }
                    let table_pos = self.current.pos;
                    if !has_dataset {
                        first_dataset_pos = table_pos;
                        has_dataset = true;
                    }
                    self.advance(); // consume @dataset
                                    // Optional row message type; MAY be omitted when an
                                    // anonymous @proto precedes (draft §3.4.4 Anonymous
                                    // binding).
                    let table_type = if matches!(self.current.kind, TokenKind::Ident) {
                        let t = std::mem::take(&mut self.current.value);
                        self.advance();
                        t
                    } else {
                        String::new()
                    };
                    if !matches!(self.current.kind, TokenKind::LParen) {
                        return Err(self.err("expected '(' to start @dataset column list"));
                    }
                    self.advance();
                    if !matches!(self.current.kind, TokenKind::Ident) {
                        return Err(
                            self.err("@dataset column list must contain at least one field name")
                        );
                    }
                    let mut columns: Vec<String> = Vec::new();
                    loop {
                        if !matches!(self.current.kind, TokenKind::Ident) {
                            return Err(self.err("expected column field name"));
                        }
                        if self.current.value.contains('.') {
                            return Err(self.err(
                                "@dataset column has dotted path; not supported in v1 (draft §3.4.4)",
                            ));
                        }
                        columns.push(std::mem::take(&mut self.current.value));
                        self.advance();
                        if matches!(self.current.kind, TokenKind::Comma) {
                            self.advance();
                            continue;
                        }
                        if matches!(self.current.kind, TokenKind::RParen) {
                            break;
                        }
                        return Err(self.err("expected ',' or ')' in @dataset column list"));
                    }
                    self.advance(); // consume )
                    let n_cols = columns.len();
                    let mut rows: Vec<DatasetRow> = Vec::new();
                    // Zero or more rows; each cell is a single scalar
                    // token (or empty).
                    while matches!(self.current.kind, TokenKind::LParen) {
                        let row_pos = self.current.pos;
                        self.advance(); // (
                        let mut cells: Vec<Option<AstValue>> = Vec::with_capacity(n_cols);
                        // First cell.
                        if matches!(self.current.kind, TokenKind::Comma | TokenKind::RParen) {
                            cells.push(None);
                        } else if matches!(
                            self.current.kind,
                            TokenKind::LBracket | TokenKind::LBrace
                        ) {
                            return Err(self.err(
                                "@dataset cells cannot contain list/block values in v1 (draft §3.4.4)",
                            ));
                        } else {
                            cells.push(Some(self.parse_scalar_cell_value()?));
                        }
                        while matches!(self.current.kind, TokenKind::Comma) {
                            self.advance();
                            if matches!(self.current.kind, TokenKind::Comma | TokenKind::RParen) {
                                cells.push(None);
                            } else if matches!(
                                self.current.kind,
                                TokenKind::LBracket | TokenKind::LBrace
                            ) {
                                return Err(self.err(
                                    "@dataset cells cannot contain list/block values in v1 (draft §3.4.4)",
                                ));
                            } else {
                                cells.push(Some(self.parse_scalar_cell_value()?));
                            }
                        }
                        if !matches!(self.current.kind, TokenKind::RParen) {
                            return Err(self.err("expected ',' or ')' in @dataset row"));
                        }
                        if cells.len() != n_cols {
                            return Err(self.err_at(
                                row_pos,
                                format!(
                                    "@dataset row has {} cells, expected {} (column count)",
                                    cells.len(),
                                    n_cols
                                ),
                            ));
                        }
                        self.advance(); // consume )
                        rows.push(DatasetRow {
                            pos: row_pos,
                            cells,
                        });
                    }
                    if let Some(p) = self.presence.as_mut() {
                        p.add_dataset(DatasetDirective {
                            pos: table_pos,
                            r#type: table_type,
                            columns,
                            rows,
                            leading_comments: Vec::new(),
                        });
                    }
                }
                TokenKind::AtProto => {
                    let at_pos = self.current.pos;
                    self.advance(); // consume @proto
                    let (shape, type_name, body) = match self.current.kind {
                        TokenKind::LBrace => {
                            let body =
                                self.consume_proto_brace_body(at_pos, "@proto (anonymous form)")?;
                            (ProtoShape::Anonymous, String::new(), body)
                        }
                        TokenKind::Ident => {
                            let type_name = std::mem::take(&mut self.current.value);
                            self.advance();
                            if !matches!(self.current.kind, TokenKind::LBrace) {
                                return Err(self.err(format!(
                                    "expected '{{' after @proto {}, got {}",
                                    type_name, self.current.kind
                                )));
                            }
                            let body = self.consume_proto_brace_body(
                                at_pos,
                                &format!("@proto {}", type_name),
                            )?;
                            (ProtoShape::Named, type_name, body)
                        }
                        TokenKind::String => {
                            let body = std::mem::take(&mut self.current.value).into_bytes();
                            self.advance();
                            (ProtoShape::Source, String::new(), body)
                        }
                        TokenKind::Bytes => {
                            let raw = std::mem::take(&mut self.current.value);
                            let decoded = decode_base64(&raw).ok_or_else(|| {
                                self.err("@proto descriptor body: invalid base64")
                            })?;
                            self.advance();
                            (ProtoShape::Descriptor, String::new(), decoded)
                        }
                        _ => {
                            return Err(self.err(format!(
                                "expected '{{', dotted identifier, triple-quoted string, or b\"...\" after @proto, got {}",
                                self.current.kind
                            )));
                        }
                    };
                    if let Some(p) = self.presence.as_mut() {
                        p.add_proto(ProtoDirective {
                            pos: at_pos,
                            shape,
                            type_name,
                            body,
                            leading_comments: Vec::new(),
                        });
                    }
                }
                _ => {
                    if has_dataset && !matches!(self.current.kind, TokenKind::Eof) {
                        return Err(self.err_at(
                            first_dataset_pos,
                            "@dataset directive cannot coexist with top-level field entries (draft §3.4.4)",
                        ));
                    }
                    return Ok(());
                }
            }
        }
    }

    fn err(&self, msg: impl Into<String>) -> PxfError {
        PxfError::new(self.current.pos, msg)
    }

    fn err_at(&self, pos: Position, msg: impl Into<String>) -> PxfError {
        PxfError::new(pos, msg)
    }

    /// Consume the raw bytes between `{` and the matching `}` (both
    /// exclusive) for an `@proto` brace-bounded body. LBrace is
    /// current on entry. Walks brace depth at the token level —
    /// strings and comments are handled by the lexer, so brace tokens
    /// here are always real braces.
    fn consume_proto_brace_body(
        &mut self,
        at_pos: Position,
        label: &str,
    ) -> Result<Vec<u8>, PxfError> {
        let open = self.current.pos.offset;
        let mut depth: usize = 1;
        self.advance();
        while depth > 0 && !matches!(self.current.kind, TokenKind::Eof) {
            match self.current.kind {
                TokenKind::LBrace => depth += 1,
                TokenKind::RBrace => {
                    depth -= 1;
                    if depth == 0 {
                        let close = self.current.pos.offset;
                        let body = self.lex.input_view()[open + 1..close].to_vec();
                        self.advance();
                        return Ok(body);
                    }
                }
                _ => {}
            }
            self.advance();
        }
        Err(self.err_at(at_pos, format!("{}: unmatched '{{'", label)))
    }

    fn decode_fields(&mut self, msg: &mut DynamicMessage, in_block: bool) -> Result<(), PxfError> {
        // The `{` itself was consumed by the caller before re-entering
        // decode_fields with in_block=true; increment depth here so that the
        // counter reflects open `{` blocks across all nested-message paths
        // (decode_field_value, list elements, map values, Any payloads).
        if in_block {
            self.enter()?;
        }
        let result = self.decode_fields_inner(msg, in_block);
        if in_block {
            self.leave();
        }
        result
    }

    fn decode_fields_inner(
        &mut self,
        msg: &mut DynamicMessage,
        in_block: bool,
    ) -> Result<(), PxfError> {
        let desc = msg.descriptor();
        let mut set_oneofs: HashMap<String, String> = HashMap::new();

        loop {
            if in_block && matches!(self.current.kind, TokenKind::RBrace) {
                self.advance();
                return Ok(());
            }
            if matches!(self.current.kind, TokenKind::Eof) {
                if in_block {
                    return Err(self.err("expected '}', got EOF"));
                }
                return Ok(());
            }

            let pos = self.current.pos;
            let key_kind = self.current.kind;
            if !matches!(
                key_kind,
                TokenKind::Ident | TokenKind::String | TokenKind::Int
            ) {
                return Err(self.err_at(
                    pos,
                    format!(
                        "expected identifier, string, or integer, got {} ({:?})",
                        key_kind, self.current.value
                    ),
                ));
            }
            let key = std::mem::take(&mut self.current.value);
            self.advance();

            match self.current.kind {
                TokenKind::Equals => {
                    self.advance();
                    let fd = match desc.get_field_by_name(&key) {
                        Some(fd) => fd,
                        None => {
                            if self.discard_unknown {
                                self.skip_value();
                                continue;
                            }
                            return Err(self.err_at(
                                pos,
                                format!("unknown field {:?} in {}", key, desc.full_name()),
                            ));
                        }
                    };
                    self.check_oneof(&fd, &mut set_oneofs, pos)?;
                    if matches!(self.current.kind, TokenKind::Null) {
                        self.mark_null(&fd);
                        self.advance();
                        continue;
                    }
                    self.mark_present(&fd);
                    self.decode_field_value(msg, &fd)?;
                }
                TokenKind::LBrace => {
                    self.advance();
                    let fd = match desc.get_field_by_name(&key) {
                        Some(fd) => fd,
                        None => {
                            if self.discard_unknown {
                                self.skip_braced();
                                continue;
                            }
                            return Err(self.err_at(
                                pos,
                                format!("unknown field {:?} in {}", key, desc.full_name()),
                            ));
                        }
                    };
                    if fd.is_list() {
                        return Err(self.err_at(
                            pos,
                            format!(
                                "repeated field {:?} must use list syntax: {} = [...]",
                                key, key
                            ),
                        ));
                    }
                    if fd.is_map() {
                        return Err(self.err_at(
                            pos,
                            format!(
                                "map field {:?} must use assignment syntax: {} = {{ ... }}",
                                key, key
                            ),
                        ));
                    }
                    if !matches!(fd.kind(), Kind::Message(_)) {
                        return Err(self.err_at(
                            pos,
                            format!(
                                "field {:?} is not a message type, cannot use block syntax",
                                key
                            ),
                        ));
                    }
                    self.check_oneof(&fd, &mut set_oneofs, pos)?;
                    self.mark_present(&fd);
                    let inner_desc = match fd.kind() {
                        Kind::Message(m) => m,
                        _ => unreachable!(),
                    };
                    let mut sub = DynamicMessage::new(inner_desc);
                    if is_any_full_name(sub.descriptor().full_name())
                        && self.type_resolver.is_some()
                        && matches!(self.current.kind, TokenKind::AtType)
                    {
                        self.decode_any_inner(&mut sub)?;
                    } else if self.presence.is_some() {
                        let saved = std::mem::take(&mut self.path_prefix);
                        self.path_prefix = format!("{}{}.", saved, fd.name());
                        self.decode_fields(&mut sub, true)?;
                        self.path_prefix = saved;
                    } else {
                        self.decode_fields(&mut sub, true)?;
                    }
                    msg.set_field(&fd, Value::Message(sub));
                }
                TokenKind::Colon => {
                    return Err(self.err_at(
                        pos,
                        "unexpected ':' in message context, use '=' for field assignments",
                    ));
                }
                _ => {
                    return Err(self.err(format!(
                        "expected '=', ':', or '{{' after {:?}, got {}",
                        key, self.current.kind
                    )));
                }
            }
        }
    }

    fn check_oneof(
        &self,
        fd: &FieldDescriptor,
        set_oneofs: &mut HashMap<String, String>,
        pos: Position,
    ) -> Result<(), PxfError> {
        let oo: Option<OneofDescriptor> = fd.containing_oneof();
        let Some(oo) = oo else { return Ok(()) };
        // Synthetic oneofs (proto3 optional) wrap a single field, so they can
        // never produce a real conflict — leaving them in the map is harmless.
        let name = oo.name().to_string();
        if let Some(prev) = set_oneofs.get(&name) {
            return Err(self.err_at(
                pos,
                format!(
                    "oneof {:?}: field {:?} conflicts with already-set field {:?}",
                    name,
                    fd.name(),
                    prev
                ),
            ));
        }
        set_oneofs.insert(name, fd.name().to_string());
        Ok(())
    }

    fn decode_field_value(
        &mut self,
        msg: &mut DynamicMessage,
        fd: &FieldDescriptor,
    ) -> Result<(), PxfError> {
        if fd.is_map() {
            return self.decode_map_inline(msg, fd);
        }
        if fd.is_list() {
            return self.decode_list_inline(msg, fd);
        }
        if let Kind::Message(inner_desc) = fd.kind() {
            let mut sub = DynamicMessage::new(inner_desc);
            if self.try_decode_wkt(&mut sub)? {
                msg.set_field(fd, Value::Message(sub));
                return Ok(());
            }
            if !matches!(self.current.kind, TokenKind::LBrace) {
                return Err(self.err(format!("expected '{{' for message field {:?}", fd.name())));
            }
            self.advance();
            if is_any_full_name(sub.descriptor().full_name())
                && self.type_resolver.is_some()
                && matches!(self.current.kind, TokenKind::AtType)
            {
                self.decode_any_inner(&mut sub)?;
            } else if self.presence.is_some() {
                let saved = std::mem::take(&mut self.path_prefix);
                self.path_prefix = format!("{}{}.", saved, fd.name());
                self.decode_fields(&mut sub, true)?;
                self.path_prefix = saved;
            } else {
                self.decode_fields(&mut sub, true)?;
            }
            msg.set_field(fd, Value::Message(sub));
            return Ok(());
        }
        if matches!(fd.kind(), Kind::Enum(_)) {
            let v = self.consume_enum(fd)?;
            msg.set_field(fd, v);
            return Ok(());
        }
        let v = self.consume_scalar(fd)?;
        msg.set_field(fd, v);
        Ok(())
    }

    fn decode_list_inline(
        &mut self,
        msg: &mut DynamicMessage,
        fd: &FieldDescriptor,
    ) -> Result<(), PxfError> {
        if !matches!(self.current.kind, TokenKind::LBracket) {
            return Err(self.err(format!("expected '[' for repeated field {:?}", fd.name())));
        }
        self.enter()?;
        self.advance();

        let mut elems: Vec<Value> = Vec::new();
        let element_kind = fd.kind();

        while !matches!(self.current.kind, TokenKind::RBracket | TokenKind::Eof) {
            if matches!(self.current.kind, TokenKind::Null) {
                return Err(self.err(format!(
                    "null is not allowed in repeated field {:?}",
                    fd.name()
                )));
            }
            let v = match &element_kind {
                Kind::Message(inner_desc) => {
                    let mut sub = DynamicMessage::new(inner_desc.clone());
                    if !self.try_decode_wkt(&mut sub)? {
                        if !matches!(self.current.kind, TokenKind::LBrace) {
                            return Err(self.err("expected '{' for repeated message element"));
                        }
                        self.advance();
                        self.decode_fields(&mut sub, true)?;
                    }
                    Value::Message(sub)
                }
                Kind::Enum(_) => self.consume_enum(fd)?,
                _ => self.consume_scalar(fd)?,
            };
            elems.push(v);
            if matches!(self.current.kind, TokenKind::Comma) {
                self.advance();
            }
        }

        if !matches!(self.current.kind, TokenKind::RBracket) {
            return Err(self.err(format!("expected ']', got {}", self.current.kind)));
        }
        self.advance();
        self.leave();

        msg.set_field(fd, Value::List(elems));
        Ok(())
    }

    fn decode_map_inline(
        &mut self,
        msg: &mut DynamicMessage,
        fd: &FieldDescriptor,
    ) -> Result<(), PxfError> {
        if !matches!(self.current.kind, TokenKind::LBrace) {
            return Err(self.err(format!("expected '{{' for map field {:?}", fd.name())));
        }
        self.enter()?;
        self.advance();

        let map_entry_desc = match fd.kind() {
            Kind::Message(m) => m,
            _ => {
                return Err(self.err(format!(
                    "internal: map field {:?} kind is not message",
                    fd.name()
                )))
            }
        };
        let key_fd = map_entry_desc.map_entry_key_field();
        let val_fd = map_entry_desc.map_entry_value_field();

        let mut map: HashMap<MapKey, Value> = HashMap::new();

        while !matches!(self.current.kind, TokenKind::RBrace | TokenKind::Eof) {
            let pos = self.current.pos;
            let tk = self.current.kind;
            if !matches!(
                tk,
                TokenKind::Ident | TokenKind::String | TokenKind::Int | TokenKind::Bool
            ) {
                return Err(self.err_at(pos, format!("expected map key, got {}", tk)));
            }
            let key_str = std::mem::take(&mut self.current.value);
            self.advance();

            match self.current.kind {
                TokenKind::Colon => self.advance(),
                TokenKind::Equals => {
                    return Err(self.err("unexpected '=' in map, use ':' for map entries"))
                }
                _ => {
                    return Err(self.err(format!(
                        "expected ':' after map key, got {}",
                        self.current.kind
                    )))
                }
            }

            let key = decode_map_key(&key_fd, key_str, pos)?;

            if matches!(self.current.kind, TokenKind::Null) {
                return Err(self.err(format!(
                    "null is not allowed as map value in field {:?}",
                    fd.name()
                )));
            }

            let value = if let Kind::Message(inner_desc) = val_fd.kind() {
                let mut sub = DynamicMessage::new(inner_desc);
                if !self.try_decode_wkt(&mut sub)? {
                    if !matches!(self.current.kind, TokenKind::LBrace) {
                        return Err(self.err("expected '{' for map message value"));
                    }
                    self.advance();
                    self.decode_fields(&mut sub, true)?;
                }
                Value::Message(sub)
            } else if matches!(val_fd.kind(), Kind::Enum(_)) {
                self.consume_enum(&val_fd)?
            } else {
                self.consume_scalar(&val_fd)?
            };

            map.insert(key, value);
        }

        if !matches!(self.current.kind, TokenKind::RBrace) {
            return Err(self.err(format!("expected '}}', got {}", self.current.kind)));
        }
        self.advance();
        self.leave();

        msg.set_field(fd, Value::Map(map));
        Ok(())
    }

    /// Decode `google.protobuf.Any` sugar — caller has already entered the
    /// `{` body or otherwise positioned the lexer at the `@type` directive.
    /// Reads `@type = "url"` followed by inline fields of the resolved inner
    /// message, packs the inner message to bytes, and writes `type_url` /
    /// `value` onto `target`.
    fn decode_any_inner(&mut self, target: &mut DynamicMessage) -> Result<(), PxfError> {
        let resolver = self
            .type_resolver
            .ok_or_else(|| self.err("internal: decode_any_inner without resolver"))?;
        if !matches!(self.current.kind, TokenKind::AtType) {
            return Err(self.err("Any field requires @type as first entry"));
        }
        self.advance();
        if !matches!(self.current.kind, TokenKind::Equals) {
            return Err(self.err("expected '=' after @type"));
        }
        self.advance();
        if !matches!(self.current.kind, TokenKind::String) {
            return Err(self.err("expected string type URL after @type ="));
        }
        let type_url = std::mem::take(&mut self.current.value);
        let url_pos = self.current.pos;
        self.advance();

        let inner_desc = resolver.find_message_by_url(&type_url).ok_or_else(|| {
            PxfError::new(url_pos, format!("cannot resolve Any type {:?}", type_url))
        })?;
        let mut inner = DynamicMessage::new(inner_desc);
        self.decode_fields(&mut inner, true)?;
        let packed = inner.encode_to_vec();

        let target_desc = target.descriptor();
        let type_url_fd = target_desc.get_field_by_name("type_url").ok_or_else(|| {
            PxfError::new(
                url_pos,
                format!(
                    "internal: {} missing type_url field",
                    target_desc.full_name()
                ),
            )
        })?;
        let value_fd = target_desc.get_field_by_name("value").ok_or_else(|| {
            PxfError::new(
                url_pos,
                format!("internal: {} missing value field", target_desc.full_name()),
            )
        })?;
        target.set_field(&type_url_fd, Value::String(type_url));
        target.set_field(&value_fd, Value::Bytes(packed.into()));
        Ok(())
    }

    /// Try to consume a Timestamp / Duration / wrapper sugar value into `target`.
    /// Returns `Ok(true)` if a WKT shortcut matched and was consumed, `Ok(false)`
    /// to fall through to a regular `{ ... }` block decode.
    fn try_decode_wkt(&mut self, target: &mut DynamicMessage) -> Result<bool, PxfError> {
        let desc = target.descriptor();
        let full = desc.full_name().to_string();

        if full == "google.protobuf.Timestamp" && matches!(self.current.kind, TokenKind::Timestamp)
        {
            let pos = self.current.pos;
            let (seconds, nanos) = parse_rfc3339(&self.current.value).map_err(|e| {
                PxfError::new(
                    pos,
                    format!("invalid timestamp {:?}: {}", self.current.value, e),
                )
            })?;
            set_seconds_nanos(target, seconds, nanos);
            self.advance();
            return Ok(true);
        }
        if full == "google.protobuf.Duration" && matches!(self.current.kind, TokenKind::Duration) {
            let pos = self.current.pos;
            let (seconds, nanos) = parse_go_duration(&self.current.value).map_err(|e| {
                PxfError::new(
                    pos,
                    format!("invalid duration {:?}: {}", self.current.value, e),
                )
            })?;
            set_seconds_nanos(target, seconds, nanos);
            self.advance();
            return Ok(true);
        }
        if is_wrapper_full_name(&full) && !matches!(self.current.kind, TokenKind::LBrace) {
            let value_fd = desc.get_field_by_name("value").ok_or_else(|| {
                self.err(format!("internal: wrapper {} missing 'value' field", full))
            })?;
            let v = self.consume_scalar(&value_fd)?;
            target.set_field(&value_fd, v);
            return Ok(true);
        }
        Ok(false)
    }

    fn consume_scalar(&mut self, fd: &FieldDescriptor) -> Result<Value, PxfError> {
        let pos = self.current.pos;
        let kind = fd.kind();
        match kind {
            Kind::String => {
                if !matches!(self.current.kind, TokenKind::String) {
                    return Err(
                        self.err_at(pos, format!("expected string for field {:?}", fd.name()))
                    );
                }
                let v = Value::String(std::mem::take(&mut self.current.value));
                self.advance();
                Ok(v)
            }
            Kind::Bool => {
                if !matches!(self.current.kind, TokenKind::Bool) {
                    return Err(
                        self.err_at(pos, format!("expected bool for field {:?}", fd.name()))
                    );
                }
                let v = Value::Bool(self.current.value == "true");
                self.advance();
                Ok(v)
            }
            Kind::Int32 | Kind::Sint32 | Kind::Sfixed32 => {
                if !matches!(self.current.kind, TokenKind::Int) {
                    return Err(
                        self.err_at(pos, format!("expected integer for field {:?}", fd.name()))
                    );
                }
                let n: i32 = self.current.value.parse().map_err(|_| {
                    self.err_at(pos, format!("invalid int32: {}", self.current.value))
                })?;
                self.advance();
                Ok(Value::I32(n))
            }
            Kind::Int64 | Kind::Sint64 | Kind::Sfixed64 => {
                if !matches!(self.current.kind, TokenKind::Int) {
                    return Err(
                        self.err_at(pos, format!("expected integer for field {:?}", fd.name()))
                    );
                }
                let n: i64 = self.current.value.parse().map_err(|_| {
                    self.err_at(pos, format!("invalid int64: {}", self.current.value))
                })?;
                self.advance();
                Ok(Value::I64(n))
            }
            Kind::Uint32 | Kind::Fixed32 => {
                if !matches!(self.current.kind, TokenKind::Int) {
                    return Err(
                        self.err_at(pos, format!("expected integer for field {:?}", fd.name()))
                    );
                }
                let n: u32 = self.current.value.parse().map_err(|_| {
                    self.err_at(pos, format!("invalid uint32: {}", self.current.value))
                })?;
                self.advance();
                Ok(Value::U32(n))
            }
            Kind::Uint64 | Kind::Fixed64 => {
                if !matches!(self.current.kind, TokenKind::Int) {
                    return Err(
                        self.err_at(pos, format!("expected integer for field {:?}", fd.name()))
                    );
                }
                let n: u64 = self.current.value.parse().map_err(|_| {
                    self.err_at(pos, format!("invalid uint64: {}", self.current.value))
                })?;
                self.advance();
                Ok(Value::U64(n))
            }
            Kind::Float => {
                if !matches!(self.current.kind, TokenKind::Float | TokenKind::Int) {
                    return Err(
                        self.err_at(pos, format!("expected number for field {:?}", fd.name()))
                    );
                }
                let f: f32 = self.current.value.parse().map_err(|_| {
                    self.err_at(pos, format!("invalid float: {}", self.current.value))
                })?;
                self.advance();
                Ok(Value::F32(f))
            }
            Kind::Double => {
                if !matches!(self.current.kind, TokenKind::Float | TokenKind::Int) {
                    return Err(
                        self.err_at(pos, format!("expected number for field {:?}", fd.name()))
                    );
                }
                let f: f64 = self.current.value.parse().map_err(|_| {
                    self.err_at(pos, format!("invalid double: {}", self.current.value))
                })?;
                self.advance();
                Ok(Value::F64(f))
            }
            Kind::Bytes => {
                if !matches!(self.current.kind, TokenKind::Bytes) {
                    return Err(
                        self.err_at(pos, format!("expected bytes for field {:?}", fd.name()))
                    );
                }
                let decoded = decode_base64(&self.current.value).ok_or_else(|| {
                    self.err_at(pos, format!("invalid base64 for field {:?}", fd.name()))
                })?;
                self.advance();
                Ok(Value::Bytes(decoded.into()))
            }
            Kind::Enum(_) => self.consume_enum(fd),
            Kind::Message(_) => Err(self.err_at(
                pos,
                format!(
                    "internal: consume_scalar called on message field {:?}",
                    fd.name()
                ),
            )),
        }
    }

    fn consume_enum(&mut self, fd: &FieldDescriptor) -> Result<Value, PxfError> {
        let pos = self.current.pos;
        let enum_desc = match fd.kind() {
            Kind::Enum(e) => e,
            _ => {
                return Err(self.err_at(
                    pos,
                    format!("internal: consume_enum on non-enum field {:?}", fd.name()),
                ))
            }
        };
        match self.current.kind {
            TokenKind::Ident => {
                let ev = enum_desc
                    .get_value_by_name(&self.current.value)
                    .ok_or_else(|| {
                        self.err_at(
                            pos,
                            format!(
                                "unknown enum value {:?} for {}",
                                self.current.value,
                                enum_desc.full_name()
                            ),
                        )
                    })?;
                self.advance();
                Ok(Value::EnumNumber(ev.number()))
            }
            TokenKind::Int => {
                let n: i32 = self.current.value.parse().map_err(|_| {
                    self.err_at(pos, format!("invalid enum number: {}", self.current.value))
                })?;
                self.advance();
                Ok(Value::EnumNumber(n))
            }
            _ => Err(self.err_at(
                pos,
                format!("expected enum name or number for field {:?}", fd.name()),
            )),
        }
    }

    fn skip_value(&mut self) {
        match self.current.kind {
            TokenKind::LBrace => {
                self.advance();
                self.skip_braced();
            }
            TokenKind::LBracket => {
                self.advance();
                self.skip_bracketed();
            }
            _ => self.advance(),
        }
    }

    fn skip_braced(&mut self) {
        let mut depth: i32 = 1;
        while depth > 0 && !matches!(self.current.kind, TokenKind::Eof) {
            match self.current.kind {
                TokenKind::LBrace => depth += 1,
                TokenKind::RBrace => depth -= 1,
                _ => {}
            }
            self.advance();
        }
    }

    fn skip_bracketed(&mut self) {
        let mut depth: i32 = 1;
        while depth > 0 && !matches!(self.current.kind, TokenKind::Eof) {
            match self.current.kind {
                TokenKind::LBracket => depth += 1,
                TokenKind::RBracket => depth -= 1,
                _ => {}
            }
            self.advance();
        }
    }
}

fn is_any_full_name(full: &str) -> bool {
    full == "google.protobuf.Any"
}

/// Validate `(pxf.required)` annotations and apply `(pxf.default)` values to
/// absent fields. Recurses into present, non-null nested message fields,
/// matching the Go reference's `postDecode`. The `_null` field itself is
/// skipped since it carries metadata, not user data.
fn post_decode(
    parent: &mut DynamicMessage,
    presence: &Presence,
    null_mask_fd: Option<&FieldDescriptor>,
    path_prefix: &str,
) -> Result<(), PxfError> {
    let desc = parent.descriptor();
    let pos = Position::new(1, 1);
    let fields: Vec<FieldDescriptor> = desc.fields().collect();
    for fd in &fields {
        if let Some(null_fd) = null_mask_fd {
            if fd.number() == null_fd.number() {
                continue;
            }
        }
        let path = format!("{}{}", path_prefix, fd.name());
        if presence.is_absent(&path) {
            if is_required(fd) {
                return Err(PxfError::new(
                    pos,
                    format!("required field {:?} is absent", path),
                ));
            }
            if let Some(def) = get_default(fd) {
                apply_default(parent, fd, &def, pos)?;
            }
            continue;
        }
        if presence.is_null(&path) {
            continue;
        }
        if let Kind::Message(inner) = fd.kind() {
            if !fd.is_list()
                && !fd.is_map()
                && !is_wkt_skip_recursion(inner.full_name())
                && parent.has_field(fd)
            {
                let mut sub = match parent.get_field(fd).into_owned() {
                    Value::Message(m) => m,
                    _ => continue,
                };
                let next_prefix = format!("{}.", path);
                post_decode(&mut sub, presence, None, &next_prefix)?;
                parent.set_field(fd, Value::Message(sub));
            }
        }
    }
    Ok(())
}

fn is_wkt_skip_recursion(full: &str) -> bool {
    full == "google.protobuf.Timestamp"
        || full == "google.protobuf.Duration"
        || full == "google.protobuf.Any"
        || is_wrapper_full_name(full)
}

fn apply_default(
    parent: &mut DynamicMessage,
    fd: &FieldDescriptor,
    def: &str,
    pos: Position,
) -> Result<(), PxfError> {
    if let Kind::Enum(enum_desc) = fd.kind() {
        if let Some(ev) = enum_desc.get_value_by_name(def) {
            parent.set_field(fd, Value::EnumNumber(ev.number()));
            return Ok(());
        }
        let n: i32 = def.parse().map_err(|_| {
            PxfError::new(
                pos,
                format!("invalid default enum {:?} for field {:?}", def, fd.name()),
            )
        })?;
        parent.set_field(fd, Value::EnumNumber(n));
        return Ok(());
    }
    if let Kind::Message(_) = fd.kind() {
        return apply_message_default(parent, fd, def, pos);
    }
    let v = parse_scalar_default(fd, def, pos)?;
    parent.set_field(fd, v);
    Ok(())
}

fn parse_scalar_default(fd: &FieldDescriptor, def: &str, pos: Position) -> Result<Value, PxfError> {
    fn err(pos: Position, kind: &str, def: &str, name: &str) -> PxfError {
        PxfError::new(
            pos,
            format!("invalid default {} {:?} for field {:?}", kind, def, name),
        )
    }
    let name = fd.name();
    Ok(match fd.kind() {
        Kind::String => Value::String(def.to_string()),
        Kind::Bool => Value::Bool(def == "true"),
        Kind::Int32 | Kind::Sint32 | Kind::Sfixed32 => {
            Value::I32(def.parse().map_err(|_| err(pos, "int32", def, name))?)
        }
        Kind::Int64 | Kind::Sint64 | Kind::Sfixed64 => {
            Value::I64(def.parse().map_err(|_| err(pos, "int64", def, name))?)
        }
        Kind::Uint32 | Kind::Fixed32 => {
            Value::U32(def.parse().map_err(|_| err(pos, "uint32", def, name))?)
        }
        Kind::Uint64 | Kind::Fixed64 => {
            Value::U64(def.parse().map_err(|_| err(pos, "uint64", def, name))?)
        }
        Kind::Float => Value::F32(def.parse().map_err(|_| err(pos, "float", def, name))?),
        Kind::Double => Value::F64(def.parse().map_err(|_| err(pos, "double", def, name))?),
        Kind::Bytes => Value::Bytes(
            decode_base64(def)
                .ok_or_else(|| err(pos, "bytes", def, name))?
                .into(),
        ),
        other => {
            return Err(PxfError::new(
                pos,
                format!(
                    "unsupported default scalar kind {:?} for field {:?}",
                    other, name
                ),
            ));
        }
    })
}

fn apply_message_default(
    parent: &mut DynamicMessage,
    fd: &FieldDescriptor,
    def: &str,
    pos: Position,
) -> Result<(), PxfError> {
    let inner_desc = match fd.kind() {
        Kind::Message(m) => m,
        _ => unreachable!("apply_message_default on non-message"),
    };
    let full = inner_desc.full_name().to_string();
    let mut sub = DynamicMessage::new(inner_desc);

    if full == "google.protobuf.Timestamp" {
        let (s, n) = parse_rfc3339(def).map_err(|e| {
            PxfError::new(
                pos,
                format!(
                    "invalid default timestamp {:?} for field {:?}: {}",
                    def,
                    fd.name(),
                    e
                ),
            )
        })?;
        set_seconds_nanos(&mut sub, s, n);
        parent.set_field(fd, Value::Message(sub));
        return Ok(());
    }
    if full == "google.protobuf.Duration" {
        let (s, n) = parse_go_duration(def).map_err(|e| {
            PxfError::new(
                pos,
                format!(
                    "invalid default duration {:?} for field {:?}: {}",
                    def,
                    fd.name(),
                    e
                ),
            )
        })?;
        set_seconds_nanos(&mut sub, s, n);
        parent.set_field(fd, Value::Message(sub));
        return Ok(());
    }
    if is_wrapper_full_name(&full) {
        let value_fd = sub.descriptor().get_field_by_name("value").ok_or_else(|| {
            PxfError::new(
                pos,
                format!("internal: wrapper {} missing 'value' field", full),
            )
        })?;
        let v = parse_scalar_default(&value_fd, def, pos)?;
        sub.set_field(&value_fd, v);
        parent.set_field(fd, Value::Message(sub));
        return Ok(());
    }
    Err(PxfError::new(
        pos,
        format!(
            "default values not supported for message type {} (field {:?})",
            full,
            fd.name()
        ),
    ))
}

fn is_wrapper_full_name(full: &str) -> bool {
    matches!(
        full,
        "google.protobuf.DoubleValue"
            | "google.protobuf.FloatValue"
            | "google.protobuf.Int64Value"
            | "google.protobuf.UInt64Value"
            | "google.protobuf.Int32Value"
            | "google.protobuf.UInt32Value"
            | "google.protobuf.BoolValue"
            | "google.protobuf.StringValue"
            | "google.protobuf.BytesValue"
    )
}

fn set_seconds_nanos(target: &mut DynamicMessage, seconds: i64, nanos: i32) {
    let desc = target.descriptor();
    if let Some(s_fd) = desc.get_field_by_name("seconds") {
        target.set_field(&s_fd, Value::I64(seconds));
    }
    if let Some(n_fd) = desc.get_field_by_name("nanos") {
        target.set_field(&n_fd, Value::I32(nanos));
    }
}

/// Coerce an owned key string into a [`MapKey`]. For string-typed maps the
/// String moves directly into `MapKey::String` (no extra allocation); for
/// numeric/bool maps the String is parsed and dropped.
fn decode_map_key(
    key_fd: &FieldDescriptor,
    key: String,
    pos: Position,
) -> Result<MapKey, PxfError> {
    match key_fd.kind() {
        Kind::String => Ok(MapKey::String(key)),
        Kind::Int32 | Kind::Sint32 | Kind::Sfixed32 => {
            let n: i32 = key
                .parse()
                .map_err(|_| PxfError::new(pos, format!("invalid int32 map key: {}", key)))?;
            Ok(MapKey::I32(n))
        }
        Kind::Int64 | Kind::Sint64 | Kind::Sfixed64 => {
            let n: i64 = key
                .parse()
                .map_err(|_| PxfError::new(pos, format!("invalid int64 map key: {}", key)))?;
            Ok(MapKey::I64(n))
        }
        Kind::Uint32 | Kind::Fixed32 => {
            let n: u32 = key
                .parse()
                .map_err(|_| PxfError::new(pos, format!("invalid uint32 map key: {}", key)))?;
            Ok(MapKey::U32(n))
        }
        Kind::Uint64 | Kind::Fixed64 => {
            let n: u64 = key
                .parse()
                .map_err(|_| PxfError::new(pos, format!("invalid uint64 map key: {}", key)))?;
            Ok(MapKey::U64(n))
        }
        Kind::Bool => match key.as_str() {
            "true" => Ok(MapKey::Bool(true)),
            "false" => Ok(MapKey::Bool(false)),
            _ => Err(PxfError::new(pos, format!("invalid bool map key: {}", key))),
        },
        other => Err(PxfError::new(
            pos,
            format!("unsupported map key kind: {:?}", other),
        )),
    }
}

/// Parse an RFC 3339 timestamp into seconds-since-epoch and a non-negative
/// nanos remainder. The lexer has already validated syntactic shape, so this
/// only reads the slots it knows are there. Hand-rolled to avoid pulling in
/// a chrono/time crate dependency just for two well-defined formats.
fn parse_rfc3339(s: &str) -> Result<(i64, i32), String> {
    let bytes = s.as_bytes();
    if bytes.len() < 20 {
        return Err("too short".into());
    }
    let year = parse_int_n(&bytes[0..4])? as i32;
    let month = parse_int_n(&bytes[5..7])? as u32;
    let day = parse_int_n(&bytes[8..10])? as u32;
    let hour = parse_int_n(&bytes[11..13])? as i64;
    let minute = parse_int_n(&bytes[14..16])? as i64;
    let second = parse_int_n(&bytes[17..19])? as i64;

    let mut i = 19;
    let mut nanos: i32 = 0;
    if i < bytes.len() && bytes[i] == b'.' {
        i += 1;
        let frac_start = i;
        while i < bytes.len() && bytes[i].is_ascii_digit() {
            i += 1;
        }
        let mut digits: Vec<u8> = bytes[frac_start..i].to_vec();
        if digits.len() > 9 {
            digits.truncate(9);
        }
        while digits.len() < 9 {
            digits.push(b'0');
        }
        nanos = parse_int_n(&digits)? as i32;
    }

    let mut offset_seconds: i64 = 0;
    if i >= bytes.len() {
        return Err("missing zone".into());
    }
    match bytes[i] {
        b'Z' | b'z' => {}
        b'+' | b'-' => {
            if bytes.len() < i + 6 {
                return Err("malformed offset".into());
            }
            let neg = bytes[i] == b'-';
            let off_h = parse_int_n(&bytes[i + 1..i + 3])? as i64;
            let off_m = parse_int_n(&bytes[i + 4..i + 6])? as i64;
            offset_seconds = (off_h * 3600 + off_m * 60) * if neg { -1 } else { 1 };
        }
        _ => return Err("unexpected zone".into()),
    }

    let days = days_from_civil(year, month, day);
    let seconds = days * 86400 + hour * 3600 + minute * 60 + second - offset_seconds;
    Ok((seconds, nanos))
}

fn parse_int_n(bytes: &[u8]) -> Result<u64, String> {
    let mut n: u64 = 0;
    for &b in bytes {
        if !b.is_ascii_digit() {
            return Err("non-digit".into());
        }
        n = n * 10 + (b - b'0') as u64;
    }
    Ok(n)
}

/// Howard Hinnant's "days from civil" — converts a (year, month, day) in the
/// proleptic Gregorian calendar to days since the Unix epoch (1970-01-01).
fn days_from_civil(y: i32, m: u32, d: u32) -> i64 {
    let y = if m <= 2 { y - 1 } else { y };
    let era = if y >= 0 { y / 400 } else { (y - 399) / 400 };
    let yoe = (y - era * 400) as u32;
    let m_shift = if m > 2 { m - 3 } else { m + 9 };
    let doy = (153 * m_shift + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era as i64 * 146097 + doe as i64 - 719468
}

/// Parse a Go-style duration (`1h30m`, `-2.5s`, `100ms`, …) into proto
/// Duration `seconds` + `nanos`. Both fields share the overall sign per the
/// proto Duration invariant. Lexer has pre-validated the syntax.
fn parse_go_duration(s: &str) -> Result<(i64, i32), String> {
    if s == "0" {
        return Ok((0, 0));
    }
    let bytes = s.as_bytes();
    let mut i = 0;
    let mut neg = false;
    if !bytes.is_empty() && (bytes[0] == b'-' || bytes[0] == b'+') {
        neg = bytes[0] == b'-';
        i = 1;
    }
    let mut total_nanos: i128 = 0;
    while i < bytes.len() {
        let int_start = i;
        while i < bytes.len() && bytes[i].is_ascii_digit() {
            i += 1;
        }
        if i == int_start {
            return Err("missing digits".into());
        }
        let int_part: i128 = std::str::from_utf8(&bytes[int_start..i])
            .map_err(|_| "invalid utf-8")?
            .parse()
            .map_err(|_| "int overflow")?;
        let mut frac_int: i128 = 0;
        let mut frac_len = 0usize;
        if i < bytes.len() && bytes[i] == b'.' {
            i += 1;
            let frac_start = i;
            while i < bytes.len() && bytes[i].is_ascii_digit() {
                i += 1;
            }
            if i == frac_start {
                return Err("missing fractional digits".into());
            }
            frac_int = std::str::from_utf8(&bytes[frac_start..i])
                .map_err(|_| "invalid utf-8")?
                .parse()
                .map_err(|_| "frac overflow")?;
            frac_len = i - frac_start;
        }
        if i >= bytes.len() {
            return Err("missing unit".into());
        }
        let next = bytes.get(i + 1).copied();
        let (unit_nanos, unit_len): (i128, usize) = match (bytes[i], next) {
            (b'n', Some(b's')) => (1, 2),
            (b'u', Some(b's')) => (1_000, 2),
            (b'm', Some(b's')) => (1_000_000, 2),
            (b's', _) => (1_000_000_000, 1),
            (b'm', _) => (60_000_000_000, 1),
            (b'h', _) => (3_600_000_000_000, 1),
            _ => return Err("unknown unit".into()),
        };
        total_nanos += int_part * unit_nanos;
        if frac_len > 0 {
            let denom: i128 = 10i128.pow(frac_len as u32);
            total_nanos += (frac_int * unit_nanos) / denom;
        }
        i += unit_len;
    }
    if neg {
        total_nanos = -total_nanos;
    }
    let sign: i128 = if total_nanos < 0 { -1 } else { 1 };
    let abs = total_nanos * sign;
    let seconds = (abs / 1_000_000_000) * sign;
    let nanos = (abs % 1_000_000_000) * sign;
    Ok((seconds as i64, nanos as i32))
}

/// Decode a base64 string, accepting both standard (padded) and raw (unpadded)
/// alphabets — matching the Go reference's `StdEncoding`/`RawStdEncoding`
/// fallback. The lexer has already validated the alphabet, so we only need to
/// handle padding here.
pub(crate) fn decode_base64(s: &str) -> Option<Vec<u8>> {
    if s.is_empty() {
        return Some(Vec::new());
    }
    let bytes = s.as_bytes();
    let mut padded: Vec<u8>;
    let input: &[u8] = if bytes.len() % 4 == 0 {
        bytes
    } else {
        padded = Vec::with_capacity(bytes.len() + 4);
        padded.extend_from_slice(bytes);
        while padded.len() % 4 != 0 {
            padded.push(b'=');
        }
        &padded
    };
    let mut out = Vec::with_capacity(input.len() / 4 * 3);
    let mut buf: u32 = 0;
    let mut bits: u32 = 0;
    for &b in input {
        if b == b'=' {
            break;
        }
        let v = match b {
            b'A'..=b'Z' => b - b'A',
            b'a'..=b'z' => b - b'a' + 26,
            b'0'..=b'9' => b - b'0' + 52,
            b'+' => 62,
            b'/' => 63,
            _ => return None,
        };
        buf = (buf << 6) | v as u32;
        bits += 6;
        if bits >= 8 {
            bits -= 8;
            out.push(((buf >> bits) & 0xff) as u8);
        }
    }
    Some(out)
}
