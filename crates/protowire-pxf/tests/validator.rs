// SPDX-License-Identifier: MIT
// Copyright (c) 2026 TrendVidia, LLC.
//! `UnmarshalOptions::validator`: protowire-go's
//! `encoding/pxf/validator_test.go`, ported. Go's `ValConfig {name, port}`
//! is the canary's `AllTypes`, whose first field is a string.
//!
//! Go's `UnmarshalFull` and `UnmarshalFullDescriptor` return the decoded
//! message and its `Result` alongside the `*check.Error`. Here a refused
//! decode returns the error alone, with the report on it
//! (`PxfError::check_error`); the validator has seen the message. That is
//! the shape decided on #126.

use std::any::Any;
use std::cell::RefCell;
use std::fmt;

use prost_reflect::{DescriptorPool, DynamicMessage, MessageDescriptor};
use protowire_check::{BoxError, Report, Validator, Violation};
use protowire_pxf::{unmarshal, unmarshal_full, UnmarshalOptions};

const TEST_FDS: &[u8] = include_bytes!("../testdata/test.binpb");

fn desc() -> MessageDescriptor {
    DescriptorPool::decode(TEST_FDS)
        .unwrap()
        .get_message_by_name("test.v1.AllTypes")
        .unwrap()
}

#[derive(Debug, PartialEq)]
struct EngineError;

impl fmt::Display for EngineError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("engine failure")
    }
}

impl std::error::Error for EngineError {}

/// Returns a fixed report, or fails, and records the `string_field` of the
/// message it was given.
struct Fake {
    rep: Report,
    fail: bool,
    got: RefCell<Option<String>>,
}

impl Fake {
    fn new(rep: Report) -> Self {
        Fake {
            rep,
            fail: false,
            got: RefCell::new(None),
        }
    }
}

impl Validator for Fake {
    fn validate(&self, decoded: &dyn Any) -> Result<Report, BoxError> {
        let msg = decoded
            .downcast_ref::<DynamicMessage>()
            .expect("the validator receives the DynamicMessage");
        let name = msg.get_field_by_name("string_field").unwrap();
        *self.got.borrow_mut() = Some(name.as_str().unwrap().to_string());
        if self.fail {
            return Err(Box::new(EngineError));
        }
        Ok(self.rep.clone())
    }
}

fn violation_report() -> Report {
    Report {
        violations: vec![Violation {
            path: "port".into(),
            rule_id: "test.rule".into(),
            message: "port out of range".into(),
        }],
    }
}

fn with(v: &dyn Validator) -> UnmarshalOptions<'_> {
    UnmarshalOptions {
        validator: Some(v),
        ..Default::default()
    }
}

const DOC: &str = r#"string_field = "x""#;

#[test]
fn unmarshal_runs_validator() {
    let fv = Fake::new(violation_report());
    let err = unmarshal(DOC, &desc(), with(&fv)).unwrap_err();
    let ce = err.check_error().expect("a check error");
    assert_eq!(ce.report.violations[0].path, "port");
    assert_eq!(
        err.to_string(),
        "check: port: port out of range (test.rule)"
    );
    assert!(!err.has_position());
    // The validator sees the decoded message, fully populated.
    assert_eq!(fv.got.borrow().as_deref(), Some("x"));
}

#[test]
fn unmarshal_clean_validator() {
    let fv = Fake::new(Report::default());
    let msg = unmarshal(DOC, &desc(), with(&fv)).expect("decodes");
    assert_eq!(
        msg.get_field_by_name("string_field").unwrap().as_str(),
        Some("x")
    );
    assert!(fv.got.borrow().is_some());
}

#[test]
fn unmarshal_validator_engine_error() {
    let fv = Fake {
        fail: true,
        ..Fake::new(Report::default())
    };
    let err = unmarshal(DOC, &desc(), with(&fv)).unwrap_err();
    assert_eq!(err.to_string(), "engine failure");
    let source = std::error::Error::source(&err).expect("the engine's error");
    assert_eq!(source.downcast_ref::<EngineError>(), Some(&EngineError));
    assert!(err.check_error().is_none());
}

/// Go's `TestUnmarshalFullAttachesReport` and
/// `TestUnmarshalFullDescriptorReturnsMessageOnViolation`: the report of a
/// refused full decode is on its error.
#[test]
fn unmarshal_full_report_on_violation() {
    let rep = violation_report();
    let err = unmarshal_full(DOC, &desc(), with(&Fake::new(rep.clone()))).unwrap_err();
    assert_eq!(err.check_error().map(|e| &e.report), Some(&rep));
}

#[test]
fn unmarshal_full_clean_report_attached() {
    let (_, presence) = unmarshal_full(DOC, &desc(), with(&Fake::new(Report::default()))).unwrap();
    assert_eq!(presence.report(), Some(&Report::default()));
    assert!(presence.is_set("string_field"));
}

#[test]
fn unmarshal_full_no_validator_no_report() {
    let (_, presence) = unmarshal_full(DOC, &desc(), UnmarshalOptions::default()).unwrap();
    assert_eq!(presence.report(), None);
}

/// In a full decode the validator runs after the post-decode pass, as in
/// Go: it sees the default the pass applied.
#[test]
fn validator_sees_the_post_decode_pass() {
    const ROW_FDS: &[u8] = include_bytes!("../testdata/scan-options-test.binpb");
    let row = DescriptorPool::decode(ROW_FDS)
        .unwrap()
        .get_message_by_name("so.v1.Row")
        .unwrap();
    let seen = RefCell::new(None);
    let v = |d: &dyn Any| -> Result<Report, BoxError> {
        let m = d.downcast_ref::<DynamicMessage>().unwrap();
        let dflt = m.get_field_by_name("dflt").unwrap();
        *seen.borrow_mut() = Some(dflt.as_str().unwrap().to_string());
        Ok(Report::default())
    };
    unmarshal_full(r#"id = "x" req = 1"#, &row, with(&v)).expect("decodes");
    assert_eq!(seen.borrow().as_deref(), Some("z"));
}

/// The validator does not run when the decode fails.
#[test]
fn validator_runs_only_on_success() {
    let fv = Fake::new(violation_report());
    let err = unmarshal(r#"string_field = 1"#, &desc(), with(&fv)).unwrap_err();
    assert!(err.has_position(), "{err}");
    assert!(err.check_error().is_none());
    assert!(
        fv.got.borrow().is_none(),
        "the validator ran on a failed decode"
    );
}
