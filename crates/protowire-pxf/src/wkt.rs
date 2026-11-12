// SPDX-License-Identifier: MIT
// Copyright (c) 2026 TrendVidia, LLC.
//! Well-known type shapes (draft -01 §wkt-shapes): a message is handled as
//! the well-known type its full name says only when it declares the fields
//! that handling reads. One that does not is an ordinary message: bind-time
//! validation reports it ([`crate::ViolationKind::WellKnownShape`]), and the
//! paths that do not validate read and write it as a block. Mirrors
//! protowire-go's `wktShapes` and `hasKnownShape` (protowire-go#224, #120).

use prost_reflect::{Kind, MessageDescriptor};
use prost_types::field_descriptor_proto::{Label, Type};

/// One field a codec reads on a well-known type.
struct WktField {
    name: &'static str,
    kind: GoKind,
    repeated: bool,
    /// May be undeclared; when declared it must still match.
    optional: bool,
}

/// The scalar kinds the shapes name, spelled as protowire-go's detail text
/// spells them.
#[derive(Clone, Copy, PartialEq, Eq)]
enum GoKind {
    Bool,
    Int32,
    Int64,
    Uint32,
    Uint64,
    Float,
    Double,
    String,
    Bytes,
}

impl GoKind {
    fn name(self) -> &'static str {
        match self {
            GoKind::Bool => "bool",
            GoKind::Int32 => "int32",
            GoKind::Int64 => "int64",
            GoKind::Uint32 => "uint32",
            GoKind::Uint64 => "uint64",
            GoKind::Float => "float",
            GoKind::Double => "double",
            GoKind::String => "string",
            GoKind::Bytes => "bytes",
        }
    }

    fn matches(self, ty: Type) -> bool {
        matches!(
            (self, ty),
            (GoKind::Bool, Type::Bool)
                | (GoKind::Int32, Type::Int32)
                | (GoKind::Int64, Type::Int64)
                | (GoKind::Uint32, Type::Uint32)
                | (GoKind::Uint64, Type::Uint64)
                | (GoKind::Float, Type::Float)
                | (GoKind::Double, Type::Double)
                | (GoKind::String, Type::String)
                | (GoKind::Bytes, Type::Bytes)
        )
    }
}

const fn field(name: &'static str, kind: GoKind) -> WktField {
    WktField {
        name,
        kind,
        repeated: false,
        optional: false,
    }
}

const SECONDS_NANOS: &[WktField] = &[
    field("seconds", GoKind::Int64),
    field("nanos", GoKind::Int32),
];

/// The nine wrappers: one singular `value` of the wrapped kind.
const BOOL_VALUE: &[WktField] = &[field("value", GoKind::Bool)];
const BYTES_VALUE: &[WktField] = &[field("value", GoKind::Bytes)];
const DOUBLE_VALUE: &[WktField] = &[field("value", GoKind::Double)];
const FLOAT_VALUE: &[WktField] = &[field("value", GoKind::Float)];
const INT32_VALUE: &[WktField] = &[field("value", GoKind::Int32)];
const INT64_VALUE: &[WktField] = &[field("value", GoKind::Int64)];
const STRING_VALUE: &[WktField] = &[field("value", GoKind::String)];
const UINT32_VALUE: &[WktField] = &[field("value", GoKind::Uint32)];
const UINT64_VALUE: &[WktField] = &[field("value", GoKind::Uint64)];

const fn optional(name: &'static str, kind: GoKind) -> WktField {
    WktField {
        name,
        kind,
        repeated: false,
        optional: true,
    }
}

const ANY: &[WktField] = &[
    field("type_url", GoKind::String),
    field("value", GoKind::Bytes),
];
const FIELD_MASK: &[WktField] = &[WktField {
    name: "paths",
    kind: GoKind::String,
    repeated: true,
    optional: false,
}];
const BIG_INT: &[WktField] = &[field("abs", GoKind::Bytes), field("negative", GoKind::Bool)];
const DECIMAL: &[WktField] = &[
    field("unscaled", GoKind::Bytes),
    field("scale", GoKind::Int32),
    field("negative", GoKind::Bool),
];
const BIG_FLOAT: &[WktField] = &[
    field("mantissa", GoKind::Bytes),
    field("exponent", GoKind::Int32),
    field("prec", GoKind::Uint32),
    field("negative", GoKind::Bool),
];
const SECRET: &[WktField] = &[
    field("value", GoKind::String),
    optional("hint", GoKind::String),
    optional("fingerprint", GoKind::String),
];

/// The fields a message must declare to be treated as the well-known type
/// its full name says it is, or `None` for a name that needs none. Field
/// numbers are not in it, since PXF binds fields by name. The canonical
/// definition of each type has this shape, and so does any copy of it.
fn shape(full: &str) -> Option<&'static [WktField]> {
    Some(match full {
        "google.protobuf.Timestamp" | "google.protobuf.Duration" => SECONDS_NANOS,
        "google.protobuf.Any" => ANY,
        "google.protobuf.FieldMask" => FIELD_MASK,
        "pxf.BigInt" => BIG_INT,
        "pxf.Decimal" => DECIMAL,
        "pxf.BigFloat" => BIG_FLOAT,
        "pxf.Secret" => SECRET,
        "google.protobuf.BoolValue" => BOOL_VALUE,
        "google.protobuf.BytesValue" => BYTES_VALUE,
        "google.protobuf.DoubleValue" => DOUBLE_VALUE,
        "google.protobuf.FloatValue" => FLOAT_VALUE,
        "google.protobuf.Int32Value" => INT32_VALUE,
        "google.protobuf.Int64Value" => INT64_VALUE,
        "google.protobuf.StringValue" => STRING_VALUE,
        "google.protobuf.UInt32Value" => UINT32_VALUE,
        "google.protobuf.UInt64Value" => UINT64_VALUE,
        _ => return None,
    })
}

/// protowire-go's spelling of a field's kind (`protoreflect.Kind.String`).
fn go_type_name(ty: Type) -> &'static str {
    match ty {
        Type::Double => "double",
        Type::Float => "float",
        Type::Int64 => "int64",
        Type::Uint64 => "uint64",
        Type::Int32 => "int32",
        Type::Fixed64 => "fixed64",
        Type::Fixed32 => "fixed32",
        Type::Bool => "bool",
        Type::String => "string",
        Type::Group => "group",
        Type::Message => "message",
        Type::Bytes => "bytes",
        Type::Uint32 => "uint32",
        Type::Enum => "enum",
        Type::Sfixed32 => "sfixed32",
        Type::Sfixed64 => "sfixed64",
        Type::Sint32 => "sint32",
        Type::Sint64 => "sint64",
    }
}

/// What stops a message having the shape its full name needs.
enum Problem {
    Missing(&'static str),
    Kind {
        name: &'static str,
        got: Type,
        want: GoKind,
    },
    Cardinality(&'static str),
}

/// protowire-go's `wktShapeProblem`, without its text. It runs for every
/// value of a well-known type, so it reads the field list as declared
/// rather than looking each field up by name: on a document of timestamps
/// alone, the lookups cost a quarter of the decode. The shapes name scalar
/// kinds only, so a map field, whose kind is message, fails on its kind
/// before its cardinality is looked at, as in protowire-go.
fn problem(desc: &MessageDescriptor) -> Option<Problem> {
    let fields = &desc.descriptor_proto().field;
    for f in shape(desc.full_name())? {
        let Some(fd) = fields.iter().find(|fd| fd.name() == f.name) else {
            if f.optional {
                continue;
            }
            return Some(Problem::Missing(f.name));
        };
        // A descriptor may leave `type` out of a field that names its
        // type by `type_name`, a message or an enum; the pool resolved it.
        let got = match fd.r#type {
            Some(_) => fd.r#type(),
            None => match desc.get_field_by_name(f.name).map(|fd| fd.kind()) {
                Some(Kind::Enum(_)) => Type::Enum,
                _ => Type::Message,
            },
        };
        if !f.kind.matches(got) {
            return Some(Problem::Kind {
                name: f.name,
                got,
                want: f.kind,
            });
        }
        if (fd.label() == Label::Repeated) != f.repeated {
            return Some(Problem::Cardinality(f.name));
        }
    }
    None
}

/// What stops `desc` having the shape its full name needs, or `None` when
/// it has it or the name needs none, in protowire-go's words.
pub(crate) fn shape_problem(desc: &MessageDescriptor) -> Option<String> {
    Some(match problem(desc)? {
        Problem::Missing(name) => format!("it has no field \"{name}\""),
        Problem::Kind { name, got, want } => format!(
            "field \"{name}\" is {}, and must be {}",
            go_type_name(got),
            want.name()
        ),
        Problem::Cardinality(name) => format!("field \"{name}\" has the wrong cardinality"),
    })
}

/// The name a codec dispatches on: `desc`'s full name, or `""` for a message
/// that has a well-known type's name without its shape, so that it matches
/// none of the names and is handled as the ordinary message it is.
pub(crate) fn dispatch_name(desc: &MessageDescriptor) -> &str {
    if problem(desc).is_some() {
        ""
    } else {
        desc.full_name()
    }
}
