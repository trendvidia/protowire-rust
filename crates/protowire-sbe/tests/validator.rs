// SPDX-License-Identifier: MIT
// Copyright (c) 2026 TrendVidia, LLC.
//! `Codec::with_validator`: protowire-go's `encoding/sbe/validator_test.go`,
//! ported. Its `UnmarshalDescriptor` case is `unmarshal` here, the one
//! decode entry point. Its stream-decoder case,
//! `TestStreamDecoderRunsValidator`, waits for the stream decoder (#128).

use std::any::Any;
use std::fmt;
use std::sync::{Arc, Mutex};

use prost_reflect::{DescriptorPool, DynamicMessage, MessageDescriptor, Value};
use protowire_check::{BoxError, Report, Validator, Violation};
use protowire_sbe::{marshal, unmarshal, Codec};

const SBE_FDS: &[u8] = include_bytes!("../testdata/sbe-test.binpb");

fn plain_codec() -> Codec {
    let p = DescriptorPool::decode(SBE_FDS).unwrap();
    Codec::from_files(&[p.get_file_by_name("sbe-test.proto").unwrap()]).unwrap()
}

fn simple() -> MessageDescriptor {
    DescriptorPool::decode(SBE_FDS)
        .unwrap()
        .get_message_by_name("test.v1.Simple")
        .expect("Simple")
}

/// A `Simple {id, value}`, encoded with a plain codec.
fn marshal_simple(id: u32, value: i32) -> Vec<u8> {
    let mut msg = DynamicMessage::new(simple());
    msg.set_field_by_name("id", Value::U32(id));
    msg.set_field_by_name("value", Value::I32(value));
    marshal(&plain_codec(), &msg).unwrap()
}

#[derive(Debug, PartialEq)]
struct EngineError;

impl fmt::Display for EngineError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("engine failure")
    }
}

impl std::error::Error for EngineError {}

/// Returns a fixed report, or fails, and records the address of the
/// message it was given.
#[derive(Default)]
struct Fake {
    rep: Report,
    fail: bool,
    got: Mutex<Option<usize>>,
}

impl Validator for Fake {
    fn validate(&self, decoded: &dyn Any) -> Result<Report, BoxError> {
        let msg = decoded
            .downcast_ref::<DynamicMessage>()
            .expect("the validator receives the DynamicMessage");
        *self.got.lock().unwrap() = Some(msg as *const DynamicMessage as usize);
        if self.fail {
            return Err(Box::new(EngineError));
        }
        Ok(self.rep.clone())
    }
}

fn violation_report() -> Report {
    Report {
        violations: vec![Violation {
            path: "value".into(),
            rule_id: "test.rule".into(),
            message: "value out of range".into(),
        }],
    }
}

#[test]
fn codec_validator_violation() {
    let data = marshal_simple(42, -100);
    let fv = Arc::new(Fake {
        rep: violation_report(),
        ..Default::default()
    });
    let codec = plain_codec().with_validator(fv.clone());

    let mut msg = DynamicMessage::new(simple());
    let err = unmarshal(&codec, &mut msg, &data).unwrap_err();
    let ce = err.check_error().expect("a check error");
    assert_eq!(ce.report.violations[0].path, "value");
    assert_eq!(
        err.to_string(),
        "check: value: value out of range (test.rule)"
    );
    // The validator sees the decoded message itself, fully populated.
    assert_eq!(
        *fv.got.lock().unwrap(),
        Some(&msg as *const DynamicMessage as usize)
    );
    assert_eq!(
        msg.get_field_by_name("value").as_deref(),
        Some(&Value::I32(-100))
    );
}

#[test]
fn codec_validator_clean_pass() {
    let data = marshal_simple(42, -100);
    let codec = plain_codec().with_validator(Arc::new(Fake::default()));
    let mut msg = DynamicMessage::new(simple());
    unmarshal(&codec, &mut msg, &data).expect("decodes");
}

#[test]
fn codec_validator_engine_error() {
    let data = marshal_simple(42, -100);
    let codec = plain_codec().with_validator(Arc::new(Fake {
        fail: true,
        ..Default::default()
    }));
    let mut msg = DynamicMessage::new(simple());
    let err = unmarshal(&codec, &mut msg, &data).unwrap_err();
    let source = std::error::Error::source(&err).expect("the engine's error");
    assert_eq!(source.downcast_ref::<EngineError>(), Some(&EngineError));
    assert!(err.check_error().is_none());
}

/// A closure is a validator, and a failed decode never reaches it.
#[test]
fn validator_runs_only_on_success() {
    let ran = Arc::new(Mutex::new(false));
    let seen = ran.clone();
    let codec = plain_codec().with_validator(Arc::new(move |_: &dyn Any| {
        *seen.lock().unwrap() = true;
        Ok::<_, BoxError>(violation_report())
    }));
    let mut msg = DynamicMessage::new(simple());
    let err = unmarshal(&codec, &mut msg, &[0u8; 3]).unwrap_err();
    assert!(err.check_error().is_none(), "{err}");
    assert!(!*ran.lock().unwrap());
}

/// A view reads the wire buffer without building a message, so the
/// validator does not apply to it, as in Go.
#[test]
fn view_is_not_validated() {
    let data = marshal_simple(42, -100);
    let codec = plain_codec().with_validator(Arc::new(Fake {
        rep: violation_report(),
        ..Default::default()
    }));
    let view = codec.view(&data).expect("a view");
    assert_eq!(view.int_field("value"), Ok(-100));
}
