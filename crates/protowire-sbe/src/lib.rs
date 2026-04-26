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
pub mod saxlite;
pub mod template;
pub mod unmarshal;
pub mod view;
pub mod prototoxml;
pub mod xmlschema;
pub mod xmltoproto;

pub use codec::{Codec, GROUP_HEADER_SIZE, HEADER_SIZE};
pub use errors::SbeError;
pub use marshal::marshal;
pub use template::{
    build_template, field_encoding_size, FieldTemplate, GroupTemplate, MessageTemplate,
    SbeEncoding,
};
pub use unmarshal::unmarshal;
pub use view::{GroupView, View};
pub use xmlschema::{
    camel_to_screaming_snake, camel_to_snake, parse_xml_schema, screaming_snake_to_pascal,
    singular_pascal, snake_to_camel, strip_enum_prefix, XmlComposite, XmlEnum, XmlField,
    XmlGroup, XmlMessage, XmlRef, XmlSchema, XmlType, XmlTypes, XmlValidValue,
};
pub use prototoxml::proto_to_xml;
pub use xmltoproto::xml_to_proto;
