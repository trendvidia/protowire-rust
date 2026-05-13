// SPDX-License-Identifier: MIT
// Copyright (c) 2026 TrendVidia, LLC.
//! PXF AST types.
//!
//! Mirrors `protowire/encoding/pxf/ast.go`. Uses Rust enums (sum types) for
//! `Entry` and `Value` rather than Go's interface-with-marker pattern.
//!
//! Timestamps and durations are kept as their raw lexeme on the AST (matching
//! the TS port). A downstream consumer (decoder, formatter) parses them when
//! needed — Rust has `prost-types::Timestamp`/`Duration` for the wire side.

use crate::token::Position;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Comment {
    pub pos: Position,
    /// Raw text including the comment prefix (`#`, `//`, or block delimiters).
    pub text: String,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Document {
    /// Empty when there is no `@type` directive.
    pub type_url: String,
    /// `@<name> *(prefix) [{ ... }]` blocks in source order; excludes
    /// the spec-defined directives (`@type`, `@dataset`, `@proto`,
    /// `@entry`).
    pub directives: Vec<Directive>,
    /// `@dataset TYPE ( cols ) row*` directives in source order. Per
    /// draft §3.4.4 a document with any `@dataset` MUST NOT also have
    /// `@type` or top-level field entries — the parser enforces this.
    pub datasets: Vec<DatasetDirective>,
    /// `@proto <body>` directives in source order (draft §3.4.5).
    pub protos: Vec<ProtoDirective>,
    /// Byte offset where the schema-typed body begins (after all
    /// leading directives). Zero when there are no directives, so
    /// chameleon hashes from byte 0.
    pub body_offset: usize,
    pub entries: Vec<Entry>,
    /// Comments before the first entry (or before `@type`).
    pub leading_comments: Vec<Comment>,
}

/// A top-of-document `@<name> *(<prefix-id>) [{ ... }]` entry. Side-
/// channel metadata that sits alongside the schema-typed body — e.g.
/// chameleon's `@header chameleon.v1.LayerHeader { id = "x" }`. The
/// grammar is open-ended: any name except `type` / `table` is parsed
/// as a generic `Directive`. Prefix identifiers are positional and
/// per-directive:
///
///   - One prefix (v0.72.0 conventional shape) — the identifier names
///     the inner block's message type, dotted. Used by `@header` and
///     similar.
///   - `@entry` (draft §3.4.3) — zero, one, or two prefix identifiers
///     (label, type); a single prefix is disambiguated by the presence
///     of a `.` (dotted ⇒ type; bare ⇒ label).
///
/// `body` holds the raw bytes between `{` and `}` (both exclusive),
/// suitable for handing back to a follow-up `unmarshal` against the
/// consumer's chosen message. `body` is empty and `has_body` is
/// `false` when the directive has no inline block.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Directive {
    pub pos: Position,
    /// e.g. "header"; never "type" / "dataset".
    pub name: String,
    /// Identifiers between `@<name>` and the optional `{ ... }`, in
    /// source order.
    pub prefixes: Vec<String>,
    /// Back-compat for v0.72.0-era consumers: when exactly one prefix
    /// identifier was supplied, `type` holds it. For zero / two-plus
    /// prefixes, `type` is empty and callers MUST read `prefixes`
    /// directly.
    pub r#type: String,
    /// Raw inner bytes of the block; empty when `has_body` is `false`.
    pub body: Vec<u8>,
    pub has_body: bool,
    pub leading_comments: Vec<Comment>,
}

/// `@dataset <type> ( col1, col2, ... ) row*` directive at document
/// root (draft §3.4.4). Carries many instances of one message type in
/// a single document — the protowire-native CSV.
///
/// Cells are scalar-shaped in v1 (no list, no block). See [`DatasetRow`]
/// for the per-cell representation.
///
/// A document with any `DatasetDirective` MUST NOT have a `@type`
/// directive or any top-level field entries: the `@dataset` header IS
/// the document's type declaration. The parser enforces this.
///
/// `type` MAY be empty when an anonymous `@proto` directive (draft
/// §3.4.5) precedes the dataset in document order; the anonymous
/// schema is consumed as the row message type.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct DatasetDirective {
    pub pos: Position,
    /// Row message type, e.g. "trades.v1.Trade".
    pub r#type: String,
    /// Top-level field names on `type`; length >= 1.
    pub columns: Vec<String>,
    pub rows: Vec<DatasetRow>,
    pub leading_comments: Vec<Comment>,
}

/// Shape of a [`ProtoDirective`]'s body (draft §3.4.5).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ProtoShape {
    /// `@proto { <message-body> }` — defines an unnamed message used
    /// by the next typed directive in document order (one-shot
    /// binding).
    #[default]
    Anonymous,
    /// `@proto <dotted-name> { <message-body> }` — sugar for a single
    /// named message; `type_name` carries the dotted name.
    Named,
    /// `@proto """<proto-source>"""` — a complete `.proto` source
    /// file carried as a triple-quoted string.
    Source,
    /// `@proto b"<base64-FileDescriptorSet>"` — base64-encoded
    /// `google.protobuf.FileDescriptorSet` bytes.
    Descriptor,
}

impl ProtoShape {
    pub fn name(self) -> &'static str {
        match self {
            ProtoShape::Anonymous => "anonymous",
            ProtoShape::Named => "named",
            ProtoShape::Source => "source",
            ProtoShape::Descriptor => "descriptor",
        }
    }
}

/// `@proto <body>` directive at document root (draft §3.4.5). Carries
/// an embedded protobuf schema, making the PXF document
/// self-describing.
///
/// `body` carries raw bytes per `shape`:
///
///   - [`ProtoShape::Anonymous`] / [`ProtoShape::Named`]: bytes
///     between the opening `{` and matching `}` (both exclusive).
///     Protobuf message-body source.
///   - [`ProtoShape::Source`]: contents of the triple-quoted string
///     (with leading-LF stripping and common-prefix dedent already
///     applied). A complete `.proto` source file.
///   - [`ProtoShape::Descriptor`]: base64-decoded bytes of the bytes
///     literal. A serialised `google.protobuf.FileDescriptorSet`.
///
/// `type_name` is non-empty only when `shape == ProtoShape::Named`.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ProtoDirective {
    pub pos: Position,
    pub shape: ProtoShape,
    pub type_name: String,
    pub body: Vec<u8>,
    pub leading_comments: Vec<Comment>,
}

/// One parenthesized cell tuple in a `@dataset` directive. `cells` has
/// the same length as the containing `DatasetDirective.columns`. A
/// `None` cell denotes an absent field (the empty cell between two
/// commas); a `Some(Value::Null(...))` cell denotes a present-but-
/// null field; any other `Some(Value::*)` denotes a present field
/// with that value.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct DatasetRow {
    pub pos: Position,
    pub cells: Vec<Option<Value>>,
}

// ---------------------------------------------------------------------------
// Entries — what appears in a message or map body
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Entry {
    Assignment(Assignment),
    MapEntry(MapEntry),
    Block(Block),
}

impl Entry {
    pub fn pos(&self) -> Position {
        match self {
            Entry::Assignment(a) => a.pos,
            Entry::MapEntry(m) => m.pos,
            Entry::Block(b) => b.pos,
        }
    }
}

/// `key = value` — a field assignment in a message context.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Assignment {
    pub pos: Position,
    pub key: String,
    pub value: Value,
    pub leading_comments: Vec<Comment>,
    /// Inline comment after the value on the same source line, if any.
    pub trailing_comment: String,
}

/// `key: value` — a key-value pair in a map context.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MapEntry {
    pub pos: Position,
    pub key: String,
    pub value: Value,
    pub leading_comments: Vec<Comment>,
    pub trailing_comment: String,
}

/// `name { entries }` — a nested message.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Block {
    pub pos: Position,
    pub name: String,
    pub entries: Vec<Entry>,
    pub leading_comments: Vec<Comment>,
}

// ---------------------------------------------------------------------------
// Values — what appears on the right of `=` or `:`
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Value {
    String(StringVal),
    Int(IntVal),
    Float(FloatVal),
    Bool(BoolVal),
    Bytes(BytesVal),
    Null(NullVal),
    Ident(IdentVal),
    Timestamp(TimestampVal),
    Duration(DurationVal),
    List(ListVal),
    Block(BlockVal),
}

impl Value {
    pub fn pos(&self) -> Position {
        match self {
            Value::String(v) => v.pos,
            Value::Int(v) => v.pos,
            Value::Float(v) => v.pos,
            Value::Bool(v) => v.pos,
            Value::Bytes(v) => v.pos,
            Value::Null(v) => v.pos,
            Value::Ident(v) => v.pos,
            Value::Timestamp(v) => v.pos,
            Value::Duration(v) => v.pos,
            Value::List(v) => v.pos,
            Value::Block(v) => v.pos,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StringVal {
    pub pos: Position,
    pub value: String,
}

/// Integer literal, preserved as raw text — schema-bound decoder picks
/// the right numeric type (int32, int64, etc).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IntVal {
    pub pos: Position,
    pub raw: String,
}

/// Floating-point literal, raw text.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FloatVal {
    pub pos: Position,
    pub raw: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BoolVal {
    pub pos: Position,
    pub value: bool,
}

/// Decoded base64 bytes (the wire-side representation).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BytesVal {
    pub pos: Position,
    pub value: Vec<u8>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NullVal {
    pub pos: Position,
}

/// Unquoted identifier used as a value — typically an enum name.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IdentVal {
    pub pos: Position,
    pub name: String,
}

/// RFC 3339 timestamp literal, raw text.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TimestampVal {
    pub pos: Position,
    pub raw: String,
}

/// Go-style duration literal, raw text.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DurationVal {
    pub pos: Position,
    pub raw: String,
}

/// `[ … ]` — a list of values.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ListVal {
    pub pos: Position,
    pub elements: Vec<Value>,
}

/// Anonymous `{ … }` block — used for map entries and inline messages in lists.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BlockVal {
    pub pos: Position,
    pub entries: Vec<Entry>,
}
