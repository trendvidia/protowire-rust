//! FIX SBE (Simple Binary Encoding) codec.
//!
//! Port of `github.com/trendvidia/protowire/encoding/sbe`.
//!
//! Slice A: annotations + template + Codec (descriptor → wire layout).
//! Slices B–D land marshal / unmarshal, View / GroupView, and the XML
//! schema converters.

pub mod annotations;
pub mod codec;
pub mod errors;
pub mod marshal;
pub mod template;
pub mod unmarshal;
pub mod view;

pub use codec::{Codec, GROUP_HEADER_SIZE, HEADER_SIZE};
pub use errors::SbeError;
pub use marshal::marshal;
pub use template::{
    build_template, field_encoding_size, FieldTemplate, GroupTemplate, MessageTemplate,
    SbeEncoding,
};
pub use unmarshal::unmarshal;
pub use view::{GroupView, View};
