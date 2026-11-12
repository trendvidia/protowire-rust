// SPDX-License-Identifier: MIT
// Copyright (c) 2026 TrendVidia, LLC.
//! Convert an SBE XML schema into proto3 source with `sbe.*` annotations.
//! Mirrors `protowire/encoding/sbe/xmltoproto.go` and the TS port's
//! `sbe/xmltoproto.ts`.

use std::collections::HashMap;
use std::fmt::Write as _;

use crate::errors::SbeError;
use crate::xmlschema::{
    camel_to_screaming_snake, camel_to_snake, parse_xml_schema, singular_pascal, XmlComposite,
    XmlEnum, XmlField, XmlGroup, XmlMessage, XmlSchema, XmlType,
};

const BUILTINS: &[&str] = &[
    "int8", "int16", "int32", "int64", "uint8", "uint16", "uint32", "uint64", "float", "double",
    "char",
];

/// Parse `xml` and emit proto3 source covering the SBE schema's enums,
/// composites, and template messages with the appropriate `sbe.*` options.
/// A malformed document is refused with the parser's error, and an array no
/// proto field can hold (one of a primitive wider than a byte) with
/// protowire-go's text, naming the message or composite and the field.
pub fn xml_to_proto(xml: &str) -> Result<String, SbeError> {
    let schema = parse_xml_schema(xml).map_err(|e| SbeError::new(e.to_string()))?;
    generate_proto(&schema)
}

fn generate_proto(schema: &XmlSchema) -> Result<String, SbeError> {
    // Build type-resolution maps; built-ins seeded first so user types can
    // shadow them (the Go reference does the same).
    let mut type_map: HashMap<String, XmlType> = HashMap::new();
    for &name in BUILTINS {
        type_map.insert(
            name.to_string(),
            XmlType {
                name: name.to_string(),
                primitive_type: name.to_string(),
                length: None,
                description: None,
                character_encoding: None,
            },
        );
    }
    for t in &schema.types.types {
        type_map.insert(t.name.clone(), t.clone());
    }

    let mut composite_map: HashMap<String, XmlComposite> = HashMap::new();
    for c in &schema.types.composites {
        composite_map.insert(c.name.clone(), c.clone());
    }

    let mut enum_map: HashMap<String, XmlEnum> = HashMap::new();
    for e in &schema.types.enums {
        enum_map.insert(e.name.clone(), e.clone());
    }

    let mut out = String::new();
    let _ = writeln!(out, "syntax = \"proto3\";\n");
    if !schema.package.is_empty() {
        let _ = writeln!(out, "package {};\n", schema.package);
    }
    let _ = writeln!(out, "import \"sbe/annotations.proto\";\n");
    let _ = writeln!(out, "option (sbe.schema_id) = {};", schema.id);
    let _ = writeln!(out, "option (sbe.version) = {};\n", schema.version);

    for e in &schema.types.enums {
        out.push_str(&write_proto_enum(e));
    }
    for c in &schema.types.composites {
        if c.name == "messageHeader" || c.name == "groupSizeEncoding" {
            continue;
        }
        out.push_str(&write_proto_composite(c)?);
    }
    for m in &schema.messages {
        out.push_str(&write_proto_message(
            m,
            &type_map,
            &composite_map,
            &enum_map,
        )?);
    }
    Ok(out)
}

fn write_proto_enum(e: &XmlEnum) -> String {
    let mut out = format!("enum {} {{\n", e.name);
    let prefix = camel_to_screaming_snake(&e.name);
    for v in &e.valid_values {
        let _ = writeln!(
            out,
            "  {}_{} = {};",
            prefix,
            camel_to_screaming_snake(&v.name),
            v.value
        );
    }
    out.push_str("}\n\n");
    out
}

fn write_proto_composite(c: &XmlComposite) -> Result<String, SbeError> {
    let mut out = format!("message {} {{\n", c.name);
    let mut field_num = 1u32;
    for t in &c.types {
        let (proto_type, opts, note) = resolve_type_to_proto(t)
            .map_err(|e| SbeError::new(format!("sbe: composite {:?}: {e}", c.name)))?;
        let name = camel_to_snake(&t.name);
        write_note(&mut out, "  ", &note);
        if !opts.is_empty() {
            let _ = writeln!(out, "  {} {} = {} [{}];", proto_type, name, field_num, opts);
        } else {
            let _ = writeln!(out, "  {} {} = {};", proto_type, name, field_num);
        }
        field_num += 1;
    }
    for r in &c.refs {
        let name = camel_to_snake(&r.name);
        let _ = writeln!(out, "  {} {} = {};", r.r#type, name, field_num);
        field_num += 1;
    }
    out.push_str("}\n\n");
    Ok(out)
}

fn write_proto_message(
    m: &XmlMessage,
    type_map: &HashMap<String, XmlType>,
    composite_map: &HashMap<String, XmlComposite>,
    enum_map: &HashMap<String, XmlEnum>,
) -> Result<String, SbeError> {
    let in_message = |e: String| SbeError::new(format!("sbe: message {:?}: {e}", m.name));
    let mut out = format!("message {} {{\n", m.name);
    let _ = writeln!(out, "  option (sbe.template_id) = {};", m.id);
    for f in &m.fields {
        out.push_str(
            &write_proto_field(f, type_map, composite_map, enum_map, "  ").map_err(in_message)?,
        );
    }
    for g in &m.groups {
        out.push_str(
            &write_proto_group(g, type_map, composite_map, enum_map, "  ").map_err(in_message)?,
        );
    }
    out.push_str("}\n\n");
    Ok(out)
}

fn write_proto_field(
    f: &XmlField,
    type_map: &HashMap<String, XmlType>,
    composite_map: &HashMap<String, XmlComposite>,
    enum_map: &HashMap<String, XmlEnum>,
    indent: &str,
) -> Result<String, String> {
    let name = camel_to_snake(&f.name);

    if enum_map.contains_key(&f.r#type) {
        return Ok(format!("{}{} {} = {};\n", indent, f.r#type, name, f.id));
    }
    if composite_map.contains_key(&f.r#type) {
        return Ok(format!("{}{} {} = {};\n", indent, f.r#type, name, f.id));
    }
    if let Some(t) = type_map.get(&f.r#type) {
        let (proto_type, opts, note) =
            resolve_type_to_proto(t).map_err(|e| format!("field {:?}: {e}", f.name))?;
        let mut out = String::new();
        write_note(&mut out, indent, &note);
        if !opts.is_empty() {
            let _ = writeln!(
                out,
                "{}{} {} = {} [{}];",
                indent, proto_type, name, f.id, opts
            );
        } else {
            let _ = writeln!(out, "{}{} {} = {};", indent, proto_type, name, f.id);
        }
        return Ok(out);
    }
    // Unknown type — pass through so the user's protoc surfaces the error.
    Ok(format!("{}{} {} = {};\n", indent, f.r#type, name, f.id))
}

fn write_proto_group(
    g: &XmlGroup,
    type_map: &HashMap<String, XmlType>,
    composite_map: &HashMap<String, XmlComposite>,
    enum_map: &HashMap<String, XmlEnum>,
    indent: &str,
) -> Result<String, String> {
    let msg_name = singular_pascal(&g.name);
    let mut out = format!("{}message {} {{\n", indent, msg_name);
    let inner_indent = format!("{}  ", indent);
    for f in &g.fields {
        out.push_str(
            &write_proto_field(f, type_map, composite_map, enum_map, &inner_indent)
                .map_err(|e| format!("group {:?}: {e}", g.name))?,
        );
    }
    let _ = writeln!(out, "{}}}", indent);
    let field_name = camel_to_snake(&g.name);
    let _ = writeln!(
        out,
        "{}repeated {} {} = {};",
        indent, msg_name, field_name, g.id
    );
    Ok(out)
}

/// Map an SBE type to a proto type and its field options, plus a note for
/// the line above the field when there is one. A `char` array is a
/// `string` only when its `characterEncoding` is UTF-8 compatible; a proto
/// string holds UTF-8 alone and a decoder refuses one that is not, so text
/// in another encoding is carried as `bytes` of the same `(sbe.length)`
/// rather than failing at run time (draft -01 § Character Encoding,
/// protowire#494). Mirrors protowire-go#248.
///
/// An array of `uint8` or `int8` (length past 1) is raw bytes, the same N
/// bytes on the wire as a `bytes` field with `(sbe.length) = N`. An array of
/// a wider primitive is refused rather than imported as one element, which
/// would misplace every later field in the block (draft -01 § Fixed-Length
/// Arrays, protowire-go#249). A length of 1, or none, is a single value.
fn resolve_type_to_proto(t: &XmlType) -> Result<(String, String, String), String> {
    if t.primitive_type == "char" {
        let len = t.length.filter(|&n| n > 0).unwrap_or(1);
        let opts = format!("(sbe.length) = {len}");
        let encoding = t.character_encoding.as_deref().unwrap_or("");
        if !is_utf8_compatible(encoding) {
            let note = format!(
                "SBE characterEncoding {encoding:?} is not UTF-8 compatible, so this field is bytes."
            );
            return Ok(("bytes".into(), opts, note));
        }
        return Ok(("string".into(), opts, String::new()));
    }
    if let Some(len) = t.length.filter(|&n| n > 1) {
        return match t.primitive_type.as_str() {
            "uint8" | "int8" => Ok((
                "bytes".into(),
                format!("(sbe.length) = {len}"),
                String::new(),
            )),
            other => Err(format!(
                "type {:?} is an array of {len} {other}, which no proto field represents",
                t.name
            )),
        };
    }
    let (proto_type, opts) = resolve_primitive_to_proto(&t.primitive_type);
    Ok((proto_type, opts, String::new()))
}

/// Write `note` as a comment line, if there is one.
fn write_note(out: &mut String, indent: &str, note: &str) {
    if !note.is_empty() {
        let _ = writeln!(out, "{indent}// {note}");
    }
}

/// Whether every string in a char array of the named encoding is valid
/// UTF-8: none named (FIX SBE's default is US-ASCII), a US-ASCII name, or a
/// UTF-8 name. Names are IANA charset names and their registered aliases,
/// compared ignoring case and punctuation.
fn is_utf8_compatible(encoding: &str) -> bool {
    let name: String = encoding
        .trim()
        .chars()
        .filter(|c| !matches!(c, '-' | '_' | '.' | ':' | ' '))
        .flat_map(char::to_lowercase)
        .collect();
    matches!(
        name.as_str(),
        // None named.
        ""
            // US-ASCII and its IANA aliases.
            | "usascii"
            | "ascii"
            | "ansix341968"
            | "ansix341986"
            | "iso646us"
            | "isoir6"
            | "iso646irv1991"
            | "us"
            | "ibm367"
            | "cp367"
            | "csascii"
            // UTF-8.
            | "utf8"
            | "csutf8"
    )
}

/// Map an SBE primitive type other than `char`.
fn resolve_primitive_to_proto(primitive_type: &str) -> (String, String) {
    match primitive_type {
        "int8" => ("int32".into(), r#"(sbe.encoding) = "int8""#.into()),
        "int16" => ("int32".into(), r#"(sbe.encoding) = "int16""#.into()),
        "int32" => ("int32".into(), String::new()),
        "int64" => ("int64".into(), String::new()),
        "uint8" => ("uint32".into(), r#"(sbe.encoding) = "uint8""#.into()),
        "uint16" => ("uint32".into(), r#"(sbe.encoding) = "uint16""#.into()),
        "uint32" => ("uint32".into(), String::new()),
        "uint64" => ("uint64".into(), String::new()),
        "float" => ("float".into(), String::new()),
        "double" => ("double".into(), String::new()),
        other => (other.to_string(), String::new()),
    }
}
