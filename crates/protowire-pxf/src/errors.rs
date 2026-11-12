// SPDX-License-Identifier: MIT
// Copyright (c) 2026 TrendVidia, LLC.
//! Position-aware errors for PXF parse / decode, and position-less ones
//! for marshal, schema checks and streams.
//!
//! Mirrors `protowire/encoding/pxf/errors.go` and the TS port's `pxf/errors.ts`.

use std::fmt;

use crate::token::Position;

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub struct PxfError {
    pub pos: Position,
    pub msg: String,
}

impl PxfError {
    pub fn new(pos: Position, msg: impl Into<String>) -> Self {
        Self {
            pos,
            msg: msg.into(),
        }
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
