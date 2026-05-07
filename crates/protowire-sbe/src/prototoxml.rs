// SPDX-License-Identifier: MIT
// Copyright (c) 2026 TrendVidia, LLC.
//! Convert proto file descriptors with SBE annotations into an SBE XML
//! schema. Mirrors `protowire/encoding/sbe/prototoxml.go` and the TS port's
//! `sbe/prototoxml.ts`.

use std::collections::HashSet;
use std::fmt::Write as _;

use prost_reflect::{EnumDescriptor, FieldDescriptor, FileDescriptor, Kind, MessageDescriptor};

use crate::annotations::{
    field_string, field_uint32, file_uint32, message_uint32, EXT_ENCODING, EXT_LENGTH,
    EXT_SCHEMA_ID, EXT_TEMPLATE_ID, EXT_VERSION,
};
use crate::errors::SbeError;
use crate::xmlschema::{snake_to_camel, strip_enum_prefix};

struct SbeTypeInfo {
    primitive_type: String,
    xml_type: String,
    length: u32,
}

pub fn proto_to_xml(file: &FileDescriptor) -> Result<String, SbeError> {
    let schema_id = file_uint32(file, EXT_SCHEMA_ID).ok_or_else(|| {
        SbeError::new(format!("sbe: file {} missing (sbe.schema_id)", file.name()))
    })?;
    let version = file_uint32(file, EXT_VERSION).unwrap_or(0);

    // Pre-collect types referenced by template messages.
    let mut str_lengths: HashSet<u32> = HashSet::new();
    let mut composites: Vec<MessageDescriptor> = Vec::new();
    let mut composites_seen: HashSet<String> = HashSet::new();
    let mut enums: Vec<EnumDescriptor> = Vec::new();
    let mut enums_seen: HashSet<String> = HashSet::new();

    for ed in file.enums() {
        enums_seen.insert(ed.full_name().to_string());
        enums.push(ed);
    }

    for md in file.messages() {
        if message_uint32(&md, EXT_TEMPLATE_ID).is_some() {
            collect_types(
                &md,
                &mut str_lengths,
                &mut composites,
                &mut composites_seen,
                &mut enums,
                &mut enums_seen,
            );
        } else if !composites_seen.contains(md.full_name()) {
            composites_seen.insert(md.full_name().to_string());
            composites.push(md);
        }
    }

    let mut lengths: Vec<u32> = str_lengths.into_iter().collect();
    lengths.sort_unstable();

    let mut out = String::new();
    out.push_str("<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n");
    out.push_str("<sbe:messageSchema xmlns:sbe=\"http://fixprotocol.io/2016/sbe\"\n");
    let _ = writeln!(
        out,
        "                   package=\"{}\"",
        file.package_name()
    );
    let _ = writeln!(out, "                   id=\"{}\"", schema_id);
    let _ = writeln!(out, "                   version=\"{}\"", version);
    out.push_str("                   byteOrder=\"littleEndian\">\n");

    out.push_str("    <types>\n");
    out.push_str("        <composite name=\"messageHeader\">\n");
    out.push_str("            <type name=\"blockLength\" primitiveType=\"uint16\"/>\n");
    out.push_str("            <type name=\"templateId\" primitiveType=\"uint16\"/>\n");
    out.push_str("            <type name=\"schemaId\" primitiveType=\"uint16\"/>\n");
    out.push_str("            <type name=\"version\" primitiveType=\"uint16\"/>\n");
    out.push_str("        </composite>\n");
    out.push_str("        <composite name=\"groupSizeEncoding\">\n");
    out.push_str("            <type name=\"blockLength\" primitiveType=\"uint16\"/>\n");
    out.push_str("            <type name=\"numInGroup\" primitiveType=\"uint16\"/>\n");
    out.push_str("        </composite>\n");

    for l in lengths {
        let _ = writeln!(
            out,
            "        <type name=\"str{l}\" primitiveType=\"char\" length=\"{l}\"/>",
        );
    }
    for e in &enums {
        out.push_str(&write_enum(e));
    }
    for md in &composites {
        out.push_str(&write_composite(md));
    }

    out.push_str("    </types>\n");

    for md in file.messages() {
        if let Some(tid) = message_uint32(&md, EXT_TEMPLATE_ID) {
            out.push_str(&write_message(&md, tid));
        }
    }
    out.push_str("</sbe:messageSchema>\n");
    Ok(out)
}

fn collect_types(
    md: &MessageDescriptor,
    str_lengths: &mut HashSet<u32>,
    composites: &mut Vec<MessageDescriptor>,
    composites_seen: &mut HashSet<String>,
    enums: &mut Vec<EnumDescriptor>,
    enums_seen: &mut HashSet<String>,
) {
    for ed in md.child_enums() {
        if enums_seen.insert(ed.full_name().to_string()) {
            enums.push(ed);
        }
    }
    for f in md.fields() {
        match f.kind() {
            Kind::String | Kind::Bytes => {
                if let Some(len) = field_uint32(&f, EXT_LENGTH) {
                    str_lengths.insert(len);
                }
            }
            Kind::Enum(ed) if enums_seen.insert(ed.full_name().to_string()) => {
                enums.push(ed);
            }
            Kind::Message(sub) => {
                if f.is_list() {
                    collect_types(
                        &sub,
                        str_lengths,
                        composites,
                        composites_seen,
                        enums,
                        enums_seen,
                    );
                } else if composites_seen.insert(sub.full_name().to_string()) {
                    composites.push(sub.clone());
                    collect_types(
                        &sub,
                        str_lengths,
                        composites,
                        composites_seen,
                        enums,
                        enums_seen,
                    );
                }
            }
            _ => {}
        }
    }
}

fn write_enum(ed: &EnumDescriptor) -> String {
    let mut out = format!(
        "        <enum name=\"{}\" encodingType=\"uint8\">\n",
        ed.name()
    );
    for v in ed.values() {
        let value_name = strip_enum_prefix(v.name(), ed.name());
        let _ = writeln!(
            out,
            "            <validValue name=\"{}\">{}</validValue>",
            value_name,
            v.number()
        );
    }
    out.push_str("        </enum>\n");
    out
}

fn write_composite(md: &MessageDescriptor) -> String {
    let mut out = format!("        <composite name=\"{}\">\n", md.name());
    for f in sorted_fields(md) {
        let field_name = snake_to_camel(f.name());
        let info = proto_field_to_sbe_type(&f);
        if info.length > 0 {
            let _ = writeln!(
                out,
                "            <type name=\"{}\" primitiveType=\"{}\" length=\"{}\"/>",
                field_name, info.primitive_type, info.length
            );
        } else {
            let _ = writeln!(
                out,
                "            <type name=\"{}\" primitiveType=\"{}\"/>",
                field_name, info.primitive_type
            );
        }
    }
    out.push_str("        </composite>\n");
    out
}

fn write_message(md: &MessageDescriptor, template_id: u32) -> String {
    let mut out = format!(
        "    <sbe:message name=\"{}\" id=\"{}\">\n",
        md.name(),
        template_id
    );
    for f in sorted_fields(md) {
        if f.is_list() && matches!(f.kind(), Kind::Message(_)) {
            out.push_str(&write_group(&f, "        "));
        } else {
            out.push_str(&write_field(&f, "        "));
        }
    }
    out.push_str("    </sbe:message>\n");
    out
}

fn write_field(fd: &FieldDescriptor, indent: &str) -> String {
    let field_name = snake_to_camel(fd.name());
    let field_id = fd.number();
    if let Kind::Enum(ed) = fd.kind() {
        return format!(
            "{}<field name=\"{}\" id=\"{}\" type=\"{}\"/>\n",
            indent,
            field_name,
            field_id,
            ed.name()
        );
    }
    if let Kind::Message(md) = fd.kind() {
        return format!(
            "{}<field name=\"{}\" id=\"{}\" type=\"{}\"/>\n",
            indent,
            field_name,
            field_id,
            md.name()
        );
    }
    let info = proto_field_to_sbe_type(fd);
    if info.length > 0 {
        format!(
            "{}<field name=\"{}\" id=\"{}\" type=\"str{}\"/>\n",
            indent, field_name, field_id, info.length
        )
    } else {
        format!(
            "{}<field name=\"{}\" id=\"{}\" type=\"{}\"/>\n",
            indent, field_name, field_id, info.xml_type
        )
    }
}

fn write_group(fd: &FieldDescriptor, indent: &str) -> String {
    let group_name = snake_to_camel(fd.name());
    let group_id = fd.number();
    let inner_indent = format!("{}    ", indent);
    let mut out = format!(
        "{}<group name=\"{}\" id=\"{}\">\n",
        indent, group_name, group_id
    );
    if let Kind::Message(sub) = fd.kind() {
        for f in sorted_fields(&sub) {
            out.push_str(&write_field(&f, &inner_indent));
        }
    }
    let _ = writeln!(out, "{}</group>", indent);
    out
}

fn proto_field_to_sbe_type(fd: &FieldDescriptor) -> SbeTypeInfo {
    if let Some(enc) = field_string(fd, EXT_ENCODING) {
        return SbeTypeInfo {
            primitive_type: enc.clone(),
            xml_type: enc,
            length: 0,
        };
    }
    match fd.kind() {
        Kind::String | Kind::Bytes => {
            let length = field_uint32(fd, EXT_LENGTH).unwrap_or(0);
            SbeTypeInfo {
                primitive_type: "char".into(),
                xml_type: "char".into(),
                length,
            }
        }
        Kind::Bool => scalar_info("uint8"),
        Kind::Int32 | Kind::Sint32 | Kind::Sfixed32 => scalar_info("int32"),
        Kind::Int64 | Kind::Sint64 | Kind::Sfixed64 => scalar_info("int64"),
        Kind::Uint32 | Kind::Fixed32 => scalar_info("uint32"),
        Kind::Uint64 | Kind::Fixed64 => scalar_info("uint64"),
        Kind::Float => scalar_info("float"),
        Kind::Double => scalar_info("double"),
        _ => scalar_info("uint8"),
    }
}

fn scalar_info(name: &str) -> SbeTypeInfo {
    SbeTypeInfo {
        primitive_type: name.into(),
        xml_type: name.into(),
        length: 0,
    }
}

fn sorted_fields(md: &MessageDescriptor) -> Vec<FieldDescriptor> {
    let mut v: Vec<FieldDescriptor> = md.fields().collect();
    v.sort_by_key(|f| f.number());
    v
}
