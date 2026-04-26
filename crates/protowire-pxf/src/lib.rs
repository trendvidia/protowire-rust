//! PXF (Proto eXpressive Format) — schema-driven text codec.
//!
//! Port of `github.com/trendvidia/protowire/encoding/pxf`. Lands across
//! Slices A through F.

pub mod errors;
pub mod lexer;
pub mod token;

pub use errors::PxfError;
pub use lexer::Lexer;
pub use token::{Position, Token, TokenKind};
