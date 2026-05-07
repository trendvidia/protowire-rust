// SPDX-License-Identifier: MIT
// Copyright (c) 2026 TrendVidia, LLC.
//! SBE XML schema model + parser. Mirrors `protowire/encoding/sbe/xmlschema.go`
//! and the TS port's `sbe/xmlschema.ts`.
//!
//! Also exports the name-conversion helpers shared between
//! [`crate::xml_to_proto`] and [`crate::proto_to_xml`].

use std::collections::HashMap;

use crate::saxlite::{parse_xml, SaxError, SaxHandler};

#[derive(Debug, Default, Clone)]
pub struct XmlSchema {
    pub package: String,
    pub id: u32,
    pub version: u32,
    pub byte_order: String,
    pub description: String,
    pub types: XmlTypes,
    pub messages: Vec<XmlMessage>,
}

#[derive(Debug, Default, Clone)]
pub struct XmlTypes {
    pub types: Vec<XmlType>,
    pub composites: Vec<XmlComposite>,
    pub enums: Vec<XmlEnum>,
}

#[derive(Debug, Clone)]
pub struct XmlType {
    pub name: String,
    pub primitive_type: String,
    pub length: Option<u32>,
    pub description: Option<String>,
}

#[derive(Debug, Clone)]
pub struct XmlComposite {
    pub name: String,
    pub description: Option<String>,
    pub types: Vec<XmlType>,
    pub refs: Vec<XmlRef>,
}

#[derive(Debug, Clone)]
pub struct XmlRef {
    pub name: String,
    pub r#type: String,
}

#[derive(Debug, Clone)]
pub struct XmlEnum {
    pub name: String,
    pub encoding_type: String,
    pub description: Option<String>,
    pub valid_values: Vec<XmlValidValue>,
}

#[derive(Debug, Clone)]
pub struct XmlValidValue {
    pub name: String,
    pub value: String,
}

#[derive(Debug, Clone)]
pub struct XmlMessage {
    pub name: String,
    pub id: u32,
    pub description: Option<String>,
    pub fields: Vec<XmlField>,
    pub groups: Vec<XmlGroup>,
}

#[derive(Debug, Clone)]
pub struct XmlField {
    pub name: String,
    pub id: u32,
    pub r#type: String,
}

#[derive(Debug, Clone)]
pub struct XmlGroup {
    pub name: String,
    pub id: u32,
    pub fields: Vec<XmlField>,
}

/// Parse an SBE XML schema document into an [`XmlSchema`]. Element namespace
/// prefixes (e.g. `sbe:message`) are stripped by the SAX layer, so the
/// builder below keys off plain names.
pub fn parse_xml_schema(xml: &str) -> Result<XmlSchema, SaxError> {
    let mut handler = SchemaBuilder::default();
    parse_xml(xml, &mut handler)?;
    if !handler.stack.is_empty() {
        return Err(SaxError::new(
            format!("sbe-xml: unclosed elements: {}", handler.stack.join(" > ")),
            0,
        ));
    }
    Ok(handler.schema)
}

#[derive(Default)]
struct SchemaBuilder {
    schema: XmlSchema,
    stack: Vec<String>,
    text_buf: String,
    // Indices into the schema rather than mutable references — keeps the
    // borrow checker happy across the SAX callbacks.
    current_composite: Option<usize>,
    current_enum: Option<usize>,
    current_valid_value: bool,
    current_message: Option<usize>,
    current_group: Option<usize>,
}

impl SaxHandler for SchemaBuilder {
    fn open(&mut self, name: &str, attrs: &HashMap<String, String>) -> Result<(), SaxError> {
        let parent = self.stack.last().cloned();
        self.stack.push(name.to_string());

        match name {
            "messageSchema" => {
                self.schema.package = attrs.get("package").cloned().unwrap_or_default();
                self.schema.id = parse_uint(attrs.get("id"))?;
                self.schema.version = parse_uint(attrs.get("version"))?;
                self.schema.byte_order = attrs.get("byteOrder").cloned().unwrap_or_default();
                self.schema.description = attrs.get("description").cloned().unwrap_or_default();
            }
            "types" => {}
            "type" => {
                let length = match attrs.get("length") {
                    Some(s) => Some(parse_uint(Some(s))?),
                    None => None,
                };
                let t = XmlType {
                    name: attrs.get("name").cloned().unwrap_or_default(),
                    primitive_type: attrs.get("primitiveType").cloned().unwrap_or_default(),
                    length,
                    description: attrs.get("description").cloned(),
                };
                if parent.as_deref() == Some("composite") {
                    if let Some(idx) = self.current_composite {
                        self.schema.types.composites[idx].types.push(t);
                    }
                } else {
                    self.schema.types.types.push(t);
                }
            }
            "ref" => {
                if let Some(idx) = self.current_composite {
                    self.schema.types.composites[idx].refs.push(XmlRef {
                        name: attrs.get("name").cloned().unwrap_or_default(),
                        r#type: attrs.get("type").cloned().unwrap_or_default(),
                    });
                }
            }
            "composite" => {
                let comp = XmlComposite {
                    name: attrs.get("name").cloned().unwrap_or_default(),
                    description: attrs.get("description").cloned(),
                    types: Vec::new(),
                    refs: Vec::new(),
                };
                self.schema.types.composites.push(comp);
                self.current_composite = Some(self.schema.types.composites.len() - 1);
            }
            "enum" => {
                let e = XmlEnum {
                    name: attrs.get("name").cloned().unwrap_or_default(),
                    encoding_type: attrs.get("encodingType").cloned().unwrap_or_default(),
                    description: attrs.get("description").cloned(),
                    valid_values: Vec::new(),
                };
                self.schema.types.enums.push(e);
                self.current_enum = Some(self.schema.types.enums.len() - 1);
            }
            "validValue" => {
                self.text_buf.clear();
                if let Some(idx) = self.current_enum {
                    self.schema.types.enums[idx].valid_values.push(XmlValidValue {
                        name: attrs.get("name").cloned().unwrap_or_default(),
                        value: String::new(),
                    });
                    self.current_valid_value = true;
                }
            }
            "message" => {
                let m = XmlMessage {
                    name: attrs.get("name").cloned().unwrap_or_default(),
                    id: parse_uint(attrs.get("id"))?,
                    description: attrs.get("description").cloned(),
                    fields: Vec::new(),
                    groups: Vec::new(),
                };
                self.schema.messages.push(m);
                self.current_message = Some(self.schema.messages.len() - 1);
            }
            "group" => {
                let g = XmlGroup {
                    name: attrs.get("name").cloned().unwrap_or_default(),
                    id: parse_uint(attrs.get("id"))?,
                    fields: Vec::new(),
                };
                if let Some(idx) = self.current_message {
                    self.schema.messages[idx].groups.push(g);
                    self.current_group = Some(self.schema.messages[idx].groups.len() - 1);
                }
            }
            "field" => {
                let f = XmlField {
                    name: attrs.get("name").cloned().unwrap_or_default(),
                    id: parse_uint(attrs.get("id"))?,
                    r#type: attrs.get("type").cloned().unwrap_or_default(),
                };
                if let (Some(mi), Some(gi)) = (self.current_message, self.current_group) {
                    self.schema.messages[mi].groups[gi].fields.push(f);
                } else if let Some(mi) = self.current_message {
                    self.schema.messages[mi].fields.push(f);
                }
            }
            _ => {}
        }
        Ok(())
    }

    fn text(&mut self, value: &str) -> Result<(), SaxError> {
        if self.current_valid_value {
            self.text_buf.push_str(value);
        }
        Ok(())
    }

    fn close(&mut self, name: &str) -> Result<(), SaxError> {
        let popped = self.stack.pop().unwrap_or_default();
        if popped != name {
            return Err(SaxError::new(
                format!("sbe-xml: close mismatch: got </{}>, expected </{}>", name, popped),
                0,
            ));
        }
        match name {
            "validValue" => {
                if let Some(ei) = self.current_enum {
                    let last = self.schema.types.enums[ei]
                        .valid_values
                        .last_mut()
                        .expect("validValue tracking");
                    last.value = self.text_buf.trim().to_string();
                }
                self.current_valid_value = false;
                self.text_buf.clear();
            }
            "enum" => self.current_enum = None,
            "composite" => self.current_composite = None,
            "group" => self.current_group = None,
            "message" => self.current_message = None,
            _ => {}
        }
        Ok(())
    }
}

fn parse_uint(v: Option<&String>) -> Result<u32, SaxError> {
    let Some(s) = v else { return Ok(0) };
    if s.is_empty() {
        return Ok(0);
    }
    s.parse::<u32>()
        .map_err(|_| SaxError::new(format!("sbe-xml: invalid uint value {:?}", s), 0))
}

// ---------- name conversion helpers (shared with proto↔xml converters) ----------

pub fn camel_to_snake(s: &str) -> String {
    let chars: Vec<char> = s.chars().collect();
    let mut out = String::with_capacity(s.len() + 4);
    for (i, &ch) in chars.iter().enumerate() {
        if ch.is_ascii_uppercase() {
            if i > 0 {
                let prev = chars[i - 1];
                let next_is_lower = i + 1 < chars.len() && chars[i + 1].is_ascii_lowercase();
                if prev.is_ascii_lowercase() || next_is_lower {
                    out.push('_');
                }
            }
            out.push(ch.to_ascii_lowercase());
        } else {
            out.push(ch);
        }
    }
    out
}

pub fn snake_to_camel(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for (i, part) in s.split('_').enumerate() {
        if part.is_empty() {
            continue;
        }
        if i == 0 {
            out.push_str(part);
        } else {
            let mut chars = part.chars();
            if let Some(first) = chars.next() {
                out.push(first.to_ascii_uppercase());
                out.push_str(chars.as_str());
            }
        }
    }
    out
}

pub fn camel_to_screaming_snake(s: &str) -> String {
    let chars: Vec<char> = s.chars().collect();
    let mut out = String::with_capacity(s.len() + 4);
    for (i, &ch) in chars.iter().enumerate() {
        if ch.is_ascii_uppercase() && i > 0 && chars[i - 1].is_ascii_lowercase() {
            out.push('_');
        }
        out.push(ch.to_ascii_uppercase());
    }
    out
}

pub fn screaming_snake_to_pascal(s: &str) -> String {
    let lower = s.to_ascii_lowercase();
    let mut out = String::with_capacity(s.len());
    for part in lower.split('_') {
        if part.is_empty() {
            continue;
        }
        let mut chars = part.chars();
        if let Some(first) = chars.next() {
            out.push(first.to_ascii_uppercase());
            out.push_str(chars.as_str());
        }
    }
    out
}

pub fn strip_enum_prefix(value_name: &str, enum_name: &str) -> String {
    let prefix = format!("{}_", camel_to_screaming_snake(enum_name));
    if let Some(rest) = value_name.strip_prefix(&prefix) {
        screaming_snake_to_pascal(rest)
    } else {
        screaming_snake_to_pascal(value_name)
    }
}

pub fn singular_pascal(s: &str) -> String {
    if s.is_empty() {
        return String::new();
    }
    let trimmed = if s.ends_with("ies") && s.len() > 3 {
        format!("{}y", &s[..s.len() - 3])
    } else if s.ends_with('s') && !s.ends_with("ss") && s.len() > 1 {
        s[..s.len() - 1].to_string()
    } else {
        s.to_string()
    };
    let mut chars = trimmed.chars();
    let first = chars.next().unwrap();
    let rest: String = chars.collect();
    format!("{}{}", first.to_ascii_uppercase(), rest)
}
