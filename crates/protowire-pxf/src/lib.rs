// SPDX-License-Identifier: MIT
// Copyright (c) 2026 TrendVidia, LLC.
//! PXF (Proto eXpressive Format) — schema-driven text codec.
//!
//! Port of `github.com/trendvidia/protowire/encoding/pxf`. Lands across
//! Slices A through F.

pub mod annotations;
pub mod ast;
pub mod bigfloat;
pub mod bignum;
pub mod dataset_reader;
pub mod decode;
pub mod encode;
pub mod errors;
pub mod format;
pub mod keyed;
pub mod lexer;
pub mod limits;
pub mod parser;
pub mod result;
pub mod schema;
pub mod token;

pub use ast::{
    Assignment, Block, BlockVal, BoolVal, BytesVal, Comment, DatasetDirective, DatasetRow,
    Directive, Document, DurationVal, Entry, FloatVal, IdentVal, IntVal, ListVal, MapEntry,
    NullVal, StringVal, TimestampVal, Value,
};
pub use dataset_reader::{bind_row, DatasetReader, DEFAULT_HEADER_MAX_BYTES};
pub use decode::{unmarshal, unmarshal_full, PoolResolver, TypeResolver, UnmarshalOptions};
pub use encode::{marshal, MarshalOptions};
pub use errors::PxfError;
pub use format::{format, format_with_options, FormatOptions};
pub use keyed::{canonicalize_keyed, ident_safe_entry_name, key_field};
pub use lexer::Lexer;
pub use limits::{
    Limits, MAX_BYTES_LITERAL_LENGTH, MAX_MESSAGE_SIZE, MAX_NESTING_DEPTH,
    MAX_NUMERIC_LITERAL_DIGITS, MAX_REPEATED_COUNT,
};
pub use parser::{parse, parse_with_limits};
pub use result::Presence;
pub use schema::{
    is_defaultable_message, validate_descriptor, validate_file, Violation, ViolationKind,
};
pub use token::{Position, Token, TokenKind};
