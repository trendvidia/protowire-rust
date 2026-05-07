// SPDX-License-Identifier: MIT
// Copyright (c) 2026 TrendVidia, LLC.
//! Errors raised by the SBE codec.

use std::fmt;

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub struct SbeError {
    pub msg: String,
}

impl SbeError {
    pub fn new(msg: impl Into<String>) -> Self {
        Self { msg: msg.into() }
    }
}

impl fmt::Display for SbeError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.msg)
    }
}
