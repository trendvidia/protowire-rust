// SPDX-License-Identifier: MIT
// Copyright (c) 2026 TrendVidia, LLC.
//! `UnmarshalOptions::validator`: the validator cases of protowire-go's
//! `encoding/pb/options_test.go`, ported.

use std::any::Any;
use std::cell::Cell;
use std::fmt;

use protowire_check::{BoxError, Report, Validator, Violation};
use protowire_pb::wire::{Reader, Result, WireType, Writer};
use protowire_pb::{marshal, Error, Message, UnmarshalOptions};

/// Go's `valMsg {Name string 1; Port int32 2}`.
#[derive(Debug, Default, PartialEq)]
struct ValMsg {
    name: String,
    port: i32,
}

impl Message for ValMsg {
    fn encode_to(&self, w: &mut Writer) {
        if !self.name.is_empty() {
            w.tag(1, WireType::LengthDelimited);
            w.string(&self.name);
        }
        if self.port != 0 {
            w.tag(2, WireType::Varint);
            w.varint(self.port as i64 as u64);
        }
    }
    fn merge_field(&mut self, num: u32, wt: WireType, r: &mut Reader<'_>) -> Result<()> {
        match num {
            1 => self.name = r.string()?,
            2 => self.port = r.varint()? as i32,
            _ => r.skip(wt)?,
        }
        Ok(())
    }
}

#[derive(Debug, PartialEq)]
struct EngineError;

impl fmt::Display for EngineError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("engine failure")
    }
}

impl std::error::Error for EngineError {}

/// Returns a fixed report, or fails, and records the port of the message
/// it was given.
#[derive(Default)]
struct Fake {
    rep: Report,
    fail: bool,
    got: Cell<Option<i32>>,
}

impl Validator for Fake {
    fn validate(&self, decoded: &dyn Any) -> std::result::Result<Report, BoxError> {
        let msg = decoded
            .downcast_ref::<ValMsg>()
            .expect("the validator receives the caller's message type");
        self.got.set(Some(msg.port));
        if self.fail {
            return Err(Box::new(EngineError));
        }
        Ok(self.rep.clone())
    }
}

fn with(v: &dyn Validator) -> UnmarshalOptions<'_> {
    UnmarshalOptions {
        validator: Some(v),
        ..Default::default()
    }
}

#[test]
fn unmarshal_options_validator_violation() {
    let data = marshal(&ValMsg {
        name: "x".into(),
        port: 99999,
    })
    .unwrap();
    let rep = Report {
        violations: vec![Violation {
            path: "port".into(),
            rule_id: "test.range".into(),
            message: "port out of range".into(),
        }],
    };
    let fv = Fake {
        rep: rep.clone(),
        ..Default::default()
    };
    let err = with(&fv).unmarshal::<ValMsg>(&data).unwrap_err();
    match &err {
        Error::Check(ce) => assert_eq!(ce.report, rep),
        other => panic!("want Error::Check, got {other:?}"),
    }
    assert_eq!(
        err.to_string(),
        "check: port: port out of range (test.range)"
    );
    // The validator saw the decoded message, fully populated.
    assert_eq!(fv.got.get(), Some(99999));
}

#[test]
fn unmarshal_options_clean_pass() {
    let data = marshal(&ValMsg {
        name: "x".into(),
        port: 0,
    })
    .unwrap();
    let got: ValMsg = with(&Fake::default()).unmarshal(&data).unwrap();
    assert_eq!(got.name, "x");
}

#[test]
fn unmarshal_options_engine_error() {
    let data = marshal(&ValMsg {
        name: "x".into(),
        port: 0,
    })
    .unwrap();
    let fv = Fake {
        fail: true,
        ..Default::default()
    };
    let err = with(&fv).unmarshal::<ValMsg>(&data).unwrap_err();
    match &err {
        Error::Validator(e) => assert_eq!(e.downcast_ref::<EngineError>(), Some(&EngineError)),
        other => panic!("want Error::Validator, got {other:?}"),
    }
    assert_eq!(err.to_string(), "engine failure");
}

#[test]
fn unmarshal_options_zero_value() {
    let data = marshal(&ValMsg {
        name: "x".into(),
        port: 0,
    })
    .unwrap();
    let got: ValMsg = UnmarshalOptions::default().unmarshal(&data).unwrap();
    assert_eq!(got.name, "x");
}

/// The options' limits apply, and a failed decode never reaches the
/// validator.
#[test]
fn limits_apply_before_the_validator() {
    let data = marshal(&ValMsg {
        name: "x".into(),
        port: 7,
    })
    .unwrap();
    let fv = Fake::default();
    let mut o = with(&fv);
    o.limits.max_message_size = 2;
    let err = o.unmarshal::<ValMsg>(&data).unwrap_err();
    assert!(matches!(err, Error::MessageTooLarge { .. }), "{err}");
    assert_eq!(fv.got.get(), None);
}
