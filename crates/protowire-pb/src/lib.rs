// SPDX-License-Identifier: MIT
// Copyright (c) 2026 TrendVidia, LLC.
//! Schema-free protobuf binary codec.
//!
//! Port of `github.com/trendvidia/protowire/encoding/pb` and the TS port's
//! `pb` module. Two layers:
//!
//! - [`wire`]: low-level primitives (varint, zigzag, fixed widths, tags,
//!   length-delimited blobs, skip-on-unknown).
//! - [`codec`]: the [`codec::Message`] trait + nested-message helpers.
//! - [`big`]: `pxf.BigInt`, `pxf.Decimal` and `pxf.BigFloat` fields, written
//!   as protowire-go writes `*big.Int`, `*big.Rat` and `*big.Float`.
//!
//! Idiomatic Rust differs from the Go (struct tags via reflection) and TS
//! (runtime field schemas) ports: each Rust message implements `Message`
//! by hand. The wire bytes match all five ports per
//! `protowire/scripts/cross_envelope_check.sh`.

pub mod big;
pub mod codec;
pub mod wire;

pub use big::{write_big_float, write_big_int, write_decimal, BigFloat, BigInt, Decimal};
pub use codec::{
    marshal, marshal_with, read_message, unmarshal, unmarshal_with, write_map_entry, write_message,
    MapEntryField, MarshalOptions, Message,
};
pub use wire::{
    Error, Limits, Reader, Result, WireType, Writer, MAX_BIG_FLOAT_EXPONENT, MAX_MESSAGE_SIZE,
    MAX_NESTING_DEPTH, MAX_NUMERIC_LITERAL_DIGITS, MAX_REPEATED_COUNT,
};
