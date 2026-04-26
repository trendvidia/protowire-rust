//! Convert an SBE XML schema into proto3 source with `sbe.*` annotations.
//! Mirrors `protowire/encoding/sbe/xmltoproto.go` and the TS port's
//! `sbe/xmltoproto.ts`.

use std::collections::HashMap;
use std::fmt::Write as _;

use crate::saxlite::SaxError;
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
pub fn xml_to_proto(xml: &str) -> Result<String, SaxError> {
    let schema = parse_xml_schema(xml)?;
    Ok(generate_proto(&schema))
}

fn generate_proto(schema: &XmlSchema) -> String {
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
        out.push_str(&write_proto_composite(c));
    }
    for m in &schema.messages {
        out.push_str(&write_proto_message(m, &type_map, &composite_map, &enum_map));
    }
    out
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

fn write_proto_composite(c: &XmlComposite) -> String {
    let mut out = format!("message {} {{\n", c.name);
    let mut field_num = 1u32;
    for t in &c.types {
        let (proto_type, opts) =
            resolve_type_to_proto(&t.primitive_type, t.length.unwrap_or(0));
        let name = camel_to_snake(&t.name);
        if !opts.is_empty() {
            let _ = writeln!(
                out,
                "  {} {} = {} [{}];",
                proto_type, name, field_num, opts
            );
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
    out
}

fn write_proto_message(
    m: &XmlMessage,
    type_map: &HashMap<String, XmlType>,
    composite_map: &HashMap<String, XmlComposite>,
    enum_map: &HashMap<String, XmlEnum>,
) -> String {
    let mut out = format!("message {} {{\n", m.name);
    let _ = writeln!(out, "  option (sbe.template_id) = {};", m.id);
    for f in &m.fields {
        out.push_str(&write_proto_field(f, type_map, composite_map, enum_map, "  "));
    }
    for g in &m.groups {
        out.push_str(&write_proto_group(g, type_map, composite_map, enum_map, "  "));
    }
    out.push_str("}\n\n");
    out
}

fn write_proto_field(
    f: &XmlField,
    type_map: &HashMap<String, XmlType>,
    composite_map: &HashMap<String, XmlComposite>,
    enum_map: &HashMap<String, XmlEnum>,
    indent: &str,
) -> String {
    let name = camel_to_snake(&f.name);

    if enum_map.contains_key(&f.r#type) {
        return format!("{}{} {} = {};\n", indent, f.r#type, name, f.id);
    }
    if composite_map.contains_key(&f.r#type) {
        return format!("{}{} {} = {};\n", indent, f.r#type, name, f.id);
    }
    if let Some(t) = type_map.get(&f.r#type) {
        let (proto_type, opts) =
            resolve_type_to_proto(&t.primitive_type, t.length.unwrap_or(0));
        return if !opts.is_empty() {
            format!(
                "{}{} {} = {} [{}];\n",
                indent, proto_type, name, f.id, opts
            )
        } else {
            format!("{}{} {} = {};\n", indent, proto_type, name, f.id)
        };
    }
    // Unknown type — pass through so the user's protoc surfaces the error.
    format!("{}{} {} = {};\n", indent, f.r#type, name, f.id)
}

fn write_proto_group(
    g: &XmlGroup,
    type_map: &HashMap<String, XmlType>,
    composite_map: &HashMap<String, XmlComposite>,
    enum_map: &HashMap<String, XmlEnum>,
    indent: &str,
) -> String {
    let msg_name = singular_pascal(&g.name);
    let mut out = format!("{}message {} {{\n", indent, msg_name);
    let inner_indent = format!("{}  ", indent);
    for f in &g.fields {
        out.push_str(&write_proto_field(
            f,
            type_map,
            composite_map,
            enum_map,
            &inner_indent,
        ));
    }
    let _ = writeln!(out, "{}}}", indent);
    let field_name = camel_to_snake(&g.name);
    let _ = writeln!(
        out,
        "{}repeated {} {} = {};",
        indent, msg_name, field_name, g.id
    );
    out
}

fn resolve_type_to_proto(primitive_type: &str, length: u32) -> (String, String) {
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
        "char" => {
            let len = if length > 0 { length } else { 1 };
            ("string".into(), format!("(sbe.length) = {}", len))
        }
        other => (other.to_string(), String::new()),
    }
}
