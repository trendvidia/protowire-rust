// SPDX-License-Identifier: MIT
// Copyright (c) 2026 TrendVidia, LLC.
//! Position-aware errors for PXF parse / decode, and position-less ones
//! for marshal, schema checks and streams.
//!
//! Mirrors `protowire/encoding/pxf/errors.go` and the TS port's `pxf/errors.ts`.

use std::error::Error as StdError;
use std::fmt;
use std::sync::Arc;

use crate::token::Position;

/// A PXF error: a position, when it has one, and protowire-go's text.
///
/// A decode that its [`crate::UnmarshalOptions::validator`] refused has no
/// position, and its text is the validator's. Its
/// [`source`](std::error::Error::source) is a [`protowire_check::Error`]
/// holding the whole report when the validator reported violations, or
/// the validator's own error when it could not run.
/// [`PxfError::check_error`] is the first of those.
#[derive(Debug, Clone)]
pub struct PxfError {
    pub pos: Position,
    pub msg: String,
    source: Option<Arc<dyn StdError + Send + Sync>>,
}

impl PxfError {
    pub fn new(pos: Position, msg: impl Into<String>) -> Self {
        Self {
            pos,
            msg: msg.into(),
            source: None,
        }
    }

    /// The error of a validator that refused a decode: its text, no
    /// position, and the validator's error as the source.
    pub(crate) fn from_validator(e: protowire_check::BoxError) -> Self {
        let source: Arc<dyn StdError + Send + Sync> = Arc::from(e);
        Self {
            pos: Position::default(),
            msg: source.to_string(),
            source: Some(source),
        }
    }

    /// The violations, when a validator refused the decode for them; `None`
    /// for any other error, a validator's own failure included.
    pub fn check_error(&self) -> Option<&protowire_check::Error> {
        self.source.as_deref()?.downcast_ref()
    }
}

/// Errors are equal when their positions and texts are; the source is not
/// compared.
impl PartialEq for PxfError {
    fn eq(&self, other: &Self) -> bool {
        self.pos == other.pos && self.msg == other.msg
    }
}

impl Eq for PxfError {}

impl StdError for PxfError {
    fn source(&self) -> Option<&(dyn StdError + 'static)> {
        self.source
            .as_deref()
            .map(|e| e as &(dyn StdError + 'static))
    }
}

impl PxfError {
    /// Whether the error carries a source position. Lines and columns count
    /// from 1, so `Position::default()` (0:0) is no position: an error from
    /// `marshal`, a schema check, or a stream rather than a token (#138).
    pub fn has_position(&self) -> bool {
        self.pos != Position::default()
    }
}

/// `line:col: msg`, as protowire-go's `Error` prints. An error with no
/// position prints its message alone, which then carries protowire-go's
/// own text (`pxf: nesting depth exceeds MaxNestingDepth=100`).
impl fmt::Display for PxfError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if self.has_position() {
            write!(f, "{}: {}", self.pos, self.msg)
        } else {
            f.write_str(&self.msg)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_error_prints_its_position_only_when_it_has_one() {
        let at = PxfError::new(Position::new(2, 1), "unknown field \"x\"");
        assert!(at.has_position());
        assert_eq!(at.to_string(), "2:1: unknown field \"x\"");
        let none = PxfError::new(Position::default(), "pxf: no @dataset directive in stream");
        assert!(!none.has_position());
        assert_eq!(none.to_string(), "pxf: no @dataset directive in stream");
    }
}
