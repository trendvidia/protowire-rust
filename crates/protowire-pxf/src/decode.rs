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

use prost_reflect::{
    Cardinality, DescriptorPool, DynamicMessage, FieldDescriptor, Kind, MapKey, MessageDescriptor,
    OneofDescriptor, ReflectMessage, Value,
};
use std::collections::{HashMap, HashSet};

use crate::annotations::find_null_mask_field;
use crate::ast::{
    DatasetDirective, DatasetRow, Directive, ProtoDirective, ProtoShape, Value as AstValue,
};
use crate::bigfloat::{clip_literal, parse_big_float, parse_big_float_default, BigFloatLit};
use crate::bignum::{parse_big_int, parse_decimal, BigIntLit, DecimalLit};
use crate::dataset_paths::column_path_error;
use crate::errors::PxfError;
use crate::go_strconv::{
    decode_base64_lenient, parse_float, parse_int, parse_uint, quote, quote_bytes,
};
use crate::keyed::{
    duplicate_key_error, empty_name_error, key_field, quoted_name_unkeyed_error,
    quoted_name_unkeyed_error_bytes, KeyedElemState,
};
use crate::lexer::Lexer;
use crate::limits::{Limits, MAX_NUMERIC_LITERAL_DIGITS};
use crate::parser::{
    find_matching_brace, non_name_assign_error, non_name_block_error, read_dropped_value,
    read_value, top_level_map_entry_error,
};
use crate::post_decode_plan::post_decode_plan;
use crate::result::Presence;
use crate::token::{Position, Token, TokenKind};

/// The error for a type entry anywhere but the start of an Any block.
pub(crate) const TYPE_ENTRY_MISPLACED: &str =
    "@type is allowed only as the first entry of a block bound to google.protobuf.Any (draft -01 \"Any Blocks\")";

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

/// The hook [`UnmarshalOptions::on_secret_field`] calls with a
/// `pxf.Secret`'s path and value. An error aborts the decode. It may borrow
/// from its surroundings for `'a`, the options' lifetime.
pub type SecretHook<'a> =
    dyn Fn(&str, &str) -> Result<(), Box<dyn std::error::Error + Send + Sync>> + 'a;

/// Options controlling [`unmarshal`] behavior.
#[derive(Default, Clone, Copy)]
pub struct UnmarshalOptions<'a> {
    /// Silently skip fields not declared in the schema instead of erroring.
    pub discard_unknown: bool,
    /// Resolves type URLs for `google.protobuf.Any` fields. A block bound to
    /// Any that opens with `@type = "..."` needs it, and is an error without
    /// it; a block without a type entry is the Any's own `type_url` and
    /// `value` fields either way.
    pub type_resolver: Option<&'a dyn TypeResolver>,
    /// Decode without the post-decode pass: `(pxf.default)` values are not
    /// applied to absent fields and `(pxf.required)` fields are not checked,
    /// at the root, in list elements and map values, and in `Any` bodies.
    /// Presence, and with it the root's `_null` mask, is still recorded. A
    /// layered-config caller sets it because defaults and required belong
    /// to the merged result, not to one layer: a base layer may omit a
    /// required field a higher layer supplies. It then runs its own pass
    /// with [`crate::apply_default`] and [`crate::annotations::is_required`]
    /// over [`crate::Presence::present_fields`]. protowire-go's
    /// `SkipPostDecode`.
    pub skip_post_decode: bool,
    /// Called with each `pxf.Secret` value instead of storing it, so the
    /// secret stays out of the decoded message (protowire-go's
    /// `OnSecretField`). It fires for every form a value takes: `pw = "x"`,
    /// a list element or a map value of that form, and the block form
    /// `pw { value = "x" hint = "h" }`, wherever the Secret sits, an `Any`
    /// body included. It gets the field's dotted path and the value:
    ///
    /// - `pw = "x"` → `"pw"`; `db { password = "x" }` → `"db.password"`
    /// - `servers = [{ password = "x" }]` → `"servers[0].password"`
    /// - `by_host = { "a": { password = "x" } }` → `"by_host[\"a\"].password"`
    /// - `keys = ["a", "b"]` → `"keys[0]"`, `"keys[1]"`; `by_id = { 7: "k" }` → `"by_id[7]"`
    ///
    /// The Secret's own `value` is left unset, though presence still
    /// reports it set; `hint` and `fingerprint` are stored as usual. An
    /// error from the hook aborts the decode, as `pxf.Secret hook for
    /// "<path>": <error>`. The plaintext is still a substring of the input
    /// for the length of the call; the hook keeps it out of the message.
    pub on_secret_field: Option<&'a SecretHook<'a>>,
    /// Validates the decoded message after the decode, and for
    /// [`unmarshal_full`] after its post-decode pass (protowire-go's
    /// `Validator`). It receives the [`DynamicMessage`] as a `&dyn Any`.
    /// When it reports violations the decode fails with a [`PxfError`] that
    /// has no position, whose text is the [`protowire_check::Error`]'s, and
    /// whose [`PxfError::check_error`] holds the report. An error of the
    /// validator's own is returned the same way, as the error's source. A
    /// clean report is [`Presence::report`].
    ///
    /// It is unrelated to [`Self::skip_validate`], which skips the
    /// bind-time checks of the schema; this checks the data against a rule
    /// set of the validator's (`buf.validate` annotations through a
    /// protovalidate adapter, say). A `@dataset` row bound with
    /// [`crate::DatasetReader::scan_one`] or [`crate::bind_row`] is
    /// validated as it is bound.
    pub validator: Option<&'a dyn protowire_check::Validator>,
    /// Skip the per-call schema reserved-name check (draft §3.13).
    /// Callers that have already validated their descriptors (typically
    /// via [`crate::validate_descriptor`] in a one-time codegen or
    /// registry-load pass) can set this to bypass the per-call recheck.
    pub skip_validate: bool,
    /// The draft's per-call limits (draft -01 § Mandatory Limits); the
    /// default is the HARDENING constants. `max_message_size` is checked
    /// before the first token is read; `max_nesting_depth` caps block /
    /// list nesting; `max_numeric_literal_digits` the digits of a
    /// `pxf.BigInt` / `pxf.Decimal` literal; `max_bytes_literal_length`
    /// the decoded length of a `b"…"` literal, judged from its length;
    /// `max_repeated_count` the elements of a repeated field or the
    /// entries of a map. Schema literals — a `(pxf.default)` — stay under
    /// the constants.
    pub limits: Limits,
}

/// Decode PXF text into a fresh [`DynamicMessage`] for `desc`.
pub fn unmarshal(
    data: &str,
    desc: &MessageDescriptor,
    options: UnmarshalOptions<'_>,
) -> Result<DynamicMessage, PxfError> {
    let (msg, _) = unmarshal_inner(data, desc, options, false)?;
    protowire_check::validate(options.validator, &msg).map_err(PxfError::from_validator)?;
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
    let mut presence = presence.expect("presence requested");
    let report =
        protowire_check::validate(options.validator, &msg).map_err(PxfError::from_validator)?;
    presence.set_report(report);
    Ok((msg, presence))
}

fn unmarshal_inner(
    data: &str,
    desc: &MessageDescriptor,
    options: UnmarshalOptions<'_>,
    track_presence: bool,
) -> Result<(DynamicMessage, Option<Presence>), PxfError> {
    options
        .limits
        .check_message_size(data.len())
        .map_err(|m| PxfError::new(Position::new(1, 1), m))?;
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
        options.limits,
    );
    decoder.post_decode_any = !options.skip_post_decode;
    decoder.on_secret = options.on_secret_field;
    decoder.advance();
    decoder.consume_directives()?;

    let mut msg = DynamicMessage::new(desc.clone());
    decoder.decode_fields(&mut msg, false)?;

    let presence = if track_presence {
        let null_mask_fd = find_null_mask_field(desc);
        let presence = decoder.into_presence();
        if !options.skip_post_decode {
            post_decode(&mut msg, &presence, null_mask_fd.as_ref(), "")?;
        }
        // Mirror null paths into the root's _null FieldMask, if any.
        if let Some(null_mask_fd) = null_mask_fd {
            // A FieldMask path names fields, not list elements or map
            // entries, so a null inside one stays in Presence only
            // (protowire-go#170). `[` is in no field name. The paths keep
            // document order, as protowire-go appends each as it binds the
            // null (#174).
            let paths: Vec<String> = presence
                .null_paths()
                .filter(|p| !p.contains('['))
                .map(|s| s.to_string())
                .collect();
            if !paths.is_empty() {
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

/// Which reader takes a block's body ([`Decoder::body_form`]).
enum BodyForm {
    /// A `pxf.Secret` block with the hook set, at this path.
    Secret(String),
    /// An Any's body, opening with `@type`.
    Any,
    /// The message's own fields.
    Fields,
}

/// What one body's entries have bound so far: each message body is its own
/// scope.
struct BodyScope {
    set_oneofs: HashMap<String, String>,
    /// The singular fields this body has bound (draft -01 § Entries and
    /// Keys; protowire#347).
    bound: HashSet<u32>,
    /// The keyed-element checks, when the body is one element of a keyed
    /// repeated field.
    ke: Option<KeyedElemState>,
}

/// A list element or map value as far as its body.
enum Elem {
    /// Read whole.
    Value(Value),
    /// A message's block, past its `{`.
    Body(DynamicMessage),
}

/// What reads the value of the entry [`Decoder::next_entry`] stopped at.
enum Step {
    /// The body's end.
    End,
    /// Nothing: the entry was read whole.
    Read,
    /// `name = value`, at the value.
    Value(FieldDescriptor),
    /// A keyed repeated field's block body (draft -01 §3.13), past its `{`.
    Keyed(FieldDescriptor, FieldDescriptor),
    /// A message field's block body, past its `{`.
    Block(FieldDescriptor, DynamicMessage),
}

struct Decoder<'a> {
    lex: Lexer<'a>,
    current: Token,
    discard_unknown: bool,
    type_resolver: Option<&'a dyn TypeResolver>,
    presence: Option<Presence>,
    path_prefix: String,
    /// Live `{` + `[` depth, mirrors the parser's counter. Capped at
    /// `limits.max_nesting_depth` per HARDENING.md §Recursion. Threaded
    /// across `decode_fields` / `decode_list_inline` / `decode_map_inline`
    /// / `decode_any_inner` so adversarial deep input rejects before the
    /// recursive descent overflows the native stack.
    depth: usize,
    /// This call's limits (`UnmarshalOptions::limits`), applied to the
    /// document. Schema literals — a `(pxf.default)` — stay under the
    /// constants: they are the schema author's, not the document's.
    limits: Limits,
    /// Context for the next `decode_fields` call: it decodes one element
    /// of a keyed repeated field (draft -01 §3.13), whose key-field
    /// assignments are checked against the entry name.
    keyed_elem: Option<KeyedElemState>,
    /// Whether an `Any` body takes the post-decode pass before it is
    /// packed: false under [`UnmarshalOptions::skip_post_decode`].
    post_decode_any: bool,
    /// [`UnmarshalOptions::on_secret_field`].
    on_secret: Option<&'a SecretHook<'a>>,
    /// Set for the one `decode_fields` call that reads an `Any`'s body, which
    /// is never a Secret block, whatever type it packs.
    any_body: bool,
    /// The bytes of the current token when it is a string literal that is
    /// not UTF-8 (#170). The token is `Illegal`, so a site that does not
    /// look refuses it with the lexer's text; the sites that look give
    /// protowire-go's text for where the string stood, or, where
    /// protowire-go never reads the string, take it.
    bad_string: Option<Vec<u8>>,
}

impl<'a> Decoder<'a> {
    /// Whether the current token can be a float or double: a number, or
    /// the unsigned identifiers `inf` and `nan` (the signed `+inf` and
    /// `-inf` lex as floats).
    fn at_float_literal(&self) -> bool {
        match self.current.kind {
            TokenKind::Float | TokenKind::Int => true,
            TokenKind::Ident => matches!(self.current.value.as_str(), "inf" | "nan"),
            _ => false,
        }
    }

    fn new(
        input: &'a str,
        discard_unknown: bool,
        type_resolver: Option<&'a dyn TypeResolver>,
        track_presence: bool,
        limits: Limits,
    ) -> Self {
        Self {
            lex: Lexer::with_limits(input, limits.max_bytes_literal_length),
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
            limits,
            keyed_elem: None,
            post_decode_any: true,
            on_secret: None,
            any_body: false,
            bad_string: None,
        }
        .keeping_invalid_strings()
    }

    /// The decoder reads the bytes of a string literal that is not UTF-8,
    /// to say where it stood or to take it (#170).
    fn keeping_invalid_strings(mut self) -> Self {
        self.lex.keep_invalid_strings();
        self
    }

    fn enter(&mut self) -> Result<(), PxfError> {
        if self.depth >= self.limits.max_nesting_depth {
            return Err(self.err(format!(
                "nesting depth exceeds MaxNestingDepth={}",
                self.limits.max_nesting_depth
            )));
        }
        self.depth += 1;
        Ok(())
    }

    /// Report the lexer's own diagnostic when the decoder sits on an
    /// `Illegal` token. `Illegal` is admitted at no position in the
    /// grammar, so at a value position there is nothing the decoder's own
    /// expectation can add: "5seconds" is not a badly-shaped message, it is
    /// not a token at all, and a `b"…"` past `MaxBytesLiteralLength` must
    /// name the limit rather than "expected bytes". Call before deciding
    /// what was wanted instead.
    fn illegal(&self) -> Result<(), PxfError> {
        if matches!(self.current.kind, TokenKind::Illegal) && self.bad_string.is_none() {
            return Err(self.err(self.current.value.clone()));
        }
        Ok(())
    }

    fn leave(&mut self) {
        self.depth -= 1;
    }

    fn into_presence(self) -> Presence {
        self.presence.expect("presence not requested")
    }

    /// Extend the presence path for the `i`th element of a list, `name[i].`,
    /// returning the path to restore once the body is decoded. Every
    /// descent into a message body extends it, so the marks made inside
    /// land on the message they belong to, never on its parent, where they
    /// would satisfy the parent's `(pxf.required)` or keep its
    /// `(pxf.default)` off.
    fn enter_index(&mut self, name: &str, i: usize) -> String {
        if !self.tracks_paths() {
            return String::new();
        }
        let saved = std::mem::take(&mut self.path_prefix);
        self.path_prefix = format!("{saved}{name}[{i}].");
        saved
    }

    /// [`Self::enter_index`] for a singular field's body: `name.`.
    #[cfg_attr(not(debug_assertions), inline(always))]
    fn enter_field(&mut self, name: &str) -> String {
        if !self.tracks_paths() {
            return String::new();
        }
        let saved = std::mem::take(&mut self.path_prefix);
        self.path_prefix = format!("{saved}{name}.");
        saved
    }

    /// [`Self::enter_index`] for the value of map key `key`: `name["k"].`.
    fn enter_key(&mut self, name: &str, key: &MapKey) -> String {
        if !self.tracks_paths() {
            return String::new();
        }
        let saved = std::mem::take(&mut self.path_prefix);
        self.path_prefix = format!("{saved}{name}[{}].", shown_key(key));
        saved
    }

    /// Whether anything reads the path: the presence of `unmarshal_full`,
    /// or the Secret hook, which is given it. Plain `unmarshal` without the
    /// hook reads neither, so it does not build them.
    fn tracks_paths(&self) -> bool {
        self.presence.is_some() || self.on_secret.is_some()
    }

    /// Hand a Secret's value to the hook, as the field at `path`, and turn
    /// its error into the decode's, at `pos`.
    fn call_secret_hook(&self, path: &str, value: &str, pos: Position) -> Result<(), PxfError> {
        let hook = self.on_secret.expect("called with a hook");
        hook(path, value).map_err(|e| {
            PxfError::new(
                pos,
                format!(
                    "pxf.Secret hook for {}: {e}",
                    crate::go_strconv::quote(path)
                ),
            )
        })
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
                self.bad_string = self.lex.take_invalid_string(self.current.pos);
                return;
            }
        }
    }

    /// After the parser has read a value over this decoder's lexer: the
    /// current token is the parser's, and so are its kept bytes.
    fn after_parser(&mut self) {
        self.bad_string = self.lex.take_invalid_string(self.current.pos);
        if self.bad_string.is_some() {
            // A parser reading a dropped value turns it into a string; the
            // decoder sees it as its own lexer gives it.
            self.current.kind = TokenKind::Illegal;
            self.current.value = "string contains invalid UTF-8".into();
        }
    }

    /// peek_kind: one-token lookahead without consumption. Used by
    /// the directive-prefix disambiguator in consume_directives.
    fn peek_kind(&mut self) -> TokenKind {
        let snap = self.lex.snapshot();
        let saved = self.current.clone();
        let bad = self.bad_string.take();
        self.advance();
        let k = self.current.kind;
        self.lex.restore(snap);
        self.current = saved;
        self.bad_string = bad;
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
    /// One cell of a @dataset row at `self.current`: `None` for an empty
    /// cell, otherwise any value, list and block included (draft §3.4.4;
    /// protowire#341), read as `parse` reads it.
    fn read_cell(&mut self) -> Result<Option<AstValue>, PxfError> {
        if matches!(self.current.kind, TokenKind::Comma | TokenKind::RParen) {
            return Ok(None);
        }
        let read = read_value(&mut self.lex, &mut self.current, 0, self.limits).map(Some);
        self.after_parser();
        read
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
                    // `type-directive = %s"@type" 1*WSP identifier` (draft
                    // -01 § The "@type" Directive), as `parse` reads it (#193).
                    self.advance(); // consume @type
                    if !matches!(self.current.kind, TokenKind::Ident) {
                        return Err(self.err(format!(
                            "expected type name after @type, got {}",
                            self.token_err_msg()
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
                    // Optional inline block. It is read as `parse` reads it,
                    // so a body that is not PXF is an error here too, not
                    // skipped by counting braces; `body` stays the raw bytes.
                    if matches!(self.current.kind, TokenKind::LBrace) {
                        let open = self.current.pos.offset;
                        self.read_dropped_value(0)?;
                        let input = self.lex.input_view();
                        let close = find_matching_brace(input, open);
                        if close < 0 {
                            return Err(
                                self.err_at(at_pos, format!("directive @{}: unmatched '{{'", name))
                            );
                        }
                        body = input[open + 1..close as usize].to_vec();
                        has_body = true;
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
                    let mut column_pos: Vec<Position> = Vec::new();
                    loop {
                        if !matches!(self.current.kind, TokenKind::Ident) {
                            return Err(self.err("expected column field name"));
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
                        return Err(self.err("expected ',' or ')' in @dataset column list"));
                    }
                    if let Some((i, msg)) = column_path_error(&columns) {
                        return Err(self.err_at(column_pos[i], msg));
                    }
                    self.advance(); // consume )
                    let n_cols = columns.len();
                    let mut rows: Vec<DatasetRow> = Vec::new();
                    // Zero or more rows.
                    while matches!(self.current.kind, TokenKind::LParen) {
                        let row_pos = self.current.pos;
                        self.advance(); // (
                        let mut cells: Vec<Option<AstValue>> = Vec::with_capacity(n_cols);
                        cells.push(self.read_cell()?);
                        while matches!(self.current.kind, TokenKind::Comma) {
                            self.advance();
                            cells.push(self.read_cell()?);
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
                            leading_comments: Vec::new(),
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
                        // The body is bytes, whatever they are (#170).
                        TokenKind::Illegal if self.bad_string.is_some() => {
                            let body = self.bad_string.take().unwrap_or_default();
                            self.advance();
                            (ProtoShape::Source, String::new(), body)
                        }
                        TokenKind::Bytes => {
                            let raw = std::mem::take(&mut self.current.value);
                            let decoded = crate::go_strconv::decode_base64_descriptor(&raw)
                                .map_err(|e| {
                                    self.err(format!("@proto descriptor body: invalid base64: {e}"))
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

    /// The current token, as an error names it: protowire-go's
    /// `tokenErrMsg`, an ILLEGAL token's own text or `kind ("value")`.
    fn token_err_msg(&self) -> String {
        if let Some(b) = &self.bad_string {
            return format!("string ({})", quote_bytes(b));
        }
        if matches!(self.current.kind, TokenKind::Illegal) {
            return self.current.value.clone();
        }
        format!("{} ({:?})", self.current.kind, self.current.value)
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
        // Chosen here, not in the loop that reads a body's fields, so an
        // Any's body holds one such loop on the stack and not two (#179).
        let result = match self.body_form(msg, in_block) {
            BodyForm::Secret(path) => self.decode_secret_block(msg, &path),
            BodyForm::Any => self.decode_any_body(msg),
            BodyForm::Fields => self.decode_fields_inner(msg, in_block),
        };
        if in_block {
            self.leave();
        }
        result
    }

    /// Which reader takes a body: a Secret's block with the hook set, an
    /// Any's typed body, or the message's fields.
    #[cfg_attr(not(debug_assertions), inline(always))]
    fn body_form(&mut self, msg: &DynamicMessage, in_block: bool) -> BodyForm {
        if !in_block {
            return BodyForm::Fields;
        }
        let desc = msg.descriptor();
        // With the hook set, a Secret's block body routes its value through
        // it, as its shorthand does (protowire-go's decodeSecretBlockInto).
        // The caller extended the path by the Secret's own name.
        if !std::mem::take(&mut self.any_body) && self.on_secret.is_some() && is_secret(&desc) {
            let path = self
                .path_prefix
                .strip_suffix('.')
                .unwrap_or(&self.path_prefix)
                .to_string();
            return BodyForm::Secret(path);
        }
        // A block bound to google.protobuf.Any may open with a type entry
        // (draft -01 "Any Blocks"; protowire#354), wherever a body is
        // written: a singular field, a list element, a map value. Without
        // one, the block is the Any's own fields, type_url and value.
        if matches!(self.current.kind, TokenKind::AtType)
            && is_any_full_name(crate::wkt::dispatch_name(&desc))
        {
            return BodyForm::Any;
        }
        BodyForm::Fields
    }

    /// The fields of a message body. Each entry is read and checked by
    /// [`Self::next_entry`], out of line: the checks build error messages,
    /// and an unoptimized build gives every temporary its own stack slot,
    /// so here they would cost every level of a nested document (#179).
    /// Each helper split out of a recursive reader this way is inlined back
    /// into it in an optimized build, which shares slots between
    /// temporaries and where the call would cost time.
    fn decode_fields_inner(
        &mut self,
        msg: &mut DynamicMessage,
        in_block: bool,
    ) -> Result<(), PxfError> {
        let desc = msg.descriptor();
        let mut scope = BodyScope {
            set_oneofs: HashMap::new(),
            bound: HashSet::new(),
            // A keyed-element context applies to exactly this body: the
            // immediate entries of one element of a keyed repeated field.
            // Take it so nested submessages don't inherit the key checks.
            ke: self.keyed_elem.take(),
        };
        loop {
            let read = match self.next_entry(&desc, &mut scope, in_block) {
                Err(e) => Err(e),
                Ok(Step::End) => return Ok(()),
                Ok(Step::Read) => Ok(()),
                Ok(Step::Value(fd)) => self.decode_field_value(msg, &fd),
                Ok(Step::Keyed(fd, key_fd)) => self.decode_keyed_block_body(msg, &fd, &key_fd),
                Ok(Step::Block(fd, sub)) => self.decode_sub_body(msg, &fd, sub),
            };
            read?;
        }
    }

    /// Reads one entry of a body as far as its value, and says what reads
    /// the value. An entry it reads whole (a null, an unknown field
    /// skipped) is [`Step::Read`].
    #[cfg_attr(not(debug_assertions), inline(always))]
    fn next_entry(
        &mut self,
        desc: &MessageDescriptor,
        scope: &mut BodyScope,
        in_block: bool,
    ) -> Result<Step, PxfError> {
        if in_block && matches!(self.current.kind, TokenKind::RBrace) {
            self.advance();
            return Ok(Step::End);
        }
        if matches!(self.current.kind, TokenKind::Eof) {
            if in_block {
                return Err(self.err("expected '}', got EOF"));
            }
            return Ok(Step::End);
        }

        let pos = self.current.pos;
        let key_kind = self.current.kind;
        if matches!(key_kind, TokenKind::AtType) {
            return Err(self.err_at(pos, TYPE_ENTRY_MISPLACED));
        }
        // A quoted key that is not UTF-8 is a quoted key: protowire-go's
        // token holds its bytes, and the errors below quote them (#170).
        let key_bytes = self.bad_string.take();
        let key_kind = if key_bytes.is_some() {
            TokenKind::String
        } else {
            key_kind
        };
        // The grammar's key set (parseEntry), so a bool or integer key
        // before '=' or '{' gets the parser's error below, not this one.
        if !matches!(
            key_kind,
            TokenKind::Ident | TokenKind::String | TokenKind::Int | TokenKind::Bool
        ) {
            return Err(self.err_at(
                pos,
                format!(
                    "expected identifier, string, integer, or bool, got {}",
                    self.token_err_msg()
                ),
            ));
        }
        let key_quoted = matches!(key_kind, TokenKind::String);
        let key = match &key_bytes {
            Some(b) => String::from_utf8_lossy(b).into_owned(),
            None => std::mem::take(&mut self.current.value),
        };
        let quoted_name_error = |pos| match &key_bytes {
            Some(b) => quoted_name_unkeyed_error_bytes(pos, b),
            None => quoted_name_unkeyed_error(pos, &key),
        };
        // Only an integer key carries a suffix; the grammar errors below
        // write it as part of the key, as protowire-go's keyPrec does.
        let key_prec = std::mem::take(&mut self.current.prec);
        self.advance();

        match self.current.kind {
            TokenKind::Equals => {
                // No field has an integer or bool name, so this is the
                // parser's error whatever discard_unknown says.
                if matches!(key_kind, TokenKind::Int | TokenKind::Bool) {
                    return Err(non_name_assign_error(pos, key_kind, &key, &key_prec));
                }
                self.advance();
                if key_quoted {
                    // The grammar accepts a string at entry-name position
                    // everywhere; the schema layer restricts it to keyed
                    // repeated fields' blocks (draft -01 §3.13), which
                    // have their own loop — in message context a quoted
                    // name never names a field.
                    return Err(quoted_name_error(pos));
                }
                let fd = match desc.get_field_by_name(&key) {
                    Some(fd) => fd,
                    None => {
                        if self.discard_unknown {
                            self.skip_value()?;
                            return Ok(Step::Read);
                        }
                        return Err(self.err_at(
                            pos,
                            format!("unknown field {:?} in {}", key, desc.full_name()),
                        ));
                    }
                };
                if let Some(ke) = &scope.ke {
                    ke.check_binding(&fd, matches!(self.current.kind, TokenKind::Null), pos)?;
                }
                check_bound(&fd, &mut scope.bound, pos)?;
                self.check_oneof(&fd, &mut scope.set_oneofs, pos)?;
                if matches!(self.current.kind, TokenKind::Null) {
                    if !nullable(&fd) {
                        return Err(null_not_allowed(self.current.pos, &fd));
                    }
                    self.mark_null(&fd);
                    self.advance();
                    return Ok(Step::Read);
                }
                if let Some(ke) = &scope.ke {
                    if fd.name() == ke.key_fd.name() {
                        if let Some(b) = &self.bad_string {
                            ke.check_explicit_key_bytes(b, self.current.pos)?;
                        }
                    }
                    if fd.name() == ke.key_fd.name()
                        && matches!(self.current.kind, TokenKind::String)
                    {
                        // Explicit assignment to the element's key field:
                        // the empty string is never a valid key, and in
                        // the named form the value must agree with the
                        // entry name (draft -01 §3.13).
                        ke.check_explicit_key(&self.current.value, self.current.pos)?;
                    }
                }
                self.mark_present(&fd);
                Ok(Step::Value(fd))
            }
            TokenKind::LBrace => {
                if matches!(key_kind, TokenKind::Int | TokenKind::Bool) {
                    return Err(non_name_block_error(pos, key_kind, &key, &key_prec));
                }
                if key_quoted {
                    return Err(quoted_name_error(pos));
                }
                let fd = match desc.get_field_by_name(&key) {
                    Some(fd) => fd,
                    None => {
                        if self.discard_unknown {
                            // From the '{', so it reads as a block.
                            self.skip_value()?;
                            return Ok(Step::Read);
                        }
                        return Err(self.err_at(
                            pos,
                            format!("unknown field {:?} in {}", key, desc.full_name()),
                        ));
                    }
                };
                self.advance();
                if fd.is_list() {
                    // Keyed repeated field (draft -01 §3.13): the block
                    // form is a sequence of named entries, one per
                    // element.
                    if let Some(key_fd) = key_field(&fd) {
                        self.mark_present(&fd);
                        return Ok(Step::Keyed(fd, key_fd));
                    }
                    if matches!(self.current.kind, TokenKind::String) {
                        // The block spells the keyed form on a field
                        // with no (pxf.key); report the quoted entry
                        // name — the more specific schema violation —
                        // rather than the generic shape error.
                        return Err(quoted_name_unkeyed_error(
                            self.current.pos,
                            &self.current.value,
                        ));
                    }
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
                let Kind::Message(inner_desc) = fd.kind() else {
                    return Err(self.err_at(
                        pos,
                        format!(
                            "field {:?} is not a message type, cannot use block syntax",
                            key
                        ),
                    ));
                };
                if let Some(ke) = &scope.ke {
                    ke.check_binding(&fd, false, pos)?;
                }
                check_bound(&fd, &mut scope.bound, pos)?;
                self.check_oneof(&fd, &mut scope.set_oneofs, pos)?;
                self.mark_present(&fd);
                Ok(Step::Block(fd, DynamicMessage::new(inner_desc)))
            }
            TokenKind::Colon => {
                if !in_block {
                    // The parser's grammar error: no map entry stands
                    // at the top level of a document, whatever the
                    // schema.
                    return Err(top_level_map_entry_error(pos));
                }
                Err(self.err_at(
                    pos,
                    "unexpected ':' in message context, use '=' for field assignments",
                ))
            }
            _ => Err(self.err(format!(
                "expected '=', ':', or '{{' after {}, got {}",
                match &key_bytes {
                    Some(b) => quote_bytes(b),
                    None => format!("{key:?}"),
                },
                self.current.kind
            ))),
        }
    }

    /// Reads a message body into `sub`, its `{` consumed, under `fd`'s path,
    /// and sets it on `msg`.
    #[cfg_attr(not(debug_assertions), inline(always))]
    fn decode_sub_body(
        &mut self,
        msg: &mut DynamicMessage,
        fd: &FieldDescriptor,
        mut sub: DynamicMessage,
    ) -> Result<(), PxfError> {
        let saved = self.enter_field(fd.name());
        self.decode_fields(&mut sub, true)?;
        self.path_prefix = saved;
        msg.set_field(fd, Value::Message(sub));
        Ok(())
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
        self.illegal()?;
        if fd.is_map() {
            return self.decode_map_inline(msg, fd);
        }
        if fd.is_list() {
            // Keyed repeated field written `name = { ... }`: a block-tail is
            // an abbreviation of `= { ... }` (draft -01 §3.13), so the
            // assignment spelling of the keyed block form is equally valid.
            if matches!(self.current.kind, TokenKind::LBrace) {
                if let Some(key_fd) = key_field(fd) {
                    self.advance();
                    return self.decode_keyed_block_body(msg, fd, &key_fd);
                }
            }
            return self.decode_list_inline(msg, fd);
        }
        if let Kind::Message(inner_desc) = fd.kind() {
            return match self.message_value(msg, fd, DynamicMessage::new(inner_desc))? {
                Some(sub) => self.decode_sub_body(msg, fd, sub),
                None => Ok(()),
            };
        }
        self.decode_scalar_value(msg, fd)
    }

    /// A singular message field's value as far as its body: a well-known
    /// type's literal, read whole and set on `msg` (`None`), or a block,
    /// returned past its `{`.
    #[cfg_attr(not(debug_assertions), inline(always))]
    fn message_value(
        &mut self,
        msg: &mut DynamicMessage,
        fd: &FieldDescriptor,
        mut sub: DynamicMessage,
    ) -> Result<Option<DynamicMessage>, PxfError> {
        let at = self
            .on_secret
            .map(|_| format!("{}{}", self.path_prefix, fd.name()));
        if let Some(inner) = self.try_decode_wkt(&mut sub, fd, at)? {
            msg.set_field(fd, Value::Message(sub));
            if let Some(p) = self.presence.as_mut() {
                for name in inner {
                    p.mark_present(format!("{}{}.{name}", self.path_prefix, fd.name()));
                }
            }
            return Ok(None);
        }
        if !matches!(self.current.kind, TokenKind::LBrace) {
            return Err(self.err(format!(
                "expected {} for message field {:?}, got {}",
                msg_value_forms(&sub.descriptor()),
                fd.name(),
                self.token_err_msg()
            )));
        }
        self.advance();
        Ok(Some(sub))
    }

    /// An enum or scalar field's value, set on `msg`.
    #[cfg_attr(not(debug_assertions), inline(always))]
    fn decode_scalar_value(
        &mut self,
        msg: &mut DynamicMessage,
        fd: &FieldDescriptor,
    ) -> Result<(), PxfError> {
        let v = if matches!(fd.kind(), Kind::Enum(_)) {
            self.consume_enum(fd)?
        } else {
            self.consume_scalar(fd)?
        };
        msg.set_field(fd, v);
        Ok(())
    }

    fn decode_list_inline(
        &mut self,
        msg: &mut DynamicMessage,
        fd: &FieldDescriptor,
    ) -> Result<(), PxfError> {
        // A repeated field bound more than once concatenates in document
        // order (draft -01 § Entries and Keys), so start from what is there.
        let mut elems = self.list_open(msg, fd)?;
        // Non-None: the anonymous form of a keyed repeated field, whose
        // element bodies get the key-field checks (draft -01 §3.13).
        let key_fd = key_field(fd);

        while !matches!(self.current.kind, TokenKind::RBracket | TokenKind::Eof) {
            let read = match self.list_element(fd, elems.len(), key_fd.as_ref()) {
                Err(e) => Err(e),
                Ok(Elem::Value(v)) => Ok(v),
                Ok(Elem::Body(sub)) => {
                    let saved = self.enter_index(fd.name(), elems.len());
                    self.decode_element_body(sub, saved)
                }
            };
            self.push_element(&mut elems, read)?;
        }
        self.close_list(msg, fd, elems)
    }

    /// A list's `[`, consumed and counted, and the elements `msg` already
    /// holds.
    #[cfg_attr(not(debug_assertions), inline(always))]
    fn list_open(
        &mut self,
        msg: &mut DynamicMessage,
        fd: &FieldDescriptor,
    ) -> Result<Vec<Value>, PxfError> {
        if !matches!(self.current.kind, TokenKind::LBracket) {
            return Err(self.err(format!("expected '[' for repeated field {:?}", fd.name())));
        }
        self.enter()?;
        self.advance();
        Ok(existing_list(msg, fd))
    }

    /// Adds a list element, and its comma.
    #[cfg_attr(not(debug_assertions), inline(always))]
    fn push_element(
        &mut self,
        elems: &mut Vec<Value>,
        read: Result<Value, PxfError>,
    ) -> Result<(), PxfError> {
        elems.push(read?);
        if matches!(self.current.kind, TokenKind::Comma) {
            self.advance();
        }
        Ok(())
    }

    /// A list's `]`, and the list set on `msg`.
    #[cfg_attr(not(debug_assertions), inline(always))]
    fn close_list(
        &mut self,
        msg: &mut DynamicMessage,
        fd: &FieldDescriptor,
        elems: Vec<Value>,
    ) -> Result<(), PxfError> {
        self.close(TokenKind::RBracket)?;
        msg.set_field(fd, Value::List(elems));
        Ok(())
    }

    /// An element's or map value's body into `sub`, its `{` consumed, its
    /// path entered; `saved` is the path to restore.
    #[cfg_attr(not(debug_assertions), inline(always))]
    fn decode_element_body(
        &mut self,
        mut sub: DynamicMessage,
        saved: String,
    ) -> Result<Value, PxfError> {
        self.decode_fields(&mut sub, true)?;
        self.path_prefix = saved;
        Ok(Value::Message(sub))
    }

    /// The `n`th element of list `fd` as far as its body: a value read
    /// whole, or a message element's block, past its `{`.
    #[cfg_attr(not(debug_assertions), inline(always))]
    fn list_element(
        &mut self,
        fd: &FieldDescriptor,
        n: usize,
        key_fd: Option<&FieldDescriptor>,
    ) -> Result<Elem, PxfError> {
        self.illegal()?;
        if n >= self.limits.max_repeated_count {
            return Err(self.err(format!(
                "repeated field {:?} exceeds MaxRepeatedCount={}",
                fd.name(),
                self.limits.max_repeated_count
            )));
        }
        if matches!(self.current.kind, TokenKind::Null) {
            return Err(self.err(format!(
                "null is not allowed in repeated field {:?}",
                fd.name()
            )));
        }
        Ok(Elem::Value(match fd.kind() {
            Kind::Message(inner_desc) => {
                let mut sub = DynamicMessage::new(inner_desc);
                let at = self
                    .on_secret
                    .map(|_| format!("{}{}[{}]", self.path_prefix, fd.name(), n));
                if self.try_decode_wkt(&mut sub, fd, at)?.is_none() {
                    if !matches!(self.current.kind, TokenKind::LBrace) {
                        return Err(self.err(format!(
                            "expected {} for repeated message element, got {}",
                            msg_value_forms(&sub.descriptor()),
                            self.token_err_msg()
                        )));
                    }
                    self.advance();
                    if let Some(key_fd) = key_fd {
                        self.keyed_elem = Some(KeyedElemState {
                            field: fd.name().to_string(),
                            key_fd: key_fd.clone(),
                            entry_name: String::new(),
                            named: false,
                        });
                    }
                    return Ok(Elem::Body(sub));
                }
                Value::Message(sub)
            }
            Kind::Enum(_) => self.consume_enum(fd)?,
            _ => self.consume_scalar(fd)?,
        }))
    }

    /// The `close` token (`]` or `}`) of a list or map, and its depth.
    #[cfg_attr(not(debug_assertions), inline(always))]
    fn close(&mut self, close: TokenKind) -> Result<(), PxfError> {
        if self.current.kind != close {
            return Err(self.err(if close == TokenKind::RBracket {
                format!("expected ']', got {}", self.current.kind)
            } else {
                format!("expected '}}', got {}", self.current.kind)
            }));
        }
        self.advance();
        self.leave();
        Ok(())
    }

    /// Decode the block form of a keyed repeated field (draft -01 §3.13):
    /// a sequence of named entries — `name { ... }` or equivalently
    /// `name = { ... }` — where each entry name (unquoted value, for
    /// string-literal names) populates the element's key field and entry
    /// order is list order. Duplicate entry names within the block, the
    /// empty string as a name, and a disagreeing explicit key-field
    /// assignment inside an entry are decode errors. The opening `{` has
    /// been consumed; the closing `}` is consumed before returning.
    fn decode_keyed_block_body(
        &mut self,
        msg: &mut DynamicMessage,
        fd: &FieldDescriptor,
        key_fd: &FieldDescriptor,
    ) -> Result<(), PxfError> {
        self.enter()?;
        let mut elems = existing_list(msg, fd);
        let mut seen: HashSet<String> = HashSet::new();
        while let Some(sub) = self.keyed_entry(fd, key_fd, elems.len(), &mut seen)? {
            let saved = self.enter_index(fd.name(), elems.len());
            // The entry name supplies the key field, so the document counts
            // as having set it (protowire-go#214).
            self.mark_present(key_fd);
            let read = self.decode_element_body(sub, saved);
            elems.push(read?);
        }
        self.leave();
        msg.set_field(fd, Value::List(elems));
        Ok(())
    }

    /// The next entry of keyed field `fd`'s block as far as its body, past
    /// its `{`, holding its key; `None` past the block's `}`.
    #[cfg_attr(not(debug_assertions), inline(always))]
    fn keyed_entry(
        &mut self,
        fd: &FieldDescriptor,
        key_fd: &FieldDescriptor,
        n: usize,
        seen: &mut HashSet<String>,
    ) -> Result<Option<DynamicMessage>, PxfError> {
        match self.current.kind {
            TokenKind::RBrace => {
                self.advance();
                return Ok(None);
            }
            TokenKind::Eof => {
                return Err(self.err(format!(
                    "expected '}}' to close keyed field {:?}, got EOF",
                    fd.name()
                )));
            }
            TokenKind::Ident | TokenKind::String => {}
            _ if self.bad_string.is_some() => {
                return Err(self.err(format!(
                    "invalid UTF-8 in entry name for keyed field {:?}",
                    fd.name()
                )));
            }
            _ => {
                return Err(self.err(format!(
                    "expected entry name (identifier or string) in keyed field {:?}, got {}",
                    fd.name(),
                    self.token_err_msg()
                )));
            }
        }
        let name_pos = self.current.pos;
        let name = std::mem::take(&mut self.current.value);
        if name.is_empty() {
            return Err(empty_name_error(name_pos, fd.name()));
        }
        if !seen.insert(name.clone()) {
            return Err(duplicate_key_error(name_pos, fd.name(), &name));
        }
        self.advance();
        match self.current.kind {
            TokenKind::LBrace => self.advance(),
            TokenKind::Equals => {
                self.advance();
                if !matches!(self.current.kind, TokenKind::LBrace) {
                    return Err(self.err(format!(
                        "keyed entry {:?} of field {:?} must have a block value ('{{ ... }}'): the element type is a message",
                        name,
                        fd.name()
                    )));
                }
                self.advance();
            }
            other => {
                return Err(self.err(format!(
                    "expected '{{' or '=' after entry name {:?} in keyed field {:?}, got {}",
                    name,
                    fd.name(),
                    other
                )));
            }
        }
        if n >= self.limits.max_repeated_count {
            return Err(self.err(format!(
                "repeated field {:?} exceeds MaxRepeatedCount={}",
                fd.name(),
                self.limits.max_repeated_count
            )));
        }
        let Kind::Message(elem_desc) = fd.kind() else {
            unreachable!("key_field only matches message-typed lists")
        };
        let mut sub = DynamicMessage::new(elem_desc);
        sub.set_field(key_fd, Value::String(name.clone()));
        self.keyed_elem = Some(KeyedElemState {
            field: fd.name().to_string(),
            key_fd: key_fd.clone(),
            entry_name: name,
            named: true,
        });
        Ok(Some(sub))
    }

    fn decode_map_inline(
        &mut self,
        msg: &mut DynamicMessage,
        fd: &FieldDescriptor,
    ) -> Result<(), PxfError> {
        let (key_fd, val_fd, mut map) = self.map_open(msg, fd)?;
        while !matches!(self.current.kind, TokenKind::RBrace | TokenKind::Eof) {
            let read = match self.map_entry(fd, &key_fd, &val_fd, &map) {
                Err(e) => Err(e),
                Ok((key, Elem::Value(v))) => Ok((key, v)),
                Ok((key, Elem::Body(sub))) => {
                    let saved = self.enter_key(fd.name(), &key);
                    self.decode_element_body(sub, saved).map(|v| (key, v))
                }
            };
            let (key, value) = read?;
            map.insert(key, value);
        }
        self.close_map(msg, fd, map)
    }

    /// A map's `{`, consumed and counted: its entry's key and value fields,
    /// and the entries `msg` already holds.
    #[cfg_attr(not(debug_assertions), inline(always))]
    fn map_open(
        &mut self,
        msg: &mut DynamicMessage,
        fd: &FieldDescriptor,
    ) -> Result<(FieldDescriptor, FieldDescriptor, HashMap<MapKey, Value>), PxfError> {
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
        // A map bound more than once merges, so start from what is there; a
        // key bound twice in one body is an error either way (draft -01
        // § Entries and Keys; protowire#396).
        let map = match msg.take_field(fd) {
            Some(Value::Map(map)) => map,
            _ => HashMap::new(),
        };
        Ok((
            map_entry_desc.map_entry_key_field(),
            map_entry_desc.map_entry_value_field(),
            map,
        ))
    }

    /// A map's `}`, and the map set on `msg`.
    #[cfg_attr(not(debug_assertions), inline(always))]
    fn close_map(
        &mut self,
        msg: &mut DynamicMessage,
        fd: &FieldDescriptor,
        map: HashMap<MapKey, Value>,
    ) -> Result<(), PxfError> {
        self.close(TokenKind::RBrace)?;
        msg.set_field(fd, Value::Map(map));
        Ok(())
    }

    /// The next entry of map `fd`, its key and its value as far as its
    /// body: a value read whole, or a message value's block, past its `{`.
    #[cfg_attr(not(debug_assertions), inline(always))]
    fn map_entry(
        &mut self,
        fd: &FieldDescriptor,
        key_fd: &FieldDescriptor,
        val_fd: &FieldDescriptor,
        map: &HashMap<MapKey, Value>,
    ) -> Result<(MapKey, Elem), PxfError> {
        let pos = self.current.pos;
        // A quoted key that is not UTF-8 is read as a quoted key (#170).
        let key_bytes = self.bad_string.take();
        let tk = if key_bytes.is_some() {
            TokenKind::String
        } else {
            self.current.kind
        };
        if map.len() >= self.limits.max_repeated_count {
            return Err(self.err_at(
                pos,
                format!(
                    "map field {:?} exceeds MaxRepeatedCount={}",
                    fd.name(),
                    self.limits.max_repeated_count
                ),
            ));
        }
        if !matches!(
            tk,
            TokenKind::Ident | TokenKind::String | TokenKind::Int | TokenKind::Bool
        ) {
            return Err(self.err_at(
                pos,
                format!("expected map key, got {}", self.token_err_msg()),
            ));
        }
        if !self.current.prec.is_empty() {
            return Err(self.err_at(
                pos,
                format!(
                    "map field {:?}: a key cannot carry a precision suffix (p{})",
                    fd.name(),
                    clip_literal(&self.current.prec)
                ),
            ));
        }
        let key_str = match &key_bytes {
            Some(b) => String::from_utf8_lossy(b).into_owned(),
            None => std::mem::take(&mut self.current.value),
        };
        let key_kind = tk;
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

        if key_bytes.is_some() && matches!(key_fd.kind(), Kind::String) {
            return Err(self.err_at(pos, "invalid UTF-8 in string map key"));
        }
        // Another key type reads the key's text, which protowire-go quotes
        // raw in its error, and which a Rust error holds as U+FFFD.
        let key = decode_map_key(fd, key_fd, key_str, key_kind, pos)?;
        // Compared as values of the key type, so 1 and "1" are one key
        // of an integer-keyed map.
        if map.contains_key(&key) {
            return Err(self.err_at(
                pos,
                format!(
                    "map field {:?}: key {} is already set",
                    fd.name(),
                    shown_key(&key)
                ),
            ));
        }

        self.illegal()?;
        if matches!(self.current.kind, TokenKind::Null) {
            return Err(self.err(format!(
                "null is not allowed as map value in field {:?}",
                fd.name()
            )));
        }

        let value = if let Kind::Message(inner_desc) = val_fd.kind() {
            let mut sub = DynamicMessage::new(inner_desc);
            let at = self
                .on_secret
                .map(|_| format!("{}{}[{}]", self.path_prefix, fd.name(), shown_key(&key)));
            if self.try_decode_wkt(&mut sub, fd, at)?.is_none() {
                if !matches!(self.current.kind, TokenKind::LBrace) {
                    return Err(self.err(format!(
                        "expected {} for map message value, got {}",
                        msg_value_forms(&sub.descriptor()),
                        self.token_err_msg()
                    )));
                }
                self.advance();
                return Ok((key, Elem::Body(sub)));
            }
            Value::Message(sub)
        } else if matches!(val_fd.kind(), Kind::Enum(_)) {
            self.consume_enum_as(val_fd, fd)?
        } else {
            self.consume_scalar_as(val_fd, fd)?
        };
        Ok((key, Elem::Value(value)))
    }

    /// Decode a `google.protobuf.Any` block that opens with a type entry,
    /// `@type = "<type-url>"`, its `{` consumed: the rest of the block is a
    /// body of the type the URL names, packed into `value`, and the URL is
    /// stored verbatim in `type_url`.
    fn decode_any_body(&mut self, target: &mut DynamicMessage) -> Result<(), PxfError> {
        let (type_url, url_pos, mut inner) = self.any_type_entry()?;
        // The body shares the Any's braces, which the enclosing
        // decode_fields has already counted as one descent.
        self.depth -= 1;
        self.any_body = true;
        let decoded = self.decode_fields(&mut inner, true);
        self.depth += 1;
        decoded?;
        self.pack_any(target, inner, type_url, url_pos)
    }

    /// An Any block's `@type = "<type-url>"`: the URL, where it stands, and
    /// an empty message of the type it names.
    #[cfg_attr(not(debug_assertions), inline(always))]
    fn any_type_entry(&mut self) -> Result<(String, Position, DynamicMessage), PxfError> {
        self.advance(); // consume @type
        if !matches!(self.current.kind, TokenKind::Equals) {
            return Err(self.err(format!(
                "expected '=' after @type, got {}",
                self.token_err_msg()
            )));
        }
        self.advance();
        if let Some(b) = self.bad_string.take() {
            // No type is named by bytes that are not UTF-8.
            let url = quote_bytes(&b);
            return Err(self.err(match self.type_resolver {
                None => format!("cannot resolve Any type {url}: no type_resolver"),
                Some(_) => format!("cannot resolve Any type {url}"),
            }));
        }
        if !matches!(self.current.kind, TokenKind::String) {
            return Err(self.err(format!(
                "expected a string type URL after @type =, got {}",
                self.token_err_msg()
            )));
        }
        let type_url = std::mem::take(&mut self.current.value);
        let url_pos = self.current.pos;
        self.advance();

        let resolver = self.type_resolver.ok_or_else(|| {
            PxfError::new(
                url_pos,
                format!("cannot resolve Any type {type_url:?}: no type_resolver"),
            )
        })?;
        let inner_desc = resolver.find_message_by_url(&type_url).ok_or_else(|| {
            PxfError::new(url_pos, format!("cannot resolve Any type {:?}", type_url))
        })?;
        Ok((type_url, url_pos, DynamicMessage::new(inner_desc)))
    }

    /// Packs an Any's decoded body into `target`, with its type URL.
    #[cfg_attr(not(debug_assertions), inline(always))]
    fn pack_any(
        &mut self,
        target: &mut DynamicMessage,
        mut inner: DynamicMessage,
        type_url: String,
        url_pos: Position,
    ) -> Result<(), PxfError> {
        // Packed to bytes below, the body is out of reach of the pass over
        // the root, so it takes that pass here, under the Any's own path.
        if let Some(presence) = self.presence.as_ref().filter(|_| self.post_decode_any) {
            post_decode(&mut inner, presence, None, &self.path_prefix)?;
        }
        // In key order, so the bytes don't depend on the map's hash order.
        let mut packed = Vec::new();
        crate::encode::encode_canonical(&inner, &mut packed);

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
    /// Returns the inner fields the literal set if a WKT shortcut matched and
    /// was consumed, `None` to fall through to a regular `{ ... }` block
    /// decode. A singular field marks those inner fields present, as
    /// protowire-go's `markInnerPresent` does; a list element or map value
    /// does not.
    fn try_decode_wkt(
        &mut self,
        target: &mut DynamicMessage,
        named: &FieldDescriptor,
        secret_path: Option<String>,
    ) -> Result<Option<&'static [&'static str]>, PxfError> {
        let desc = target.descriptor();
        // A message with a well-known type's name and not its shape has no
        // literal form (draft -01 §wkt-shapes): its name matches none here.
        let full = crate::wkt::dispatch_name(&desc).to_string();

        if full == "google.protobuf.Timestamp" && matches!(self.current.kind, TokenKind::Timestamp)
        {
            let pos = self.current.pos;
            let (seconds, nanos) = crate::rfc3339::parse(&self.current.value)
                .ok_or("not an RFC 3339 timestamp")
                .map_err(|e| {
                    PxfError::new(
                        pos,
                        format!("invalid timestamp {:?}: {}", self.current.value, e),
                    )
                })?;
            set_seconds_nanos(target, seconds, nanos);
            self.advance();
            return Ok(Some(&["seconds", "nanos"]));
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
            return Ok(Some(&["seconds", "nanos"]));
        }
        if is_wrapper_full_name(&full) && !matches!(self.current.kind, TokenKind::LBrace) {
            let value_fd = desc.get_field_by_name("value").ok_or_else(|| {
                self.err(format!("internal: wrapper {} missing 'value' field", full))
            })?;
            let v = self.consume_scalar_as(&value_fd, named)?;
            target.set_field(&value_fd, v);
            return Ok(Some(&["value"]));
        }
        // Arbitrary-precision literal forms (protowire-rust#34): a bare
        // integer on a pxf.BigInt field, an integer or decimal literal on a
        // pxf.Decimal field. Any other token — a `{` in particular — falls
        // through to the block form, as for every message type. The digit
        // cap runs before the quadratic conversion.
        if full == "pxf.BigInt" && matches!(self.current.kind, TokenKind::Int) {
            self.no_precision(&full)?;
            let pos = self.current.pos;
            let lit = parse_big_int(&self.current.value, self.limits.max_numeric_literal_digits)
                .map_err(|e| PxfError::new(pos, e))?;
            set_big_int_fields(target, &lit);
            self.advance();
            return Ok(Some(&["abs", "negative"]));
        }
        if full == "pxf.Decimal" && matches!(self.current.kind, TokenKind::Int | TokenKind::Float) {
            self.no_precision(&full)?;
            let pos = self.current.pos;
            let lit = parse_decimal(&self.current.value, self.limits.max_numeric_literal_digits)
                .map_err(|e| PxfError::new(pos, e))?;
            set_decimal_fields(target, &lit);
            self.advance();
            return Ok(Some(&["unscaled", "scale", "negative"]));
        }
        // A pxf.Secret written as a string literal sets its value alone
        // (draft -01 § pxf.Secret): `pw = "x"`, in a field, a list element
        // or a map value. The block form falls through.
        if full == "pxf.Secret"
            && (matches!(self.current.kind, TokenKind::String) || self.bad_string.is_some())
        {
            if self.on_secret.is_some() {
                if self.bad_string.is_some() {
                    return Err(self.err(format!(
                        "invalid UTF-8 in pxf.Secret value for field {:?}",
                        named.name()
                    )));
                }
                // The hook takes the value; the Secret stays empty.
                let path = secret_path.expect("a path whenever the hook is set");
                self.call_secret_hook(&path, &self.current.value, self.current.pos)?;
                self.advance();
                return Ok(Some(&["value"]));
            }
            let value_fd = desc
                .get_field_by_name("value")
                .expect("checked by dispatch_name");
            let v = self.consume_scalar_as(&value_fd, named)?;
            target.set_field(&value_fd, v);
            return Ok(Some(&["value"]));
        }
        if full == "pxf.BigFloat" && matches!(self.current.kind, TokenKind::Int | TokenKind::Float)
        {
            let pos = self.current.pos;
            let lit = parse_big_float(
                &self.current.value,
                &self.current.prec,
                self.limits.max_numeric_literal_digits,
                self.limits.max_big_float_exponent,
            )
            .map_err(|e| PxfError::new(pos, e))?;
            set_big_float_fields(target, &lit);
            self.advance();
            return Ok(Some(&["mantissa", "exponent", "prec", "negative"]));
        }
        Ok(None)
    }

    /// The body of a `pxf.Secret` block, its `{` consumed and its depth
    /// counted by `decode_fields`, with the hook set: `value` goes to the
    /// hook as the field at `path`, `hint` and `fingerprint` into `sub`.
    /// Each may appear once and in any order; any other name is an error,
    /// since a Secret is a closed shape. Consumes the closing `}`.
    /// protowire-go's `decodeSecretBlockInto`.
    fn decode_secret_block(
        &mut self,
        sub: &mut DynamicMessage,
        path: &str,
    ) -> Result<(), PxfError> {
        let desc = sub.descriptor();
        let value_fd = desc
            .get_field_by_name("value")
            .expect("checked by is_secret");
        let mut bound: HashSet<u32> = HashSet::new();
        loop {
            match self.current.kind {
                TokenKind::RBrace => {
                    self.advance();
                    return Ok(());
                }
                TokenKind::Eof => {
                    return Err(self.err("expected '}' to close pxf.Secret block, got EOF"))
                }
                TokenKind::Ident => {}
                _ => {
                    return Err(self.err(format!(
                        "expected pxf.Secret field name (value/hint/fingerprint), got {}",
                        self.token_err_msg()
                    )))
                }
            }
            let name = std::mem::take(&mut self.current.value);
            let name_pos = self.current.pos;
            self.advance();
            if !matches!(self.current.kind, TokenKind::Equals) {
                return Err(self.err(format!(
                    "expected '=' after pxf.Secret field {}, got {}",
                    crate::go_strconv::quote(&name),
                    self.current.kind
                )));
            }
            self.advance();
            if self.bad_string.is_some() {
                return Err(self.err(format!("invalid UTF-8 in pxf.Secret {name}")));
            }
            self.illegal()?;
            if !matches!(self.current.kind, TokenKind::String) {
                return Err(self.err(format!(
                    "expected string value for pxf.Secret {name}, got {}",
                    self.current.kind
                )));
            }
            match name.as_str() {
                "value" => {
                    check_bound(&value_fd, &mut bound, name_pos)?;
                    self.call_secret_hook(path, &self.current.value, self.current.pos)?;
                }
                "hint" | "fingerprint" => {
                    let Some(fd) = desc.get_field_by_name(&name) else {
                        return Err(PxfError::new(
                            name_pos,
                            format!(
                                "unknown pxf.Secret field {}: this schema does not declare it",
                                crate::go_strconv::quote(&name)
                            ),
                        ));
                    };
                    check_bound(&fd, &mut bound, name_pos)?;
                    sub.set_field(&fd, Value::String(self.current.value.clone()));
                }
                _ => {
                    return Err(PxfError::new(
                        name_pos,
                        format!(
                            "unknown pxf.Secret field {} (expected value/hint/fingerprint)",
                            crate::go_strconv::quote(&name)
                        ),
                    ))
                }
            }
            if let Some(p) = self.presence.as_mut() {
                p.mark_present(format!("{path}.{name}"));
            }
            self.advance();
        }
    }

    /// Reject a precision suffix on the current literal, which binds to
    /// `field`, not a `pxf.BigFloat` (draft -01 § PXF Annotations;
    /// protowire#406). The suffix lexes on any number; only a BigFloat reads
    /// it.
    fn no_precision(&self, field: &str) -> Result<(), PxfError> {
        if self.current.prec.is_empty() {
            return Ok(());
        }
        Err(self.err(format!(
            "precision suffix p{} is allowed only on a pxf.BigFloat, not on field {:?}",
            self.current.prec, field
        )))
    }

    fn consume_scalar(&mut self, fd: &FieldDescriptor) -> Result<Value, PxfError> {
        self.consume_scalar_as(fd, fd)
    }

    /// One scalar typed by `fd`, with every error naming `named`, the field
    /// the document wrote. They differ where the typing descriptor is one
    /// no document names: a well-known wrapper's or a `pxf.Secret`'s scalar
    /// shorthand, typed by the message's inner `value`, and a map's value,
    /// typed by the map entry's. The block form `n { value = "x" }` does
    /// write `value`, and is named so. protowire-go's `consumeScalarAs`
    /// (protowire-go#85).
    fn consume_scalar_as(
        &mut self,
        fd: &FieldDescriptor,
        named: &FieldDescriptor,
    ) -> Result<Value, PxfError> {
        self.illegal()?;
        self.no_precision(named.name())?;
        let pos = self.current.pos;
        let kind = fd.kind();
        match kind {
            Kind::String => {
                if self.bad_string.is_some() {
                    // HARDENING.md § UTF-8: a string field holds UTF-8; a
                    // `\xHH` escape can write any byte, and bytes go in b"…".
                    return Err(self.err_at(
                        pos,
                        format!(
                            "invalid UTF-8 in string field {:?} (use b\"…\" for raw bytes)",
                            named.name()
                        ),
                    ));
                }
                if !matches!(self.current.kind, TokenKind::String) {
                    return Err(
                        self.err_at(pos, format!("expected string for field {:?}", named.name()))
                    );
                }
                let v = Value::String(std::mem::take(&mut self.current.value));
                self.advance();
                Ok(v)
            }
            Kind::Bool => {
                if !matches!(self.current.kind, TokenKind::Bool) {
                    return Err(
                        self.err_at(pos, format!("expected bool for field {:?}", named.name()))
                    );
                }
                let v = Value::Bool(self.current.value == "true");
                self.advance();
                Ok(v)
            }
            Kind::Int32 | Kind::Sint32 | Kind::Sfixed32 => {
                if !matches!(self.current.kind, TokenKind::Int) {
                    return Err(self.err_at(
                        pos,
                        format!("expected integer for field {:?}", named.name()),
                    ));
                }
                let n: i32 = self.current.value.parse().map_err(|_| {
                    self.err_at(pos, format!("invalid int32: {}", self.current.value))
                })?;
                self.advance();
                Ok(Value::I32(n))
            }
            Kind::Int64 | Kind::Sint64 | Kind::Sfixed64 => {
                if !matches!(self.current.kind, TokenKind::Int) {
                    return Err(self.err_at(
                        pos,
                        format!("expected integer for field {:?}", named.name()),
                    ));
                }
                let n: i64 = self.current.value.parse().map_err(|_| {
                    self.err_at(pos, format!("invalid int64: {}", self.current.value))
                })?;
                self.advance();
                Ok(Value::I64(n))
            }
            Kind::Uint32 | Kind::Fixed32 => {
                if !matches!(self.current.kind, TokenKind::Int) {
                    return Err(self.err_at(
                        pos,
                        format!("expected integer for field {:?}", named.name()),
                    ));
                }
                let n: u32 = self.current.value.parse().map_err(|_| {
                    self.err_at(pos, format!("invalid uint32: {}", self.current.value))
                })?;
                self.advance();
                Ok(Value::U32(n))
            }
            Kind::Uint64 | Kind::Fixed64 => {
                if !matches!(self.current.kind, TokenKind::Int) {
                    return Err(self.err_at(
                        pos,
                        format!("expected integer for field {:?}", named.name()),
                    ));
                }
                let n: u64 = self.current.value.parse().map_err(|_| {
                    self.err_at(pos, format!("invalid uint64: {}", self.current.value))
                })?;
                self.advance();
                Ok(Value::U64(n))
            }
            Kind::Float => {
                if !self.at_float_literal() {
                    return Err(
                        self.err_at(pos, format!("expected number for field {:?}", named.name()))
                    );
                }
                let f = parse_f32_literal(&self.current.value).ok_or_else(|| {
                    self.err_at(pos, format!("invalid float: {}", self.current.value))
                })?;
                self.advance();
                Ok(Value::F32(f))
            }
            Kind::Double => {
                if !self.at_float_literal() {
                    return Err(
                        self.err_at(pos, format!("expected number for field {:?}", named.name()))
                    );
                }
                let f = parse_f64_literal(&self.current.value).ok_or_else(|| {
                    self.err_at(pos, format!("invalid double: {}", self.current.value))
                })?;
                self.advance();
                Ok(Value::F64(f))
            }
            Kind::Bytes => {
                if !matches!(self.current.kind, TokenKind::Bytes) {
                    return Err(
                        self.err_at(pos, format!("expected bytes for field {:?}", named.name()))
                    );
                }
                let decoded = crate::go_strconv::decode_base64_lenient(&self.current.value)
                    .map_err(|e| {
                        self.err_at(
                            pos,
                            format!("invalid base64 for field {:?}: {e}", named.name()),
                        )
                    })?;
                self.advance();
                Ok(Value::Bytes(decoded.into()))
            }
            Kind::Enum(_) => self.consume_enum_as(fd, named),
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
        self.consume_enum_as(fd, fd)
    }

    /// [`Self::consume_scalar_as`] for an enum.
    fn consume_enum_as(
        &mut self,
        fd: &FieldDescriptor,
        named: &FieldDescriptor,
    ) -> Result<Value, PxfError> {
        self.illegal()?;
        self.no_precision(named.name())?;
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
                format!("expected enum name or number for field {:?}", named.name()),
            )),
        }
    }

    /// Consume the value of a field `discard_unknown` drops. It is read
    /// with the parser, not by counting braces: dropping a field does not
    /// exempt it from the grammar, and the same rule covers its depth.
    ///
    /// Its strings are never read, so one that is not UTF-8 is taken, as
    /// protowire-go takes it (#170).
    fn skip_value(&mut self) -> Result<(), PxfError> {
        self.read_dropped_value(self.depth)
    }

    /// Reads a value with the parser and drops it: a value `discard_unknown`
    /// drops, or a directive's body, which is kept as its bytes. A string
    /// in it that is not UTF-8 is taken, since its text is never read.
    fn read_dropped_value(&mut self, depth: usize) -> Result<(), PxfError> {
        if self.bad_string.take().is_some() {
            self.current.kind = TokenKind::String;
        }
        let read = read_dropped_value(&mut self.lex, &mut self.current, depth, self.limits);
        self.after_parser();
        read.map(drop)
    }
}

/// Whether `null` may bind to `fd`: a singular field with explicit
/// presence, which it leaves unset and present-but-null (draft -01
/// § Booleans, Null, and Identifier Values; protowire#339, #379). That is a
/// message field, wrappers included, a proto3 `optional` scalar, a oneof
/// member or a proto2 `optional` field. A required field would be invalid
/// in binary unset, and a scalar without presence has no unset state, so
/// `null` would read back as its zero value.
/// The canonical quiet NaNs, the one encoding a decoder may store for the
/// literal `nan` (draft -01 § Scalar Values, protowire-java#161).
const CANONICAL_NAN_64: u64 = 0x7FF8_0000_0000_0000;
const CANONICAL_NAN_32: u32 = 0x7FC0_0000;

/// Whether `s` spells an infinity outright, as `inf`, `+inf` or `-inf`
/// (and the spellings Go's `strconv.ParseFloat` also takes, which a
/// `(pxf.default)` string may use), rather than rounding to one.
fn spells_infinity(s: &str) -> bool {
    let unsigned = s.strip_prefix(['+', '-']).unwrap_or(s);
    unsigned.eq_ignore_ascii_case("inf") || unsigned.eq_ignore_ascii_case("infinity")
}

/// A double literal (draft -01 § Scalar Values): a number in IEEE 754 range,
/// or one of the identifiers `inf`, `+inf`, `-inf`, `nan`. A number that
/// rounds to an infinity is `None`, as is anything else that is not a
/// float; `nan` is stored as the canonical quiet NaN.
fn parse_f64_literal(s: &str) -> Option<f64> {
    let f: f64 = s.parse().ok()?;
    if f.is_infinite() && !spells_infinity(s) {
        return None;
    }
    Some(if f.is_nan() {
        f64::from_bits(CANONICAL_NAN_64)
    } else {
        f
    })
}

/// [`parse_f64_literal`] for a float field, rounding the literal to 32 bits
/// once, as Go's `strconv.ParseFloat(s, 32)` does.
fn parse_f32_literal(s: &str) -> Option<f32> {
    let f: f32 = s.parse().ok()?;
    if f.is_infinite() && !spells_infinity(s) {
        return None;
    }
    Some(if f.is_nan() {
        f32::from_bits(CANONICAL_NAN_32)
    } else {
        f
    })
}

/// Whether a field can hold `null`, protowire-go's `nullable`: a field
/// with presence that is neither repeated nor a map (a message field,
/// wrappers included, a proto3 `optional` scalar, a oneof member, a proto2
/// `optional` field). A proto2 `required` field is not nullable: unset, it
/// would make the message invalid in binary (protowire#379). Nor is a field
/// with `(pxf.required)` or `@required`, whatever its presence: a required
/// field holds a value, so `null` is rejected as absence is (#106,
/// protowire-go#268). The decoder and the writers both ask this, so a body,
/// a nested body, a `@dataset` cell and the `_null` mask follow one rule.
///
/// `is_required` reads the field's options, so it is asked last.
pub(crate) fn nullable(fd: &FieldDescriptor) -> bool {
    fd.supports_presence()
        && !fd.is_list()
        && !fd.is_map()
        && fd.cardinality() != Cardinality::Required
        && !crate::annotations::is_required(fd)
}

/// The error for a `null` that [`nullable`] refuses, saying how the schema
/// can admit one where it can.
fn null_not_allowed(pos: Position, fd: &FieldDescriptor) -> PxfError {
    let name = fd.name();
    let msg = if fd.is_map() {
        format!("null is not allowed on map field {name:?}")
    } else if fd.is_list() {
        format!("null is not allowed on repeated field {name:?}")
    } else if fd.cardinality() == Cardinality::Required {
        if matches!(fd.kind(), Kind::Message(_)) {
            format!("null is not allowed on required field {name:?}: it must be set")
        } else {
            format!(
                "null is not allowed on scalar field {name:?}: the field is required, so it must be set"
            )
        }
    } else if crate::annotations::is_required(fd) {
        format!("null is not allowed on required field {name:?}: a required field holds a value")
    } else {
        format!(
            "null is not allowed on scalar field {name:?}: declare it optional, or as a wrapper type (google.protobuf.{}Value or similar), to make it nullable",
            wrapper_name(&fd.kind())
        )
    };
    PxfError::new(pos, msg)
}

/// The `google.protobuf` wrapper stem for a scalar kind, for the hint in
/// [`null_not_allowed`].
fn wrapper_name(kind: &Kind) -> &'static str {
    match kind {
        Kind::Bool => "Bool",
        Kind::String => "String",
        Kind::Bytes => "Bytes",
        Kind::Float => "Float",
        Kind::Double => "Double",
        Kind::Int64 | Kind::Sint64 | Kind::Sfixed64 => "Int64",
        Kind::Uint32 | Kind::Fixed32 => "UInt32",
        Kind::Uint64 | Kind::Fixed64 => "UInt64",
        _ => "Int32",
    }
}

fn is_any_full_name(full: &str) -> bool {
    full == "google.protobuf.Any"
}

/// The elements a repeated field already holds, so a second binding of
/// the same field appends rather than replaces (draft -01 § Entries and
/// Keys: "elements concatenated in document order"). Taken, not copied:
/// the caller writes the list back, and a copy per binding would make a
/// field bound many times quadratic.
fn existing_list(msg: &mut DynamicMessage, fd: &FieldDescriptor) -> Vec<Value> {
    match msg.take_field(fd) {
        Some(Value::List(items)) => items,
        _ => Vec::new(),
    }
}

/// Record that this body bound `fd`, or report that it already did: a
/// singular field is bound at most once in one message body, whatever the
/// values, and two blocks do not merge (draft -01 § Entries and Keys;
/// protowire#347). A repeated or map field has no such limit.
fn check_bound(
    fd: &FieldDescriptor,
    bound: &mut HashSet<u32>,
    pos: Position,
) -> Result<(), PxfError> {
    if fd.is_list() || fd.is_map() || bound.insert(fd.number()) {
        return Ok(());
    }
    Err(PxfError::new(
        pos,
        format!("field {:?} is already set", fd.name()),
    ))
}

/// A map key as an error or a presence path shows it: a string quoted,
/// anything else bare.
pub(crate) fn shown_key(key: &MapKey) -> String {
    match key {
        MapKey::Bool(v) => v.to_string(),
        MapKey::I32(v) => v.to_string(),
        MapKey::I64(v) => v.to_string(),
        MapKey::U32(v) => v.to_string(),
        MapKey::U64(v) => v.to_string(),
        MapKey::String(v) => crate::go_strconv::quote(v),
    }
}

/// Validate `(pxf.required)` annotations and apply `(pxf.default)` values to
/// absent fields, recursing into the message bodies the input wrote: present
/// singular message fields, list elements and map values, each under the path
/// the decoder recorded its fields at (`items[0].`, `by_key["k"].`). The
/// draft's rules have no exception for elements (protowire-go#161). `Any`
/// bodies take the pass when they are decoded. The `_null` field itself is
/// skipped since it carries metadata, not user data.
fn post_decode(
    parent: &mut DynamicMessage,
    presence: &Presence,
    null_mask_fd: Option<&FieldDescriptor>,
    path_prefix: &str,
) -> Result<(), PxfError> {
    let desc = parent.descriptor();
    let pos = Position::new(1, 1);
    let plan = post_decode_plan(&desc);
    for (fd, p) in desc.fields().zip(plan.iter()) {
        if !p.required && p.default.is_none() && !p.visit {
            continue; // nothing to check here or below
        }
        if null_mask_fd.is_some_and(|null_fd| fd.number() == null_fd.number()) {
            continue;
        }
        let path = format!("{}{}", path_prefix, fd.name());
        if presence.is_absent(&path) {
            if p.required {
                return Err(PxfError::new(
                    pos,
                    format!("required field {:?} is absent", path),
                ));
            }
            if let Some(def) = &p.default {
                if !oneof_already_chosen(&fd, presence, path_prefix) {
                    apply_default_at(parent, &fd, def, pos)?;
                }
            }
        } else if p.visit && !presence.is_null(&path) && parent.has_field(&fd) {
            post_decode_bodies(parent.get_field_mut(&fd), presence, &path)?;
        }
    }
    Ok(())
}

/// Run [`post_decode`] on the message bodies in `value`, a present, non-null
/// field at `path`: the message itself, each list element, or each map value.
/// Map values go in key order, so the first error is the same on every run.
/// A null element or map value is refused while decoding, so none reaches here.
fn post_decode_bodies(value: &mut Value, presence: &Presence, path: &str) -> Result<(), PxfError> {
    match value {
        Value::Message(m) => post_decode(m, presence, None, &format!("{path}.")),
        Value::List(items) => {
            for (i, item) in items.iter_mut().enumerate() {
                if let Value::Message(m) = item {
                    post_decode(m, presence, None, &format!("{path}[{i}]."))?;
                }
            }
            Ok(())
        }
        Value::Map(map) => {
            let mut bodies: Vec<(String, &mut DynamicMessage)> = map
                .iter_mut()
                .filter_map(|(k, v)| match v {
                    Value::Message(m) => Some((format!("{path}[{}].", shown_key(k)), m)),
                    _ => None,
                })
                .collect();
            bodies.sort_by(|a, b| a.0.cmp(&b.0));
            for (prefix, m) in bodies {
                post_decode(m, presence, None, &prefix)?;
            }
            Ok(())
        }
        _ => Ok(()),
    }
}

/// Reports whether `fd` is a member of a real oneof some *other* member of
/// which the document supplied, in which case `fd`'s `(pxf.default)` MUST
/// NOT be applied (draft -01 §annotation-extensions, "Oneof Members").
/// Setting any member of a oneof clears the rest, so applying the default
/// over a chosen sibling does not shadow the supplied value — it destroys
/// it (protowire-rust#24).
///
/// Presence is read from the document, not from the decoded message's
/// current oneof case: a default applied earlier in the same pass would
/// itself set the case and let the first annotated member suppress every
/// later one. A member bound to `null` is present (`mark_null` records it
/// as present too), consistent with null suppressing a default.
///
/// A proto3 `optional` field sits in a synthetic single-member oneof that
/// nothing can clear; it is excluded so its default keeps applying.
fn oneof_already_chosen(fd: &FieldDescriptor, presence: &Presence, path_prefix: &str) -> bool {
    let Some(oo) = fd.containing_oneof() else {
        return false;
    };
    if oo.is_synthetic() {
        return false;
    }
    let chosen = oo.fields().any(|m| {
        m.number() != fd.number() && !presence.is_absent(&format!("{}{}", path_prefix, m.name()))
    });
    chosen
}

/// The value forms a message field admits, for the error when the token at
/// the value position matches none of them: protowire-go's `msgValueForms`.
/// Every message takes a block; the well-known types that also take a
/// literal say so.
fn msg_value_forms(desc: &MessageDescriptor) -> &'static str {
    match crate::wkt::dispatch_name(desc) {
        "google.protobuf.Timestamp" => "a timestamp literal or '{'",
        "google.protobuf.Duration" => "a duration literal or '{'",
        "pxf.BigInt" => "an integer literal or '{'",
        "pxf.Decimal" | "pxf.BigFloat" => "a numeric literal or '{'",
        "pxf.Secret" => "a string literal or '{'",
        _ => "'{'",
    }
}

pub(crate) fn is_wkt_skip_recursion(full: &str) -> bool {
    full == "google.protobuf.Timestamp"
        || full == "google.protobuf.Duration"
        || full == "google.protobuf.Any"
        || full == "pxf.BigInt"
        || full == "pxf.Decimal"
        || full == "pxf.BigFloat"
        || is_wrapper_full_name(full)
}

/// Write a parsed `pxf.BigInt` literal into its message: `abs` only when
/// non-zero, `negative` only when set — the same fields the reference's
/// `setBigIntFields` writes, so the pb bytes match.
fn set_big_int_fields(target: &mut DynamicMessage, lit: &BigIntLit) {
    let desc = target.descriptor();
    if !lit.abs.is_empty() {
        if let Some(fd) = desc.get_field_by_name("abs") {
            target.set_field(&fd, Value::Bytes(lit.abs.clone().into()));
        }
    }
    if lit.negative {
        if let Some(fd) = desc.get_field_by_name("negative") {
            target.set_field(&fd, Value::Bool(true));
        }
    }
}

/// Write a parsed `pxf.BigFloat` literal into its message, mirroring the
/// reference's `setBigFloatFields`: `mantissa` only when non-zero,
/// `exponent` only when non-zero, `prec` only when non-zero (an unsuffixed
/// zero has none), `negative` only when set.
fn set_big_float_fields(target: &mut DynamicMessage, lit: &BigFloatLit) {
    let desc = target.descriptor();
    if !lit.mantissa.is_empty() {
        if let Some(fd) = desc.get_field_by_name("mantissa") {
            target.set_field(&fd, Value::Bytes(lit.mantissa.clone().into()));
        }
    }
    if lit.exponent != 0 {
        if let Some(fd) = desc.get_field_by_name("exponent") {
            target.set_field(&fd, Value::I32(lit.exponent));
        }
    }
    if lit.prec != 0 {
        if let Some(fd) = desc.get_field_by_name("prec") {
            target.set_field(&fd, Value::U32(lit.prec));
        }
    }
    if lit.negative {
        if let Some(fd) = desc.get_field_by_name("negative") {
            target.set_field(&fd, Value::Bool(true));
        }
    }
}

/// Write a parsed `pxf.Decimal` literal into its message, mirroring the
/// reference's `setDecimalFields`: `unscaled` only when non-zero, `scale`
/// only when non-zero, `negative` only when set.
fn set_decimal_fields(target: &mut DynamicMessage, lit: &DecimalLit) {
    let desc = target.descriptor();
    if !lit.unscaled.is_empty() {
        if let Some(fd) = desc.get_field_by_name("unscaled") {
            target.set_field(&fd, Value::Bytes(lit.unscaled.clone().into()));
        }
    }
    if lit.scale != 0 {
        if let Some(fd) = desc.get_field_by_name("scale") {
            target.set_field(&fd, Value::I32(lit.scale));
        }
    }
    if lit.negative {
        if let Some(fd) = desc.get_field_by_name("negative") {
            target.set_field(&fd, Value::Bool(true));
        }
    }
}

/// Apply one default literal, `def`, to `fd` of `msg`, as the post-decode
/// pass applies a `(pxf.default)` to an absent field: protowire-go's
/// `ApplyDefault`. It is for a layered-config caller that decodes each layer
/// with [`UnmarshalOptions::skip_post_decode`], merges them, and runs its own
/// defaults pass over the merged message. A literal that does not parse as
/// the field's type, and a field no single literal can denote (a list, a
/// map, a message other than a well-known type with a literal form), is an
/// error.
///
/// One guard is the caller's, as it is in protowire-go: for a member of a
/// non-synthetic oneof, call this only when no member of that oneof is
/// present (draft -01 §annotation-extensions, "Oneof Members"). Setting a
/// member clears the others, so a default applied over a chosen sibling
/// destroys the merged value rather than standing in for an absent one.
/// [`crate::Presence::present_fields`] holds the presence the pass tests,
/// a member bound to `null` counting as present.
pub fn apply_default(
    msg: &mut DynamicMessage,
    fd: &FieldDescriptor,
    def: &str,
) -> Result<(), PxfError> {
    apply_default_at(msg, fd, def, Position::default())
}

fn apply_default_at(
    parent: &mut DynamicMessage,
    fd: &FieldDescriptor,
    def: &str,
    pos: Position,
) -> Result<(), PxfError> {
    // A (pxf.default) carries exactly one PXF literal, so it can denote a
    // singular field only (draft -01 §annotation-extensions, "Default
    // Placement"). `fd.kind()` reports the *element* kind and says nothing
    // about cardinality — `repeated string` is `Kind::String` — so without
    // this guard the scalar branch below handed `set_field` a scalar for a
    // list field, which prost-reflect accepts and encodes as a one-element
    // list: pb bytes no other port emits (protowire-rust#23). The spec
    // forbids inventing that semantics; the placement is an error.
    if fd.is_map() {
        return Err(PxfError::new(
            pos,
            format!("default values not supported for map field {:?}", fd.name()),
        ));
    }
    if fd.is_list() {
        return Err(PxfError::new(
            pos,
            format!(
                "default values not supported for repeated field {:?}",
                fd.name()
            ),
        ));
    }
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
    let v = parse_scalar_default(fd, fd.name(), def, pos)?;
    parent.set_field(fd, v);
    Ok(())
}

/// The value of default literal `def` for a field of `fd`'s kind. `name` is
/// the field the error names: `fd`'s own, or for a wrapper's `value` the
/// field holding the wrapper, as protowire-go names it.
fn parse_scalar_default(
    fd: &FieldDescriptor,
    name: &str,
    def: &str,
    pos: Position,
) -> Result<Value, PxfError> {
    // protowire-go's text: its own words, then the error of the Go parser
    // that refused the literal (#167).
    let err = |kind: &str, why: String| {
        PxfError::new(
            pos,
            format!(
                "invalid default {kind} {} for field {}: {why}",
                quote(def),
                quote(name)
            ),
        )
    };
    Ok(match fd.kind() {
        Kind::String => Value::String(def.to_string()),
        // A PXF bool literal is exactly `true` or `false`, as the document
        // grammar has it; protowire-go's parseBoolDefault.
        Kind::Bool => Value::Bool(match def {
            "true" => true,
            "false" => false,
            _ => {
                return Err(PxfError::new(
                    pos,
                    format!(
                        "invalid default bool {def:?} for field {name:?}: a PXF bool literal is exactly \"true\" or \"false\", case-sensitive"
                    ),
                ))
            }
        }),
        Kind::Int32 | Kind::Sint32 | Kind::Sfixed32 => {
            Value::I32(parse_int(def, 32).map_err(|e| err("int32", e))? as i32)
        }
        Kind::Int64 | Kind::Sint64 | Kind::Sfixed64 => {
            Value::I64(parse_int(def, 64).map_err(|e| err("int64", e))?)
        }
        Kind::Uint32 | Kind::Fixed32 => {
            Value::U32(parse_uint(def, 32).map_err(|e| err("uint32", e))? as u32)
        }
        Kind::Uint64 | Kind::Fixed64 => {
            Value::U64(parse_uint(def, 64).map_err(|e| err("uint64", e))?)
        }
        Kind::Float => {
            let f = parse_float(def, 32).map_err(|e| err("float", e))? as f32;
            Value::F32(if f.is_nan() {
                f32::from_bits(CANONICAL_NAN_32)
            } else {
                f
            })
        }
        Kind::Double => {
            let f = parse_float(def, 64).map_err(|e| err("double", e))?;
            Value::F64(if f.is_nan() {
                f64::from_bits(CANONICAL_NAN_64)
            } else {
                f
            })
        }
        Kind::Bytes => Value::Bytes(
            decode_base64_lenient(def)
                .map_err(|e| err("bytes", e))?
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
    let full = crate::wkt::dispatch_name(&inner_desc).to_string();
    let mut sub = DynamicMessage::new(inner_desc.clone());

    if full == "google.protobuf.Timestamp" {
        let (s, n) = crate::rfc3339::parse(def).ok_or_else(|| {
            PxfError::new(
                pos,
                format!(
                    "invalid default timestamp {} for field {}: {}",
                    quote(def),
                    quote(fd.name()),
                    crate::go_strconv::timestamp_error(def)
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
        let v = parse_scalar_default(&value_fd, fd.name(), def, pos)?;
        sub.set_field(&value_fd, v);
        parent.set_field(fd, Value::Message(sub));
        return Ok(());
    }
    // Schema literals stay under the constant cap, not the call's: a
    // (pxf.default) is the schema author's, not the document's.
    if full == "pxf.BigInt" {
        let lit = parse_big_int(def, MAX_NUMERIC_LITERAL_DIGITS).map_err(|e| {
            PxfError::new(
                pos,
                format!(
                    "invalid default big integer {:?} for field {:?}: {}",
                    def,
                    fd.name(),
                    e
                ),
            )
        })?;
        set_big_int_fields(&mut sub, &lit);
        parent.set_field(fd, Value::Message(sub));
        return Ok(());
    }
    if full == "pxf.Decimal" {
        let lit = parse_decimal(def, MAX_NUMERIC_LITERAL_DIGITS).map_err(|e| {
            PxfError::new(
                pos,
                format!(
                    "invalid default decimal {:?} for field {:?}: {}",
                    def,
                    fd.name(),
                    e
                ),
            )
        })?;
        set_decimal_fields(&mut sub, &lit);
        parent.set_field(fd, Value::Message(sub));
        return Ok(());
    }
    if full == "pxf.BigFloat" {
        let lit = parse_big_float_default(def).map_err(|e| {
            PxfError::new(
                pos,
                format!(
                    "invalid default big float {:?} for field {:?}: {}",
                    def,
                    fd.name(),
                    e
                ),
            )
        })?;
        set_big_float_fields(&mut sub, &lit);
        parent.set_field(fd, Value::Message(sub));
        return Ok(());
    }
    Err(PxfError::new(
        pos,
        format!(
            "default values not supported for message type {} (field {:?})",
            inner_desc.full_name(),
            fd.name()
        ),
    ))
}

/// Whether `desc` is a `pxf.Secret` of the shape the draft gives it
/// (§ Well-Known Type Shapes): a singular string `value`, and `hint` and
/// `fingerprint`, each optional, singular strings. Another message of that
/// name is an ordinary message, with no literal form.
pub(crate) fn is_secret(desc: &MessageDescriptor) -> bool {
    crate::wkt::dispatch_name(desc) == "pxf.Secret"
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
///
/// `kind` is the token the key arrived as — `map-key = identifier / string
/// / integer / bool` (draft -01 §abnf-grammar) — because the spelling
/// decides what a key means on a bool `K`, and a quoted `"1"` is not the
/// same key as a bare `1`.
fn decode_map_key(
    field: &FieldDescriptor,
    key_fd: &FieldDescriptor,
    key: String,
    kind: TokenKind,
    pos: Position,
) -> Result<MapKey, PxfError> {
    match key_fd.kind() {
        Kind::String => {
            if matches!(kind, TokenKind::Bool) {
                // A bool key matches a map<bool,V> field and nothing else;
                // the string "true" is spelled quoted.
                return Err(PxfError::new(
                    pos,
                    format!(
                        "invalid string map key {} for field {:?}: the keyword {} is a bool key; write {:?} for the string",
                        key,
                        field.name(),
                        key,
                        key
                    ),
                ));
            }
            Ok(MapKey::String(key))
        }
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
        Kind::Bool => {
            // A bool map key has three spellings in the grammar (draft -01
            // §entries-and-keys; protowire#284): the keyword true / false,
            // bare; the bare integers 0 / 1 (an integer key matches "bool
            // encoded as 0/1"); and the quoted literals "true" / "false" (a
            // string key "is parsed as a literal of K's type", and a PXF
            // bool literal is exactly those two words).
            //
            // NOT the twelve strconv.ParseBool spellings — t, T, TRUE, True,
            // f, F, FALSE, False — which no port that follows the grammar
            // binds. An identifier key on a bool K matches nothing: an
            // identifier names a field, and a map has none. "1" / "0" in
            // quotes are rejected too: an integer literal inside a string
            // is not a bool literal (protowire-rust#31).
            let spelled = match kind {
                TokenKind::Bool => Some(key == "true"),
                TokenKind::String => match key.as_str() {
                    "true" => Some(true),
                    "false" => Some(false),
                    _ => None,
                },
                TokenKind::Int => match key.as_str() {
                    "1" => Some(true),
                    "0" => Some(false),
                    _ => None,
                },
                _ => None,
            };
            match spelled {
                Some(b) => Ok(MapKey::Bool(b)),
                None => {
                    let shown = if matches!(kind, TokenKind::String) {
                        format!("{:?}", key)
                    } else {
                        key
                    };
                    Err(PxfError::new(
                        pos,
                        format!(
                            "invalid bool map key {} for field {:?}: a bool key is true, false, 0, 1, \"true\" or \"false\"",
                            shown,
                            field.name()
                        ),
                    ))
                }
            }
        }
        other => Err(PxfError::new(
            pos,
            format!("unsupported map key kind: {:?}", other),
        )),
    }
}

/// Parse a Go-style duration (`1h30m`, `-2.5s`, `100ms`, …) into proto
/// Duration `seconds` + `nanos`. Both fields share the overall sign per the
/// proto Duration invariant. Lexer has pre-validated the syntax.
/// A duration literal's seconds and nanoseconds, from Go's
/// `time.ParseDuration` (`go_duration::parse`), split as protowire-go's
/// `setDurationFields` splits them: both truncated toward zero, with the
/// literal's sign.
fn parse_go_duration(s: &str) -> Result<(i64, i32), String> {
    let d = crate::go_duration::parse(s)?;
    Ok((d / 1_000_000_000, (d % 1_000_000_000) as i32))
}
