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
    Equals,
    Colon,
    Comma,

    AtType,
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
            TokenKind::Equals => "=",
            TokenKind::Colon => ":",
            TokenKind::Comma => ",",
            TokenKind::AtType => "@type",
        }
    }
}

impl fmt::Display for TokenKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.name())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Position {
    pub line: usize,
    pub column: usize,
}

impl Position {
    pub fn new(line: usize, column: usize) -> Self {
        Self { line, column }
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
