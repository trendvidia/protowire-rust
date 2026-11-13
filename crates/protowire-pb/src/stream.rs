// SPDX-License-Identifier: MIT
// Copyright (c) 2026 TrendVidia, LLC.
//! Message streams: length-prefixed pb messages, the protobuf-delimited
//! framing (a varint length, then the body). A port of protowire-go's
//! `encoding/pb/stream.go`, so a Rust program reads the streams a Go program
//! writes, and the other way round (#128).

use std::fmt;
use std::io::{self, Read, Write};

use crate::codec::{marshal_with, MarshalOptions, Message, UnmarshalOptions};
use crate::wire::Error;

/// The largest frame a [`Decoder`] reads by default: 16 MiB. A stream faces
/// an adversary per connection; [`Decoder::set_max_frame_size`] changes it.
/// It is below `MaxMessageSize`, which still bounds a frame's body through
/// the decoder's [`UnmarshalOptions`] when the cap is raised. protowire-go's
/// `DefaultMaxFrameSize`.
pub const DEFAULT_MAX_FRAME_SIZE: usize = 16 << 20;

/// An error from a stream [`Encoder`] or [`Decoder`].
///
/// [`StreamError::FramingCorrupt`], [`StreamError::UnexpectedEof`] and
/// [`StreamError::Io`] leave the stream unreadable, so the decoder returns
/// the same error from then on. [`StreamError::Decode`] does not: its frame
/// was read whole, and the next call reads the next one.
#[derive(Debug)]
pub enum StreamError {
    /// The framing is invalid: a corrupt length varint, or a frame larger
    /// than the decoder's cap. protowire-go's `ErrFramingCorrupt`; the text
    /// says what was wrong.
    FramingCorrupt(String),
    /// The stream ended inside a frame.
    UnexpectedEof,
    /// The reader or writer failed.
    Io(io::Error),
    /// A frame was read, and its body did not decode, or the decoder's
    /// validator refused it; or a message did not encode.
    Decode(Error),
}

impl StreamError {
    /// Whether the framing is corrupt, as `errors.Is(err,
    /// pb.ErrFramingCorrupt)` asks in protowire-go.
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
            // Never kept: a body that does not decode leaves the stream
            // readable.
            StreamError::Decode(_) => unreachable!("a decode error is not sticky"),
        }
    }
}

/// protowire-go's texts: `pb: stream framing corrupt: <what>`, `unexpected
/// EOF`, and the reader's or the decoder's own error.
impl fmt::Display for StreamError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            StreamError::FramingCorrupt(m) => write!(f, "pb: stream framing corrupt: {m}"),
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

/// Writes length-prefixed messages: each is its varint length, then its pb
/// encoding. protowire-go's `pb.Encoder`.
pub struct Encoder<W: Write> {
    w: W,
    options: MarshalOptions,
}

impl<W: Write> Encoder<W> {
    /// An encoder writing to `w` under the default [`MarshalOptions`].
    pub fn new(w: W) -> Self {
        Self::with_options(w, MarshalOptions::default())
    }

    /// An encoder writing each message as [`marshal_with`] does under
    /// `options` (protowire-go's `MarshalOptions.NewEncoder`).
    pub fn with_options(w: W, options: MarshalOptions) -> Self {
        Self { w, options }
    }

    /// Write one message as a frame. A message that does not encode writes
    /// nothing.
    pub fn encode<M: Message>(&mut self, msg: &M) -> Result<(), StreamError> {
        let body = marshal_with(msg, self.options).map_err(StreamError::Decode)?;
        let mut hdr = [0u8; 10];
        let n = put_uvarint(&mut hdr, body.len() as u64);
        self.w.write_all(&hdr[..n]).map_err(StreamError::Io)?;
        self.w.write_all(&body).map_err(StreamError::Io)
    }

    /// The writer, for a caller that wraps one to flush it.
    pub fn get_mut(&mut self) -> &mut W {
        &mut self.w
    }

    /// The writer, consuming the encoder.
    pub fn into_inner(self) -> W {
        self.w
    }
}

/// Reads length-prefixed messages, as [`Encoder`] writes them. protowire-go's
/// `pb.Decoder`.
///
/// The decoder reads exactly a frame's bytes from `r` per call, never
/// ahead, so it reads a byte at a time while it reads a length: give it a
/// buffered reader (`std::io::BufReader`) when `r` is not one.
pub struct Decoder<'a, R: Read> {
    r: R,
    buf: Vec<u8>,
    max: usize,
    err: Option<StreamError>,
    options: UnmarshalOptions<'a>,
}

impl<R: Read> Decoder<'static, R> {
    /// A decoder reading from `r` under the default [`UnmarshalOptions`].
    pub fn new(r: R) -> Self {
        Decoder::with_options(r, UnmarshalOptions::default())
    }
}

impl<'a, R: Read> Decoder<'a, R> {
    /// A decoder whose frames decode under `options`, their limits and
    /// validator (protowire-go's `UnmarshalOptions.NewDecoder`).
    pub fn with_options(r: R, options: UnmarshalOptions<'a>) -> Self {
        Self {
            r,
            buf: Vec::new(),
            max: DEFAULT_MAX_FRAME_SIZE,
            err: None,
            options,
        }
    }

    /// Refuse a frame longer than `n` bytes as corrupt framing; the default
    /// is [`DEFAULT_MAX_FRAME_SIZE`].
    pub fn set_max_frame_size(&mut self, n: usize) {
        self.max = n;
    }

    /// Read the next frame into `msg`: true when one was read, false at the
    /// end of the stream, between frames.
    ///
    /// A frame whose body does not decode is a [`StreamError::Decode`], and
    /// the next call reads the next frame. Any other error leaves the stream
    /// unreadable, and every later call returns it again.
    pub fn decode<M: Message + 'static>(&mut self, msg: &mut M) -> Result<bool, StreamError> {
        if let Some(e) = &self.err {
            return Err(e.again());
        }
        let n = match read_uvarint(&mut self.r) {
            Ok(None) => return Ok(false),
            Ok(Some(n)) => n,
            Err(e) => return Err(self.stick(e)),
        };
        if n > self.max as u64 {
            return Err(self.stick(StreamError::FramingCorrupt(format!(
                "frame size {n} exceeds max {}",
                self.max
            ))));
        }
        // Within the cap, so it fits.
        let n = n as usize;
        self.buf.resize(n, 0);
        if let Err(e) = self.r.read_exact(&mut self.buf) {
            let e = if e.kind() == io::ErrorKind::UnexpectedEof {
                StreamError::UnexpectedEof
            } else {
                StreamError::Io(e)
            };
            return Err(self.stick(e));
        }
        *msg = self
            .options
            .unmarshal(&self.buf)
            .map_err(StreamError::Decode)?;
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

fn put_uvarint(buf: &mut [u8; 10], mut v: u64) -> usize {
    let mut i = 0;
    while v >= 0x80 {
        buf[i] = (v as u8) | 0x80;
        v >>= 7;
        i += 1;
    }
    buf[i] = v as u8;
    i + 1
}

/// One byte, or `None` at the end of the stream; an interrupted read is
/// retried.
fn read_byte<R: Read>(r: &mut R) -> io::Result<Option<u8>> {
    let mut b = [0u8];
    loop {
        match r.read(&mut b) {
            Ok(0) => return Ok(None),
            Ok(_) => return Ok(Some(b[0])),
            Err(e) if e.kind() == io::ErrorKind::Interrupted => continue,
            Err(e) => return Err(e),
        }
    }
}

/// A frame's length, as Go's `binary.ReadUvarint` reads it: `None` at the end
/// of the stream before its first byte, an unexpected end after it, and
/// corrupt framing past ten bytes or 64 bits. A read error is corrupt
/// framing too, as protowire-go wraps it.
fn read_uvarint<R: Read>(r: &mut R) -> Result<Option<u64>, StreamError> {
    let (mut x, mut s) = (0u64, 0u32);
    for i in 0..10 {
        let b = match read_byte(r) {
            Ok(Some(b)) => b,
            Ok(None) if i == 0 => return Ok(None),
            Ok(None) => return Err(StreamError::UnexpectedEof),
            Err(e) => return Err(StreamError::FramingCorrupt(e.to_string())),
        };
        if b < 0x80 {
            if i == 9 && b > 1 {
                return Err(overflow());
            }
            return Ok(Some(x | (b as u64) << s));
        }
        x |= ((b & 0x7f) as u64) << s;
        s += 7;
    }
    Err(overflow())
}

/// Go's `binary.ReadUvarint` overflow, as protowire-go wraps it.
fn overflow() -> StreamError {
    StreamError::FramingCorrupt("binary: varint overflows a 64-bit integer".to_string())
}
