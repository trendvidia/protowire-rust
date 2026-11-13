// SPDX-License-Identifier: MIT
// Copyright (c) 2026 TrendVidia, LLC.
//! Errors raised by the SBE codec.

use std::error::Error as StdError;
use std::fmt;
use std::sync::Arc;

/// An SBE error, with protowire-go's text.
///
/// A decode that the codec's validator ([`crate::Codec::with_validator`])
/// refused has the validator's text. Its
/// [`source`](std::error::Error::source) is a [`protowire_check::Error`]
/// holding the whole report when the validator reported violations, or
/// the validator's own error when it could not run.
/// [`SbeError::check_error`] is the first of those.
#[derive(Debug, Clone)]
pub struct SbeError {
    pub msg: String,
    source: Option<Arc<dyn StdError + Send + Sync>>,
}

impl SbeError {
    pub fn new(msg: impl Into<String>) -> Self {
        Self {
            msg: msg.into(),
            source: None,
        }
    }

    /// The error of a validator that refused a decode: its text, and the
    /// validator's error as the source.
    pub(crate) fn from_validator(e: protowire_check::BoxError) -> Self {
        let source: Arc<dyn StdError + Send + Sync> = Arc::from(e);
        Self {
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

/// Errors are equal when their texts are; the source is not compared.
impl PartialEq for SbeError {
    fn eq(&self, other: &Self) -> bool {
        self.msg == other.msg
    }
}

impl Eq for SbeError {}

impl StdError for SbeError {
    fn source(&self) -> Option<&(dyn StdError + 'static)> {
        self.source
            .as_deref()
            .map(|e| e as &(dyn StdError + 'static))
    }
}

impl fmt::Display for SbeError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.msg)
    }
}
