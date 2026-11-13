// SPDX-License-Identifier: MIT
// Copyright (c) 2026 TrendVidia, LLC.
//! SBE message streams, framed by the Simple Open Framing Header: a 6-byte
//! big-endian header (total length, then the encoding type 0xEB50 for
//! little-endian SBE), then the message. A port of protowire-go's
//! `encoding/sbe/stream.go`, so a Rust program reads the streams a Go
//! program writes, and the other way round (#128).

use std::fmt;
use std::io::{self, Read, Write};

use prost_reflect::DynamicMessage;

use crate::codec::Codec;
use crate::errors::SbeError;
use crate::marshal::marshal;
use crate::unmarshal::unmarshal;

/// SOFH's encoding type for little-endian SBE.
const SOFH_ENCODING_SBE_LE: u16 = 0xEB50;
const SOFH_HEADER_LEN: usize = 6;

/// The largest frame body a [`Decoder`] reads by default: 16 MiB.
/// protowire-go's `DefaultMaxFrameSize`.
pub const DEFAULT_MAX_FRAME_SIZE: usize = 16 << 20;

/// An error from a stream [`Encoder`] or [`Decoder`].
///
/// [`StreamError::FramingCorrupt`], [`StreamError::UnexpectedEof`] and
/// [`StreamError::Io`] leave the stream unreadable, so the decoder returns
/// the same error from then on. [`StreamError::Decode`] does not: its frame
/// was read whole, and the next call reads the next one.
#[derive(Debug)]
pub enum StreamError {
    /// The framing is invalid: an encoding type other than SBE's, a total
    /// length below the header's, or a frame larger than the decoder's cap.
    /// protowire-go's `ErrFramingCorrupt`; the text says what was wrong.
    FramingCorrupt(String),
    /// The stream ended inside a frame.
    UnexpectedEof,
    /// The reader or writer failed.
    Io(io::Error),
    /// A frame was read, and its message did not decode, or the codec's
    /// validator refused it; or a message did not encode.
    Decode(SbeError),
}

impl StreamError {
    /// Whether the framing is corrupt, as `errors.Is(err,
    /// sbe.ErrFramingCorrupt)` asks in protowire-go.
    pub fn is_framing_corrupt(&self) -> bool {
        matches!(self, StreamError::FramingCorrupt(_))
    }

    /// The same error again, for the decoder to return once its stream is
    /// unreadable.
    fn again(&self) -> StreamError {
        match self {
            StreamError::FramingCorrupt(m) => StreamError::FramingCorrupt(m.clone()),
            StreamError::UnexpectedEof => StreamError::UnexpectedEof,
            StreamError::Io(e) => StreamError::Io(io::Error::new(e.kind(), e.to_string())),
            StreamError::Decode(_) => unreachable!("a decode error is not sticky"),
        }
    }
}

/// protowire-go's texts: `sbe: stream framing corrupt: <what>`, `unexpected
/// EOF`, and the reader's or the codec's own error.
impl fmt::Display for StreamError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            StreamError::FramingCorrupt(m) => write!(f, "sbe: stream framing corrupt: {m}"),
            StreamError::UnexpectedEof => f.write_str("unexpected EOF"),
            StreamError::Io(e) => e.fmt(f),
            StreamError::Decode(e) => e.fmt(f),
        }
    }
}

impl std::error::Error for StreamError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            StreamError::Io(e) => Some(e),
            StreamError::Decode(e) => Some(e),
            _ => None,
        }
    }
}

impl Codec {
    /// An encoder writing this codec's messages to `w`, one SOFH frame each
    /// (protowire-go's `Codec.NewEncoder`).
    pub fn new_encoder<W: Write>(&self, w: W) -> Encoder<'_, W> {
        Encoder { codec: self, w }
    }

    /// A decoder reading SOFH frames from `r` with this codec, its limits
    /// and validator (protowire-go's `Codec.NewDecoder`).
    pub fn new_decoder<R: Read>(&self, r: R) -> Decoder<'_, R> {
        Decoder {
            codec: self,
            r,
            buf: Vec::new(),
            max: DEFAULT_MAX_FRAME_SIZE,
            err: None,
        }
    }
}

/// Writes SOFH-framed SBE messages. protowire-go's `sbe.Encoder`.
pub struct Encoder<'c, W: Write> {
    codec: &'c Codec,
    w: W,
}

impl<W: Write> Encoder<'_, W> {
    /// Write one message as a frame. A message that does not encode writes
    /// nothing.
    pub fn encode(&mut self, msg: &DynamicMessage) -> Result<(), StreamError> {
        let body = marshal(self.codec, msg).map_err(StreamError::Decode)?;
        let total = (SOFH_HEADER_LEN + body.len()) as u32;
        let mut hdr = [0u8; SOFH_HEADER_LEN];
        hdr[0..4].copy_from_slice(&total.to_be_bytes());
        hdr[4..6].copy_from_slice(&SOFH_ENCODING_SBE_LE.to_be_bytes());
        self.w.write_all(&hdr).map_err(StreamError::Io)?;
        self.w.write_all(&body).map_err(StreamError::Io)
    }

    /// The writer, consuming the encoder.
    pub fn into_inner(self) -> W {
        self.w
    }
}

/// Reads SOFH-framed SBE messages, as [`Encoder`] writes them.
/// protowire-go's `sbe.Decoder`.
pub struct Decoder<'c, R: Read> {
    codec: &'c Codec,
    r: R,
    buf: Vec<u8>,
    max: usize,
    err: Option<StreamError>,
}

impl<R: Read> Decoder<'_, R> {
    /// Refuse a frame whose body is longer than `n` bytes as corrupt
    /// framing; the default is [`DEFAULT_MAX_FRAME_SIZE`].
    pub fn set_max_frame_size(&mut self, n: usize) {
        self.max = n;
    }

    /// Read the next frame into `msg`: true when one was read, false at the
    /// end of the stream, between frames.
    ///
    /// A frame whose message does not decode, or that the codec's validator
    /// refuses, is a [`StreamError::Decode`], and the next call reads the
    /// next frame. Any other error leaves the stream unreadable, and every
    /// later call returns it again.
    pub fn decode(&mut self, msg: &mut DynamicMessage) -> Result<bool, StreamError> {
        if let Some(e) = &self.err {
            return Err(e.again());
        }
        let mut hdr = [0u8; SOFH_HEADER_LEN];
        match read_full(&mut self.r, &mut hdr) {
            Ok(n) if n == SOFH_HEADER_LEN => {}
            // The end of the stream, between frames.
            Ok(0) => return Ok(false),
            // A header cut short, or a read that failed inside one: the
            // stream is unreadable, as protowire-go reports it.
            Ok(_) | Err(_) => return Err(self.stick(StreamError::UnexpectedEof)),
        }
        let total = u32::from_be_bytes([hdr[0], hdr[1], hdr[2], hdr[3]]);
        let enc = u16::from_be_bytes([hdr[4], hdr[5]]);
        if enc != SOFH_ENCODING_SBE_LE {
            return Err(self.stick(StreamError::FramingCorrupt(format!(
                "SOFH encoding type 0x{enc:04x} not supported"
            ))));
        }
        if (total as usize) < SOFH_HEADER_LEN {
            return Err(self.stick(StreamError::FramingCorrupt(format!(
                "SOFH total length {total} < header size"
            ))));
        }
        let body_len = total as usize - SOFH_HEADER_LEN;
        if body_len > self.max {
            return Err(self.stick(StreamError::FramingCorrupt(format!(
                "frame size {body_len} exceeds max {}",
                self.max
            ))));
        }
        self.buf.resize(body_len, 0);
        if let Err(e) = self.r.read_exact(&mut self.buf) {
            let e = if e.kind() == io::ErrorKind::UnexpectedEof {
                StreamError::UnexpectedEof
            } else {
                StreamError::Io(e)
            };
            return Err(self.stick(e));
        }
        unmarshal(self.codec, msg, &self.buf).map_err(StreamError::Decode)?;
        Ok(true)
    }

    fn stick(&mut self, e: StreamError) -> StreamError {
        let again = e.again();
        self.err = Some(e);
        again
    }

    /// The reader, consuming the decoder.
    pub fn into_inner(self) -> R {
        self.r
    }
}

/// Fill `buf` from `r`, as Go's `io.ReadFull`: the bytes read before the end
/// of the stream, or the error a read gave.
fn read_full<R: Read>(r: &mut R, buf: &mut [u8]) -> io::Result<usize> {
    let mut n = 0;
    while n < buf.len() {
        match r.read(&mut buf[n..]) {
            Ok(0) => break,
            Ok(k) => n += k,
            Err(e) if e.kind() == io::ErrorKind::Interrupted => {}
            Err(e) => return Err(e),
        }
    }
    Ok(n)
}
