//! PXF (Proto eXpressive Format) — schema-driven text codec.
//!
//! Port of `github.com/trendvidia/protowire/encoding/pxf`. Lands across
//! Slices A through F.

pub mod annotations;
pub mod ast;
pub mod decode;
pub mod encode;
pub mod errors;
pub mod format;
pub mod lexer;
pub mod parser;
pub mod result;
pub mod token;

pub use ast::{
    Assignment, Block, BlockVal, BoolVal, BytesVal, Comment, Document, DurationVal, Entry,
    FloatVal, IdentVal, IntVal, ListVal, MapEntry, NullVal, StringVal, TimestampVal, Value,
};
pub use decode::{unmarshal, unmarshal_full, PoolResolver, TypeResolver, UnmarshalOptions};
pub use encode::{marshal, MarshalOptions};
pub use result::Presence;
pub use errors::PxfError;
pub use format::{format, format_with_options, FormatOptions};
pub use lexer::Lexer;
pub use parser::parse;
pub use token::{Position, Token, TokenKind};
