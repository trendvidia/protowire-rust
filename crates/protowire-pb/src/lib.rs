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
//!
//! Idiomatic Rust differs from the Go (struct tags via reflection) and TS
//! (runtime field schemas) ports: each Rust message implements `Message`
//! by hand. The wire bytes match all five ports per
//! `protowire/scripts/cross_envelope_check.sh`.

pub mod codec;
pub mod wire;

pub use codec::{
    marshal, read_message, unmarshal, unmarshal_with, write_map_entry, write_message,
    MapEntryField, Message,
};
pub use wire::{
    Error, Limits, Reader, Result, WireType, Writer, MAX_MESSAGE_SIZE, MAX_NESTING_DEPTH,
    MAX_REPEATED_COUNT,
};
