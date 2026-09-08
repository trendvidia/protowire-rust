// SPDX-License-Identifier: MIT
// Copyright (c) 2026 TrendVidia, LLC.
//! HARDENING.md § Mandatory limits, as this port ships them and as a
//! caller can lower them per call (draft -01 § Mandatory Limits: every
//! limit but `MaxVarintBytes` is "configurable per call by the calling
//! application").
//!
//! The constants are the defaults every port must ship with and the
//! values the conformance corpus is run at. [`Limits`] is the per-call
//! form: [`Limits::default()`] is the constants, and a caller lowers one
//! with struct-update syntax (`Limits { max_message_size: 1 << 20,
//! ..Limits::default() }`). Schema input — a `(pxf.default)` literal —
//! stays under the constants whatever the call says: it is the schema
//! author's, not the document's.

/// Caps `{` and `[` nesting (HARDENING § Recursion). The root message is
/// depth 0 and every `{` or `[` is one descent, so a document exactly
/// this deep decodes and one level more is rejected — before the
/// recursive descent can overflow the native stack.
pub const MAX_NESTING_DEPTH: usize = 100;

/// Caps the total input to one decode or parse call: peak memory is a
/// multiple of the input, so the input is what bounds it. Checked before
/// the first token is read; the `@dataset` stream reader applies it to
/// the bytes it holds while looking for a row boundary.
pub const MAX_MESSAGE_SIZE: usize = 64 << 20;

/// Caps the digit count of any single numeric literal before it is
/// converted into a `pxf.BigInt` / `pxf.Decimal` / `pxf.BigFloat` — the
/// conversions are quadratic — and bounds the magnitude of
/// `pxf.Decimal.scale` on the PB wire. A literal of exactly this many
/// digits is within the limit.
pub const MAX_NUMERIC_LITERAL_DIGITS: usize = 4096;

/// Caps the decoded length of one `b"…"` literal. Bounded by
/// [`MAX_MESSAGE_SIZE`] transitively; the lexer checks it from the
/// literal's length before decoding, so the check holds when a caller
/// raises the message cap alone.
pub const MAX_BYTES_LITERAL_LENGTH: usize = MAX_MESSAGE_SIZE;

/// Caps the element count of any repeated or map field. Elements are at
/// least one byte of input each, so the count is bounded by
/// [`MAX_MESSAGE_SIZE`] transitively; the check holds when a caller
/// raises the message cap alone.
pub const MAX_REPEATED_COUNT: usize = MAX_MESSAGE_SIZE;

/// The per-call limits of one decode or parse. The default is the
/// HARDENING constants; lower a field to tighten one call.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Limits {
    /// [`MAX_MESSAGE_SIZE`]: total input to this call, checked before the
    /// first token is read.
    pub max_message_size: usize,
    /// [`MAX_NESTING_DEPTH`]: `{` / `[` nesting.
    pub max_nesting_depth: usize,
    /// [`MAX_NUMERIC_LITERAL_DIGITS`]: digits of a `pxf.BigInt` /
    /// `pxf.Decimal` literal in the document.
    pub max_numeric_literal_digits: usize,
    /// [`MAX_BYTES_LITERAL_LENGTH`]: decoded length of a `b"…"` literal,
    /// judged from its length before it is decoded.
    pub max_bytes_literal_length: usize,
    /// [`MAX_REPEATED_COUNT`]: elements of a repeated field or entries of
    /// a map, refused before the element past the bound is added.
    pub max_repeated_count: usize,
}

impl Default for Limits {
    fn default() -> Self {
        Self {
            max_message_size: MAX_MESSAGE_SIZE,
            max_nesting_depth: MAX_NESTING_DEPTH,
            max_numeric_literal_digits: MAX_NUMERIC_LITERAL_DIGITS,
            max_bytes_literal_length: MAX_BYTES_LITERAL_LENGTH,
            max_repeated_count: MAX_REPEATED_COUNT,
        }
    }
}

impl Limits {
    /// The error for input past [`Limits::max_message_size`], shared by
    /// every entry point so the wording names the limit the same way.
    pub(crate) fn check_message_size(&self, len: usize) -> Result<(), String> {
        if len > self.max_message_size {
            return Err(format!(
                "input of {} bytes exceeds MaxMessageSize={}",
                len, self.max_message_size
            ));
        }
        Ok(())
    }
}
