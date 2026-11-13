// SPDX-License-Identifier: MIT
// Copyright (c) 2026 TrendVidia, LLC.
//! SBE message streams (#128), framed by the Simple Open Framing Header as
//! protowire-go's `Codec.NewEncoder` and `Codec.NewDecoder` frame them. A
//! port of its `encoding/sbe/stream_test.go` and of the stream case of its
//! `validator_test.go`, and `testdata/go-sbe-stream.bin`, which
//! protowire-go's encoder wrote (`scripts/gen-stream-oracle.sh`).

use std::any::Any;
use std::io::Cursor;
use std::sync::Arc;

use prost_reflect::{DescriptorPool, DynamicMessage, MessageDescriptor, Value};
use protowire_check::{BoxError, Report, Validator, Violation};
use protowire_sbe::{Codec, StreamError};

const SBE_FDS: &[u8] = include_bytes!("../testdata/sbe-test.binpb");
const GO_STREAM: &[u8] = include_bytes!("../testdata/go-sbe-stream.bin");

fn codec() -> Codec {
    let p = DescriptorPool::decode(SBE_FDS).unwrap();
    Codec::from_files(&[p.get_file_by_name("sbe-test.proto").unwrap()]).unwrap()
}

fn simple() -> MessageDescriptor {
    DescriptorPool::decode(SBE_FDS)
        .unwrap()
        .get_message_by_name("test.v1.Simple")
        .expect("Simple")
}

fn new_simple(id: u32, value: i32) -> DynamicMessage {
    let mut m = DynamicMessage::new(simple());
    m.set_field_by_name("id", Value::U32(id));
    m.set_field_by_name("value", Value::I32(value));
    m
}

fn fields(m: &DynamicMessage) -> (u32, i32) {
    (
        m.get_field_by_name("id").unwrap().as_u32().unwrap(),
        m.get_field_by_name("value").unwrap().as_i32().unwrap(),
    )
}

/// A SOFH header: total length, then the encoding type.
fn hdr(total: u32, enc: u16) -> Vec<u8> {
    let mut h = total.to_be_bytes().to_vec();
    h.extend_from_slice(&enc.to_be_bytes());
    h
}

/// The messages `scripts/stream-oracle/main.go` writes, as in Go's
/// `TestStreamRoundTrip`.
const GO_MESSAGES: [(u32, i32); 3] = [(1, -100), (2, 0), (1 << 20, 2147483647)];

#[test]
fn reads_and_writes_what_go_writes() {
    let codec = codec();
    let mut dec = codec.new_decoder(Cursor::new(GO_STREAM));
    let mut got = Vec::new();
    let mut m = DynamicMessage::new(simple());
    while dec.decode(&mut m).expect("decode") {
        got.push(fields(&m));
    }
    assert_eq!(got, GO_MESSAGES);

    let mut enc = codec.new_encoder(Vec::new());
    for (id, value) in GO_MESSAGES {
        enc.encode(&new_simple(id, value)).unwrap();
    }
    assert_eq!(enc.into_inner(), GO_STREAM);
}

/// `TestStreamRoundTrip`, then the clean end.
#[test]
fn round_trip() {
    let codec = codec();
    let mut enc = codec.new_encoder(Vec::new());
    for (id, value) in GO_MESSAGES {
        enc.encode(&new_simple(id, value)).unwrap();
    }
    let mut dec = codec.new_decoder(Cursor::new(enc.into_inner()));
    for want in GO_MESSAGES {
        let mut got = DynamicMessage::new(simple());
        assert!(dec.decode(&mut got).unwrap());
        assert_eq!(fields(&got), want);
    }
    assert!(!dec.decode(&mut DynamicMessage::new(simple())).unwrap());
}

/// `TestStreamCleanEOF`.
#[test]
fn an_empty_stream_ends_cleanly() {
    let codec = codec();
    let mut dec = codec.new_decoder(Cursor::new(Vec::new()));
    assert!(!dec.decode(&mut DynamicMessage::new(simple())).unwrap());
}

/// `TestStreamPartialHeader` and `TestStreamPartialBody`: the stream ends
/// inside a frame, which every later call reports again.
#[test]
fn a_frame_cut_short_is_an_unexpected_end_that_sticks() {
    let mut body = hdr(100, 0xEB50);
    body.extend_from_slice(&[0; 10]);
    for input in [vec![0, 0, 0], body] {
        let codec = codec();
        let mut dec = codec.new_decoder(Cursor::new(input.clone()));
        let err = dec.decode(&mut DynamicMessage::new(simple())).unwrap_err();
        assert!(
            matches!(err, StreamError::UnexpectedEof),
            "{input:?}: {err:?}"
        );
        assert_eq!(err.to_string(), "unexpected EOF");
        let again = dec.decode(&mut DynamicMessage::new(simple())).unwrap_err();
        assert!(matches!(again, StreamError::UnexpectedEof), "not sticky");
    }
}

/// `TestStreamWrongEncodingType`, `TestStreamLengthBelowHeader` and
/// `TestStreamOversized`: corrupt framing, with protowire-go's texts, which
/// every later call reports again.
#[test]
fn corrupt_framing_sticks() {
    for (input, max, text) in [
        (
            hdr(6, 0x5BE0),
            None,
            "sbe: stream framing corrupt: SOFH encoding type 0x5be0 not supported",
        ),
        (
            hdr(4, 0xEB50),
            None,
            "sbe: stream framing corrupt: SOFH total length 4 < header size",
        ),
        (
            hdr(6 + (1 << 30), 0xEB50),
            Some(1 << 16),
            "sbe: stream framing corrupt: frame size 1073741824 exceeds max 65536",
        ),
    ] {
        let codec = codec();
        let mut dec = codec.new_decoder(Cursor::new(input));
        if let Some(max) = max {
            dec.set_max_frame_size(max);
        }
        let err = dec.decode(&mut DynamicMessage::new(simple())).unwrap_err();
        assert!(err.is_framing_corrupt(), "{err:?}");
        assert_eq!(err.to_string(), text);
        let again = dec.decode(&mut DynamicMessage::new(simple())).unwrap_err();
        assert!(again.is_framing_corrupt(), "not sticky");
        assert_eq!(again.to_string(), text);
    }
}

/// `FuzzStreamDecode`'s body over its seeds and every prefix of the Go
/// stream: decoding never panics, and stops at the first error.
#[test]
fn any_input_decodes_or_fails() {
    let codec = codec();
    let mut inputs = vec![
        vec![0, 0, 0, 6, 0xEB, 0x50],
        vec![
            0, 0, 0, 16, 0xEB, 0x50, 0, 8, 0, 2, 0, 1, 1, 0, 0, 0, 0xff, 0xff, 0xff, 0xfe,
        ],
    ];
    inputs.extend((0..=GO_STREAM.len()).map(|i| GO_STREAM[..i].to_vec()));
    for input in inputs {
        let mut dec = codec.new_decoder(Cursor::new(input));
        dec.set_max_frame_size(1 << 20);
        for _ in 0..64 {
            match dec.decode(&mut DynamicMessage::new(simple())) {
                Ok(true) => {}
                Ok(false) | Err(_) => break,
            }
        }
    }
}

struct Refuse;

impl Validator for Refuse {
    fn validate(&self, _: &dyn Any) -> Result<Report, BoxError> {
        Ok(Report {
            violations: vec![Violation {
                path: "value".into(),
                rule_id: "test.rule".into(),
                message: "value out of range".into(),
            }],
        })
    }
}

/// `TestStreamDecoderRunsValidator`: the codec's validator runs on each
/// frame, and a refusal consumes the frame without breaking the stream.
#[test]
fn the_decoder_runs_the_codecs_validator() {
    let plain = codec();
    let mut enc = plain.new_encoder(Vec::new());
    enc.encode(&new_simple(7, 0)).unwrap();
    let framed = enc.into_inner();

    let codec = codec().with_validator(Arc::new(Refuse));
    let mut dec = codec.new_decoder(Cursor::new(framed));
    let mut m = DynamicMessage::new(simple());
    let err = dec.decode(&mut m).unwrap_err();
    let StreamError::Decode(e) = &err else {
        panic!("want a decode error, got {err:?}");
    };
    assert!(e.check_error().is_some(), "{e}");
    assert!(!err.is_framing_corrupt());
    assert!(
        !dec.decode(&mut m).unwrap(),
        "the end of the stream follows"
    );
}
