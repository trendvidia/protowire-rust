// SPDX-License-Identifier: MIT
// Copyright (c) 2026 TrendVidia, LLC.
//! protowire-go's `check/check_test.go`, ported.

use std::any::Any;
use std::cell::RefCell;
use std::fmt;

use protowire_check::{validate, BoxError, Error, Report, Validator, Violation};

/// A validator that returns a fixed report or error and records what it
/// was given.
struct FakeValidator {
    rep: Option<Report>,
    err: Option<&'static str>,
    got: RefCell<Option<i32>>,
}

#[derive(Debug, PartialEq)]
struct EngineError(&'static str);

impl fmt::Display for EngineError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.0)
    }
}

impl std::error::Error for EngineError {}

impl Validator for FakeValidator {
    fn validate(&self, decoded: &dyn Any) -> Result<Report, BoxError> {
        *self.got.borrow_mut() = decoded.downcast_ref::<i32>().copied();
        if let Some(e) = self.err {
            return Err(Box::new(EngineError(e)));
        }
        Ok(self.rep.clone().unwrap_or_default())
    }
}

fn fake(rep: Option<Report>, err: Option<&'static str>) -> FakeValidator {
    FakeValidator {
        rep,
        err,
        got: RefCell::new(None),
    }
}

fn violation(path: &str, rule_id: &str, message: &str) -> Violation {
    Violation {
        path: path.into(),
        rule_id: rule_id.into(),
        message: message.into(),
    }
}

#[test]
fn report_ok() {
    assert!(Report::default().ok());
    assert!(!Report {
        violations: vec![violation("", "", "bad")]
    }
    .ok());
}

#[test]
fn error_message() {
    // Go's "nil report" and "empty report" are one case here: an Error
    // always holds a Report.
    let cases: [(&str, Vec<Violation>, &str); 4] = [
        ("empty report", vec![], "check: validation failed"),
        (
            "single full violation",
            vec![violation("port", "rfc001.required", "value is required")],
            "check: port: value is required (rfc001.required)",
        ),
        (
            "message-level violation without path or rule",
            vec![violation("", "", "inconsistent")],
            "check: inconsistent",
        ),
        (
            "multiple violations",
            vec![violation("a", "", "first"), violation("b", "", "second")],
            "check: 2 violations; first: a: first",
        ),
    ];
    for (name, violations, want) in cases {
        let e = Error {
            report: Report { violations },
        };
        assert_eq!(e.to_string(), want, "{name}");
    }
}

#[test]
fn validate_nil_validator() {
    assert!(matches!(validate(None, &"anything"), Ok(None)));
}

#[test]
fn validate_engine_error() {
    let fv = fake(None, Some("engine exploded"));
    let err = validate(Some(&fv), &42i32).unwrap_err();
    assert_eq!(
        err.downcast_ref::<EngineError>(),
        Some(&EngineError("engine exploded"))
    );
    assert!(
        err.downcast_ref::<Error>().is_none(),
        "engine errors must not be check::Error"
    );
    assert_eq!(*fv.got.borrow(), Some(42));
}

#[test]
fn validate_violations() {
    let rep = Report {
        violations: vec![violation("x", "", "bad")],
    };
    let err = validate(Some(&fake(Some(rep.clone()), None)), &()).unwrap_err();
    let ce = err.downcast_ref::<Error>().expect("a check::Error");
    assert_eq!(ce.report, rep);
}

#[test]
fn validate_clean_pass() {
    let got = validate(Some(&fake(Some(Report::default()), None)), &()).unwrap();
    assert_eq!(got, Some(Report::default()));
}

#[test]
fn a_closure_is_a_validator() {
    let deny = |_: &dyn Any| -> Result<Report, BoxError> {
        Ok(Report {
            violations: vec![violation("id", "test.deny", "denied")],
        })
    };
    let err = validate(Some(&deny), &()).unwrap_err();
    assert_eq!(err.to_string(), "check: id: denied (test.deny)");
}
