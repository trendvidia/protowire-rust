// SPDX-License-Identifier: MIT
// Copyright (c) 2026 TrendVidia, LLC.
//! The validation seam the protowire decoders share, as protowire-go's
//! `check` package.
//!
//! A [`Validator`] runs after a successful decode and reports whether the
//! decoded value satisfies some rule set: `buf.validate` annotations through
//! a protovalidate adapter, say. `protowire-pxf`, `protowire-pb` and
//! `protowire-sbe` each take one in their decode options and call
//! [`validate`] on what they decoded. This crate has no dependencies, so the
//! codecs depend on it and on no validation engine.
//!
//! A decode that a validator refuses fails with an error whose text is
//! [`Error`]'s, `check: port: port out of range (test.rule)`. The [`Error`]
//! itself, with the whole [`Report`], is that error's
//! [`source`](std::error::Error::source):
//!
//! ```
//! # use protowire_check::{Error, Report};
//! fn report_of<'a>(err: &'a (dyn std::error::Error + 'static)) -> Option<&'a Report> {
//!     err.source()?.downcast_ref::<Error>().map(|e| &e.report)
//! }
//! ```
//!
//! An error the validator itself returns (its engine failed) is the source
//! instead, as it was returned, and is not an [`Error`].

use std::any::Any;
use std::error::Error as StdError;
use std::fmt;

/// An error a [`Validator`] returns when it could not run, as opposed to a
/// [`Report`] of what it found.
pub type BoxError = Box<dyn StdError + Send + Sync>;

/// Data validation over a decoded value. The value is the decoder's own:
/// a `prost_reflect::DynamicMessage` from pxf and sbe, the caller's
/// `Message` type from pb. A validator downcasts it to what it knows.
///
/// It returns the [`Report`] of what it found, empty when the value
/// passes, or an error when it could not validate at all. A closure of the
/// same shape is a validator.
pub trait Validator {
    fn validate(&self, decoded: &dyn Any) -> Result<Report, BoxError>;
}

impl<F> Validator for F
where
    F: Fn(&dyn Any) -> Result<Report, BoxError>,
{
    fn validate(&self, decoded: &dyn Any) -> Result<Report, BoxError> {
        self(decoded)
    }
}

/// One failed rule.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Violation {
    /// The dotted path to the offending field (`"port"`,
    /// `"servers[0].host"`), or empty when the violation concerns the
    /// whole message.
    pub path: String,
    /// The engine's identifier for the rule (`"rfc001.required"`), or
    /// empty.
    pub rule_id: String,
    /// What is wrong, for a person to read.
    pub message: String,
}

/// What a [`Validator`] found. It passes when it holds no violations.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Report {
    pub violations: Vec<Violation>,
}

impl Report {
    /// Whether the report holds no violations.
    pub fn ok(&self) -> bool {
        self.violations.is_empty()
    }
}

/// The failure a decode returns when its [`Validator`] reports
/// violations, carrying the [`Report`]. Its text names the first
/// violation: `check: port: value is required (rfc001.required)`, or
/// `check: 2 violations; first: a: first` when there are more.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Error {
    pub report: Report,
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let Some(v) = self.report.violations.first() else {
            return f.write_str("check: validation failed");
        };
        let mut msg = v.message.clone();
        if !v.path.is_empty() {
            msg = format!("{}: {msg}", v.path);
        }
        if !v.rule_id.is_empty() {
            msg = format!("{msg} ({})", v.rule_id);
        }
        match self.report.violations.len() {
            1 => write!(f, "check: {msg}"),
            n => write!(f, "check: {n} violations; first: {msg}"),
        }
    }
}

impl StdError for Error {}

/// Run `v` over `decoded`, as the decoders do after a successful decode.
///
/// - No validator: `Ok(None)`.
/// - A clean report: `Ok(Some(report))`.
/// - Violations: an [`Error`] holding the report, boxed.
/// - The validator's own error: that error, unchanged, so a caller can
///   still downcast it to the engine's type.
pub fn validate(v: Option<&dyn Validator>, decoded: &dyn Any) -> Result<Option<Report>, BoxError> {
    let Some(v) = v else { return Ok(None) };
    let report = v.validate(decoded)?;
    if !report.ok() {
        return Err(Box::new(Error { report }));
    }
    Ok(Some(report))
}
