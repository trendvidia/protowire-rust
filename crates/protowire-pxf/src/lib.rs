//! PXF (Proto eXpressive Format) — schema-driven text codec.
//!
//! Port of `github.com/trendvidia/protowire/encoding/pxf`. Lands across
//! Slices A through F.

pub mod ast;
pub mod errors;
pub mod lexer;
pub mod parser;
pub mod token;

pub use ast::{
    Assignment, Block, BlockVal, BoolVal, BytesVal, Comment, Document, DurationVal, Entry,
    FloatVal, IdentVal, IntVal, ListVal, MapEntry, NullVal, StringVal, TimestampVal, Value,
};
pub use errors::PxfError;
pub use lexer::Lexer;
pub use parser::parse;
pub use token::{Position, Token, TokenKind};
