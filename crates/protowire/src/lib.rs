//! Protowire — schema-driven binary and text codecs (envelope, pb, pxf, sbe).
//!
//! Umbrella crate that re-exports the four sub-crates so downstream users
//! can depend on `protowire` alone.

pub use protowire_envelope as envelope;
pub use protowire_pb as pb;
pub use protowire_pxf as pxf;
pub use protowire_sbe as sbe;
