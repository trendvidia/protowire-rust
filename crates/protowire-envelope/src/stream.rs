// SPDX-License-Identifier: MIT
// Copyright (c) 2026 TrendVidia, LLC.
//! Envelope streams: length-prefixed [`Envelope`]s, as
//! [`protowire_pb::stream`] frames any message. A port of protowire-go's
//! `envelope/stream.go`, which reuses `pb.Encoder` so framing has one place
//! to be fixed (#128).

use std::io::{Read, Write};

pub use protowire_pb::stream::{StreamError, DEFAULT_MAX_FRAME_SIZE};

use crate::Envelope;

/// Writes length-prefixed envelopes. protowire-go's `envelope.Encoder`.
pub struct Encoder<W: Write>(protowire_pb::Encoder<W>);

impl<W: Write> Encoder<W> {
    pub fn new(w: W) -> Self {
        Self(protowire_pb::Encoder::new(w))
    }

    /// Write one envelope as a frame.
    pub fn encode(&mut self, env: &Envelope) -> Result<(), StreamError> {
        self.0.encode(env)
    }

    /// The writer, consuming the encoder.
    pub fn into_inner(self) -> W {
        self.0.into_inner()
    }
}

/// Reads length-prefixed envelopes, with pb's frame cap and errors.
/// protowire-go's `envelope.Decoder`.
pub struct Decoder<R: Read>(protowire_pb::Decoder<'static, R>);

impl<R: Read> Decoder<R> {
    pub fn new(r: R) -> Self {
        Self(protowire_pb::Decoder::new(r))
    }

    /// See [`protowire_pb::Decoder::set_max_frame_size`].
    pub fn set_max_frame_size(&mut self, n: usize) {
        self.0.set_max_frame_size(n);
    }

    /// Read the next frame into `env`: true when one was read, false at the
    /// end of the stream. See [`protowire_pb::Decoder::decode`].
    pub fn decode(&mut self, env: &mut Envelope) -> Result<bool, StreamError> {
        self.0.decode(env)
    }

    /// The reader, consuming the decoder.
    pub fn into_inner(self) -> R {
        self.0.into_inner()
    }
}
