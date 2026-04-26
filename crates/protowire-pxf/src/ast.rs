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
    pub entries: Vec<Entry>,
    /// Comments before the first entry (or before `@type`).
    pub leading_comments: Vec<Comment>,
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
