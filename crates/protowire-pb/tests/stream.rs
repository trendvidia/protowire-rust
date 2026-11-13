// SPDX-License-Identifier: MIT
// Copyright (c) 2026 TrendVidia, LLC.
//! pb message streams (#128): the protobuf-delimited framing protowire-go's
//! `pb.Encoder` and `pb.Decoder` read and write. A port of its
//! `stream_test.go`, with the stream decoder cases of `limits_size_test.go`
//! and `utf8_test.go`, and `testdata/go-pb-stream.bin`, which protowire-go's
//! encoder wrote (`scripts/gen-stream-oracle.sh`): it reads as the messages
//! Go wrote, and this port writes it byte for byte.

use std::io::{self, Cursor, Read};

use protowire_pb::wire::{Limits, Reader, Result, WireType, Writer};
use protowire_pb::{Decoder, Encoder, Message, StreamError, UnmarshalOptions};

const GO_STREAM: &[u8] = include_bytes!("../testdata/go-pb-stream.bin");

/// protowire-go's `StreamMsg`: `N int`, `Text string`, `Data []byte` at 1, 2
/// and 3.
#[derive(Debug, Clone, Default, PartialEq)]
struct StreamMsg {
    n: i64,
    text: String,
    data: Vec<u8>,
}

impl Message for StreamMsg {
    fn encode_to(&self, w: &mut Writer) {
        if self.n != 0 {
            w.tag(1, WireType::Varint);
            w.varint(self.n as u64);
        }
        if !self.text.is_empty() {
            w.tag(2, WireType::LengthDelimited);
            w.string(&self.text);
        }
        if !self.data.is_empty() {
            w.tag(3, WireType::LengthDelimited);
            w.bytes(&self.data);
        }
    }

    fn merge_field(&mut self, num: u32, wt: WireType, r: &mut Reader<'_>) -> Result<()> {
        match num {
            1 => self.n = r.varint()? as i64,
            2 => self.text = r.string()?,
            3 => self.data = r.bytes()?,
            _ => r.skip(wt)?,
        }
        Ok(())
    }
}

fn msg(n: i64, text: &str, data: Vec<u8>) -> StreamMsg {
    StreamMsg {
        n,
        text: text.to_string(),
        data,
    }
}

/// The messages `scripts/stream-oracle/main.go` writes.
fn go_messages() -> Vec<StreamMsg> {
    vec![
        msg(1, "small", vec![]),
        msg(2, &"medium ".repeat(128), vec![]),
        msg(3, "", vec![0xab; 20000]),
        StreamMsg::default(),
        msg(-5, "neg", vec![]),
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
    let mut m = StreamMsg::default();
    while dec.decode(&mut m).expect("decode") {
        got.push(m.clone());
    }
    assert_eq!(got, go_messages());

    let mut enc = Encoder::new(Vec::new());
    for m in go_messages() {
        enc.encode(&m).unwrap();
    }
    assert_eq!(enc.into_inner(), GO_STREAM);
}

/// `TestStreamRoundTrip`.
#[test]
fn round_trip() {
    let msgs = vec![
        msg(1, "small", vec![]),
        msg(2, &"medium ".repeat(128), vec![]),
        msg(3, "", vec![0xab; 1 << 20]),
    ];
    let mut enc = Encoder::new(Vec::new());
    for m in &msgs {
        enc.encode(m).unwrap();
    }
    let buf = enc.into_inner();
    let mut dec = Decoder::new(Cursor::new(buf));
    for want in &msgs {
        let mut got = StreamMsg::default();
        assert!(dec.decode(&mut got).unwrap());
        assert_eq!(&got, want);
    }
    assert!(!dec.decode(&mut StreamMsg::default()).unwrap());
}

/// `TestStreamCleanEOF`.
#[test]
fn an_empty_stream_ends_cleanly() {
    let mut dec = Decoder::new(Cursor::new(Vec::new()));
    assert!(!dec.decode(&mut StreamMsg::default()).unwrap());
    // The end is not an error, and stays the end.
    assert!(!dec.decode(&mut StreamMsg::default()).unwrap());
}

/// `TestStreamPartialPrefix` and `TestStreamPartialBody`: the stream ends
/// inside a frame, which every later call reports again.
#[test]
fn a_frame_cut_short_is_an_unexpected_end_that_sticks() {
    let mut body = uvarint(100);
    body.extend_from_slice(&[0; 10]);
    for input in [vec![0x80], body] {
        let mut dec = Decoder::new(Cursor::new(input.clone()));
        let err = dec.decode(&mut StreamMsg::default()).unwrap_err();
        assert!(
            matches!(err, StreamError::UnexpectedEof),
            "{input:?}: {err:?}"
        );
        assert_eq!(err.to_string(), "unexpected EOF");
        let again = dec.decode(&mut StreamMsg::default()).unwrap_err();
        assert!(matches!(again, StreamError::UnexpectedEof), "not sticky");
    }
}

/// `TestStreamOversized`.
#[test]
fn a_frame_past_the_cap_is_corrupt_framing_that_sticks() {
    let mut dec = Decoder::new(Cursor::new(uvarint(1 << 30)));
    dec.set_max_frame_size(1 << 20);
    let err = dec.decode(&mut StreamMsg::default()).unwrap_err();
    assert!(err.is_framing_corrupt(), "{err:?}");
    assert_eq!(
        err.to_string(),
        "pb: stream framing corrupt: frame size 1073741824 exceeds max 1048576"
    );
    let again = dec.decode(&mut StreamMsg::default()).unwrap_err();
    assert!(again.is_framing_corrupt(), "not sticky");
    assert_eq!(again.to_string(), err.to_string());
}

/// `TestStreamVarintOverflow`.
#[test]
fn a_length_past_ten_bytes_is_corrupt_framing() {
    let mut dec = Decoder::new(Cursor::new(vec![0xff; 11]));
    let err = dec.decode(&mut StreamMsg::default()).unwrap_err();
    assert!(err.is_framing_corrupt(), "{err:?}");
    assert_eq!(
        err.to_string(),
        "pb: stream framing corrupt: binary: varint overflows a 64-bit integer"
    );
    // A tenth byte above 1 overflows 64 bits too.
    let mut ten = vec![0xff; 9];
    ten.push(0x02);
    let err = Decoder::new(Cursor::new(ten))
        .decode(&mut StreamMsg::default())
        .unwrap_err();
    assert_eq!(
        err.to_string(),
        "pb: stream framing corrupt: binary: varint overflows a 64-bit integer"
    );
}

/// A frame of exactly the cap reads; one byte more is corrupt framing, as
/// protowire-go compares (`n > max`).
#[test]
fn the_cap_is_the_largest_frame_read() {
    let mut enc = Encoder::new(Vec::new());
    enc.encode(&msg(0, "", vec![7; 100])).unwrap();
    let framed = enc.into_inner();
    let body = framed.len() - 1; // a one-byte length
    let mut dec = Decoder::new(Cursor::new(framed.clone()));
    dec.set_max_frame_size(body);
    assert!(dec.decode(&mut StreamMsg::default()).unwrap(), "at the cap");
    let mut dec = Decoder::new(Cursor::new(framed));
    dec.set_max_frame_size(body - 1);
    let err = dec.decode(&mut StreamMsg::default()).unwrap_err();
    assert_eq!(
        err.to_string(),
        format!(
            "pb: stream framing corrupt: frame size {body} exceeds max {}",
            body - 1
        )
    );
}

/// `TestStreamInnerUnmarshalRecoverable`: a frame whose body does not decode
/// leaves the framing in step, and the next frame reads.
#[test]
fn a_body_that_does_not_decode_leaves_the_stream_readable() {
    let good = msg(7, "ok", vec![]);
    let mut enc = Encoder::new(Vec::new());
    enc.encode(&good).unwrap();
    let mut buf = enc.into_inner();
    buf.extend(uvarint(4));
    buf.extend_from_slice(&[0xff; 4]);
    let mut enc = Encoder::new(buf);
    enc.encode(&good).unwrap();
    let buf = enc.into_inner();

    let mut dec = Decoder::new(Cursor::new(buf));
    let mut got = StreamMsg::default();
    assert!(dec.decode(&mut got).unwrap());
    assert_eq!(got, good);
    let err = dec.decode(&mut StreamMsg::default()).unwrap_err();
    assert!(matches!(err, StreamError::Decode(_)), "{err:?}");
    assert!(!err.is_framing_corrupt());
    let mut third = StreamMsg::default();
    assert!(
        dec.decode(&mut third).unwrap(),
        "frame 3 after a body error"
    );
    assert_eq!(third, good);
    assert!(!dec.decode(&mut StreamMsg::default()).unwrap());
}

/// `TestStreamLengthBoundaries`: lengths at each varint width.
#[test]
fn every_length_width_round_trips() {
    for size in [0usize, 1, 127, 128, 16383, 16384, 1 << 17] {
        let m = msg(0, "", vec![0xcc; size]);
        let mut enc = Encoder::new(Vec::new());
        enc.encode(&m).unwrap();
        let mut got = StreamMsg::default();
        assert!(Decoder::new(Cursor::new(enc.into_inner()))
            .decode(&mut got)
            .unwrap());
        assert_eq!(got.data.len(), size);
        assert_eq!(got, m, "size={size}");
    }
}

/// `FuzzStreamDecode`'s body, over its seeds and every prefix of the Go
/// stream: decoding never panics, and stops at the first error.
#[test]
fn any_input_decodes_or_fails() {
    let mut inputs = vec![vec![0x00], vec![0x03, 0x08, 0x2a, 0x00]];
    inputs.extend((0..GO_STREAM.len().min(1200)).map(|i| GO_STREAM[..i].to_vec()));
    for input in inputs {
        let mut dec = Decoder::new(Cursor::new(input));
        dec.set_max_frame_size(1 << 20);
        for _ in 0..64 {
            match dec.decode(&mut StreamMsg::default()) {
                Ok(true) => {}
                Ok(false) | Err(_) => break,
            }
        }
    }
}

/// The decoder's options bound each frame's body: a frame inside the frame
/// cap, past the call's `max_message_size` (protowire-go's
/// `TestMaxMessageSize` stream case).
#[test]
fn the_options_limits_bound_a_frame() {
    let mut enc = Encoder::new(Vec::new());
    enc.encode(&msg(0, "", vec![0; 32])).unwrap();
    let framed = enc.into_inner();
    assert!(Decoder::new(Cursor::new(framed.clone()))
        .decode(&mut StreamMsg::default())
        .unwrap());
    let options = UnmarshalOptions {
        limits: Limits {
            max_message_size: 16,
            ..Limits::default()
        },
        ..UnmarshalOptions::default()
    };
    let err = Decoder::with_options(Cursor::new(framed), options)
        .decode(&mut StreamMsg::default())
        .unwrap_err();
    assert!(matches!(err, StreamError::Decode(_)), "{err:?}");
    assert!(err.to_string().contains("MaxMessageSize=16"), "{err}");
}

/// protowire-go's `utf8_test.go` stream case: a frame whose string is not
/// UTF-8 is refused, and the next frame reads.
#[test]
fn a_frame_with_invalid_utf8_is_refused_and_the_next_reads() {
    let mut stream = Vec::new();
    for text in [b"AB\xff".as_slice(), b"good"] {
        let mut body = vec![0x12, text.len() as u8];
        body.extend_from_slice(text);
        stream.extend(uvarint(body.len() as u64));
        stream.extend(body);
    }
    let mut dec = Decoder::new(Cursor::new(stream));
    let err = dec.decode(&mut StreamMsg::default()).unwrap_err();
    assert!(matches!(err, StreamError::Decode(_)), "{err:?}");
    let mut next = StreamMsg::default();
    assert!(dec.decode(&mut next).unwrap());
    assert_eq!(next.text, "good");
    assert!(!dec.decode(&mut StreamMsg::default()).unwrap());
}

/// A reader that fails after `ok` bytes.
struct Failing {
    data: Vec<u8>,
    ok: usize,
    at: usize,
}

impl Read for Failing {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        if self.at >= self.ok {
            return Err(io::Error::other("boom"));
        }
        let n = buf
            .len()
            .min(self.ok - self.at)
            .min(self.data.len() - self.at);
        buf[..n].copy_from_slice(&self.data[self.at..self.at + n]);
        self.at += n;
        Ok(n)
    }
}

/// A read error inside a length is corrupt framing, as protowire-go wraps
/// it; inside a body it is the reader's own error. Both stick.
#[test]
fn a_read_error_sticks() {
    let mut enc = Encoder::new(Vec::new());
    enc.encode(&msg(0, "", vec![1; 300])).unwrap();
    let data = enc.into_inner();
    let mut dec = Decoder::new(Failing {
        data: data.clone(),
        ok: 1,
        at: 0,
    });
    let err = dec.decode(&mut StreamMsg::default()).unwrap_err();
    assert!(err.is_framing_corrupt(), "{err:?}");
    assert_eq!(err.to_string(), "pb: stream framing corrupt: boom");
    assert!(dec
        .decode(&mut StreamMsg::default())
        .unwrap_err()
        .is_framing_corrupt());

    let mut dec = Decoder::new(Failing {
        data,
        ok: 10,
        at: 0,
    });
    let err = dec.decode(&mut StreamMsg::default()).unwrap_err();
    assert!(matches!(err, StreamError::Io(_)), "{err:?}");
    assert_eq!(err.to_string(), "boom");
    let again = dec.decode(&mut StreamMsg::default()).unwrap_err();
    assert!(matches!(again, StreamError::Io(_)), "not sticky: {again:?}");
}
