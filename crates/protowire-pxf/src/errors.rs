// SPDX-License-Identifier: MIT
// Copyright (c) 2026 TrendVidia, LLC.
//! Position-aware errors for PXF parse / decode.
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

impl fmt::Display for PxfError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}: {}", self.pos, self.msg)
    }
}
