// SPDX-License-Identifier: MIT
// Copyright (c) 2026 TrendVidia, LLC.
//! Protowire — schema-driven binary and text codecs (envelope, pb, pxf, sbe).
//!
//! Umbrella crate that re-exports the sub-crates so downstream users can
//! depend on `protowire` alone: the four codecs, and [`check`], the
//! validator hook their decoders share.

pub use protowire_check as check;
pub use protowire_envelope as envelope;
pub use protowire_pb as pb;
pub use protowire_pxf as pxf;
pub use protowire_sbe as sbe;
