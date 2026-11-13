// SPDX-License-Identifier: MIT
// Copyright (c) 2026 TrendVidia, LLC.
//! Envelope streams (#128): protowire-go's `envelope.Encoder` and
//! `envelope.Decoder`, which frame envelopes as pb frames any message. A
//! port of its `envelope/stream_test.go`, and `testdata/go-envelope-stream.bin`,
//! which protowire-go's encoder wrote (`scripts/gen-stream-oracle.sh`).

use std::io::Cursor;

use protowire_envelope::{Decoder, Encoder, Envelope, StreamError};

const GO_STREAM: &[u8] = include_bytes!("../testdata/go-envelope-stream.bin");

/// The envelopes `scripts/stream-oracle/main.go` writes.
fn go_envelopes() -> Vec<Envelope> {
    vec![
        Envelope::ok(200, b"hi".to_vec()),
        Envelope::err(400, "E_BAD", "bad %s", vec!["x".to_string()]),
        Envelope::transport_err("connection reset"),
    ]
}

fn uvarint(mut v: u64) -> Vec<u8> {
    let mut out = Vec::new();
    while v >= 0x80 {
        out.push((v as u8) | 0x80);
        v >>= 7;
    }
    out.push(v as u8);
    out
}

#[test]
fn reads_and_writes_what_go_writes() {
    let mut dec = Decoder::new(Cursor::new(GO_STREAM));
    let mut got = Vec::new();
    let mut env = Envelope::default();
    while dec.decode(&mut env).expect("decode") {
        got.push(env.clone());
    }
    assert_eq!(got, go_envelopes());

    let mut enc = Encoder::new(Vec::new());
    for env in go_envelopes() {
        enc.encode(&env).unwrap();
    }
    assert_eq!(enc.into_inner(), GO_STREAM);
}

/// `TestStreamRoundTrip`, with its `withDetails`.
#[test]
fn round_trip() {
    let mut detailed = Envelope::err(400, "INVALID_ARG", "bad input", vec!["field=name".into()]);
    detailed
        .error
        .as_mut()
        .unwrap()
        .with_field("name", "REQUIRED", "name is required", vec![])
        .with_meta("trace_id", "abc123");
    let envs = vec![
        Envelope::ok(200, vec![0xde, 0xad, 0xbe, 0xef]),
        detailed,
        Envelope::transport_err("connection refused"),
    ];
    let mut enc = Encoder::new(Vec::new());
    for env in &envs {
        enc.encode(env).unwrap();
    }
    let mut dec = Decoder::new(Cursor::new(enc.into_inner()));
    for want in &envs {
        let mut got = Envelope::default();
        assert!(dec.decode(&mut got).unwrap());
        assert_eq!(&got, want);
    }
    assert!(!dec.decode(&mut Envelope::default()).unwrap());
}

/// `TestStreamCleanEOF`.
#[test]
fn an_empty_stream_ends_cleanly() {
    let mut dec = Decoder::new(Cursor::new(Vec::new()));
    assert!(!dec.decode(&mut Envelope::default()).unwrap());
}

/// `TestStreamPartialBody`.
#[test]
fn a_body_cut_short_is_an_unexpected_end() {
    let mut input = uvarint(50);
    input.extend_from_slice(&[0; 5]);
    let err = Decoder::new(Cursor::new(input))
        .decode(&mut Envelope::default())
        .unwrap_err();
    assert!(matches!(err, StreamError::UnexpectedEof), "{err:?}");
}

/// `TestStreamOversized`: pb's frame cap and error.
#[test]
fn a_frame_past_the_cap_is_corrupt_framing() {
    let mut dec = Decoder::new(Cursor::new(uvarint(1 << 30)));
    dec.set_max_frame_size(1 << 16);
    let err = dec.decode(&mut Envelope::default()).unwrap_err();
    assert!(err.is_framing_corrupt(), "{err:?}");
    assert_eq!(
        err.to_string(),
        "pb: stream framing corrupt: frame size 1073741824 exceeds max 65536"
    );
}
