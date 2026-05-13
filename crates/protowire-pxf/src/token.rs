// SPDX-License-Identifier: MIT
// Copyright (c) 2026 TrendVidia, LLC.
//! Lexical tokens and source positions for PXF (Proto eXpressive Format).
//!
//! Mirrors `protowire/encoding/pxf/token.go` and the TS port's `pxf/token.ts`.

use std::fmt;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum TokenKind {
    Eof,
    Illegal,
    Newline,
    Comment,

    Ident,
    String,
    Int,
    Float,
    Bool,
    Null,
    Bytes,
    Timestamp,
    Duration,

    LBrace,
    RBrace,
    LBracket,
    RBracket,
    /// `(` — used by @dataset column list and row tuples.
    LParen,
    /// `)`
    RParen,
    Equals,
    Colon,
    Comma,

    AtType,
    /// `@<ident>` for any non-reserved name. The token's `value`
    /// carries the bare name (no leading `@`); the parser uses it
    /// as the directive's name.
    AtDirective,
    /// `@dataset` — row-oriented bulk-data directive (draft §3.4.4).
    AtDataset,
    /// `@proto` — embedded protobuf schema directive (draft §3.4.5).
    AtProto,
}

impl TokenKind {
    pub fn name(self) -> &'static str {
        match self {
            TokenKind::Eof => "EOF",
            TokenKind::Illegal => "ILLEGAL",
            TokenKind::Newline => "newline",
            TokenKind::Comment => "comment",
            TokenKind::Ident => "identifier",
            TokenKind::String => "string",
            TokenKind::Int => "integer",
            TokenKind::Float => "float",
            TokenKind::Bool => "bool",
            TokenKind::Null => "null",
            TokenKind::Bytes => "bytes",
            TokenKind::Timestamp => "timestamp",
            TokenKind::Duration => "duration",
            TokenKind::LBrace => "{",
            TokenKind::RBrace => "}",
            TokenKind::LBracket => "[",
            TokenKind::RBracket => "]",
            TokenKind::LParen => "(",
            TokenKind::RParen => ")",
            TokenKind::Equals => "=",
            TokenKind::Colon => ":",
            TokenKind::Comma => ",",
            TokenKind::AtType => "@type",
            TokenKind::AtDirective => "@<directive>",
            TokenKind::AtDataset => "@dataset",
            TokenKind::AtProto => "@proto",
        }
    }
}

impl fmt::Display for TokenKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.name())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub struct Position {
    pub line: usize,
    pub column: usize,
    /// Byte offset into the lexer's input. Used by directive body
    /// extraction to slice the raw bytes between `{` and `}`;
    /// line/column remain the primary user-facing identifier. Zero is
    /// the start of input.
    pub offset: usize,
}

impl Position {
    pub fn new(line: usize, column: usize) -> Self {
        Self {
            line,
            column,
            offset: 0,
        }
    }

    pub fn with_offset(line: usize, column: usize, offset: usize) -> Self {
        Self {
            line,
            column,
            offset,
        }
    }
}

impl fmt::Display for Position {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}:{}", self.line, self.column)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Token {
    pub kind: TokenKind,
    pub value: String,
    pub pos: Position,
}

impl Token {
    pub fn new(kind: TokenKind, value: impl Into<String>, pos: Position) -> Self {
        Self {
            kind,
            value: value.into(),
            pos,
        }
    }
}
