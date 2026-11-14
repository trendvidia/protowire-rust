// SPDX-License-Identifier: MIT
// Copyright (c) 2026 TrendVidia, LLC.
//! SBE XML schema model + parser. Mirrors `protowire/encoding/sbe/xmlschema.go`
//! and the TS port's `sbe/xmlschema.ts`.
//!
//! Also exports the name-conversion helpers shared between
//! [`crate::xml_to_proto`] and [`crate::proto_to_xml`].

use crate::saxlite::{go_quote, Attr, SaxError, Token, Tokenizer};

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
    /// The IANA charset name a `char` type's bytes are in. FIX SBE makes
    /// US-ASCII the default for a char array.
    pub character_encoding: Option<String>,
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

/// Parse an SBE XML schema document into an [`XmlSchema`], as
/// protowire-go's `ParseXMLSchema` reads one with `xml.Unmarshal`:
///
/// - `sbe:message` is replaced by `message` everywhere in the text first, as
///   protowire-go does, and other prefixes are stripped from names;
/// - the document's first element must be `messageSchema`; what follows it
///   is not read, and a document with no element is refused as `EOF`;
/// - an element is read only where the schema has a place for it
///   (`messageSchema > types > composite > type`, and so on); any other is
///   skipped, its attributes unread;
/// - attributes are read in document order. A number is parsed as Go's
///   `strconv.ParseUint` parses it, after trimming spaces, and an empty one
///   is 0.
///
/// Errors carry protowire-go's texts; see [`SaxError`].
pub fn parse_xml_schema(xml: &str) -> Result<XmlSchema, SaxError> {
    let input = xml.replace("sbe:message", "message");
    let mut t = Tokenizer::new(&input);
    let root = loop {
        match t.token()? {
            None => return Err(SaxError::new("EOF", input.len())),
            Some(Token::Start(name, attrs)) => break (name, attrs),
            Some(_) => {}
        }
    };
    if root.0.local != "messageSchema" {
        return Err(SaxError::new(
            format!(
                "expected element type <messageSchema> but have <{}>",
                root.0.local
            ),
            0,
        ));
    }
    let mut b = SchemaBuilder::default();
    let mut stack = vec![Place::Schema];
    b.open(Place::Schema, &root.1)?;
    while !stack.is_empty() {
        match t.token()?.expect("open elements end in an error") {
            Token::Start(name, attrs) => {
                let place = stack.last().expect("open").child(&name.local);
                b.open(place, &attrs)?;
                stack.push(place);
            }
            Token::End(_) => {
                let place = stack.pop().expect("open");
                b.close(place);
            }
            Token::Text(s) => {
                if stack.last() == Some(&Place::ValidValue) {
                    b.text_buf.push_str(&s);
                }
            }
            Token::Other => {}
        }
    }
    Ok(b.schema)
}

/// Where an element sits in the schema: the field of protowire-go's
/// `XMLSchema` structs it is read into, or `Skip`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Place {
    Schema,
    Types,
    Type,
    Composite,
    CompositeType,
    Ref,
    Enum,
    ValidValue,
    Message,
    Field,
    Group,
    GroupField,
    Skip,
}

impl Place {
    fn child(self, local: &str) -> Place {
        match (self, local) {
            (Place::Schema, "types") => Place::Types,
            (Place::Schema, "message") => Place::Message,
            (Place::Types, "type") => Place::Type,
            (Place::Types, "composite") => Place::Composite,
            (Place::Types, "enum") => Place::Enum,
            (Place::Composite, "type") => Place::CompositeType,
            (Place::Composite, "ref") => Place::Ref,
            (Place::Enum, "validValue") => Place::ValidValue,
            (Place::Message, "field") => Place::Field,
            (Place::Message, "group") => Place::Group,
            (Place::Group, "field") => Place::GroupField,
            _ => Place::Skip,
        }
    }
}

#[derive(Default)]
struct SchemaBuilder {
    schema: XmlSchema,
    text_buf: String,
}

/// The attributes, as `name=value` by local name, in document order;
/// namespace declarations left out.
fn attrs_of(attrs: &[Attr]) -> impl Iterator<Item = (&str, &str)> {
    attrs
        .iter()
        .filter(|a| !a.is_xmlns())
        .map(|a| (a.name.local.as_str(), a.value.as_str()))
}

impl SchemaBuilder {
    fn open(&mut self, place: Place, attrs: &[Attr]) -> Result<(), SaxError> {
        let s = &mut self.schema;
        match place {
            Place::Schema => {
                for (k, v) in attrs_of(attrs) {
                    match k {
                        "package" => s.package = v.to_string(),
                        "id" => s.id = parse_uint(v)?,
                        "version" => s.version = parse_uint(v)?,
                        "byteOrder" => s.byte_order = v.to_string(),
                        "description" => s.description = v.to_string(),
                        _ => {}
                    }
                }
            }
            Place::Type | Place::CompositeType => {
                let mut t = XmlType {
                    name: String::new(),
                    primitive_type: String::new(),
                    length: None,
                    description: None,
                    character_encoding: None,
                };
                for (k, v) in attrs_of(attrs) {
                    match k {
                        "name" => t.name = v.to_string(),
                        "primitiveType" => t.primitive_type = v.to_string(),
                        "length" => t.length = Some(parse_uint(v)?),
                        "description" => t.description = Some(v.to_string()),
                        "characterEncoding" => t.character_encoding = Some(v.to_string()),
                        _ => {}
                    }
                }
                if place == Place::Type {
                    s.types.types.push(t);
                } else {
                    s.types
                        .composites
                        .last_mut()
                        .expect("composite")
                        .types
                        .push(t);
                }
            }
            Place::Ref => {
                let mut r = XmlRef {
                    name: String::new(),
                    r#type: String::new(),
                };
                for (k, v) in attrs_of(attrs) {
                    match k {
                        "name" => r.name = v.to_string(),
                        "type" => r.r#type = v.to_string(),
                        _ => {}
                    }
                }
                s.types
                    .composites
                    .last_mut()
                    .expect("composite")
                    .refs
                    .push(r);
            }
            Place::Composite => {
                let mut c = XmlComposite {
                    name: String::new(),
                    description: None,
                    types: Vec::new(),
                    refs: Vec::new(),
                };
                for (k, v) in attrs_of(attrs) {
                    match k {
                        "name" => c.name = v.to_string(),
                        "description" => c.description = Some(v.to_string()),
                        _ => {}
                    }
                }
                s.types.composites.push(c);
            }
            Place::Enum => {
                let mut e = XmlEnum {
                    name: String::new(),
                    encoding_type: String::new(),
                    description: None,
                    valid_values: Vec::new(),
                };
                for (k, v) in attrs_of(attrs) {
                    match k {
                        "name" => e.name = v.to_string(),
                        "encodingType" => e.encoding_type = v.to_string(),
                        "description" => e.description = Some(v.to_string()),
                        _ => {}
                    }
                }
                s.types.enums.push(e);
            }
            Place::ValidValue => {
                self.text_buf.clear();
                let mut name = String::new();
                for (k, v) in attrs_of(attrs) {
                    if k == "name" {
                        name = v.to_string();
                    }
                }
                s.types
                    .enums
                    .last_mut()
                    .expect("enum")
                    .valid_values
                    .push(XmlValidValue {
                        name,
                        value: String::new(),
                    });
            }
            Place::Message => {
                let mut m = XmlMessage {
                    name: String::new(),
                    id: 0,
                    description: None,
                    fields: Vec::new(),
                    groups: Vec::new(),
                };
                for (k, v) in attrs_of(attrs) {
                    match k {
                        "name" => m.name = v.to_string(),
                        "id" => m.id = parse_uint(v)?,
                        "description" => m.description = Some(v.to_string()),
                        _ => {}
                    }
                }
                s.messages.push(m);
            }
            Place::Group => {
                let mut g = XmlGroup {
                    name: String::new(),
                    id: 0,
                    fields: Vec::new(),
                };
                for (k, v) in attrs_of(attrs) {
                    match k {
                        "name" => g.name = v.to_string(),
                        "id" => g.id = parse_uint(v)?,
                        _ => {}
                    }
                }
                s.messages.last_mut().expect("message").groups.push(g);
            }
            Place::Field | Place::GroupField => {
                let mut f = XmlField {
                    name: String::new(),
                    id: 0,
                    r#type: String::new(),
                };
                for (k, v) in attrs_of(attrs) {
                    match k {
                        "name" => f.name = v.to_string(),
                        "id" => f.id = parse_uint(v)?,
                        "type" => f.r#type = v.to_string(),
                        _ => {}
                    }
                }
                let m = s.messages.last_mut().expect("message");
                if place == Place::Field {
                    m.fields.push(f);
                } else {
                    m.groups.last_mut().expect("group").fields.push(f);
                }
            }
            Place::Types | Place::Skip => {}
        }
        Ok(())
    }

    fn close(&mut self, place: Place) {
        if place == Place::ValidValue {
            // protowire-go keeps the character data as it is, spaces and all.
            let v = std::mem::take(&mut self.text_buf);
            let e = self.schema.types.enums.last_mut().expect("enum");
            e.valid_values.last_mut().expect("validValue").value = v;
        }
    }
}

/// An attribute read into a `uint32`, as `encoding/xml` reads one: empty is
/// 0, and anything else is trimmed of spaces and parsed by Go's
/// `strconv.ParseUint(s, 10, 32)`, with its error text.
fn parse_uint(v: &str) -> Result<u32, SaxError> {
    if v.is_empty() {
        return Ok(0);
    }
    let s = v.trim();
    let err = |what: &str| {
        SaxError::new(
            format!("strconv.ParseUint: parsing {}: {what}", go_quote(s)),
            0,
        )
    };
    if s.is_empty() {
        return Err(err("invalid syntax"));
    }
    let mut n: u64 = 0;
    for c in s.bytes() {
        if !c.is_ascii_digit() {
            return Err(err("invalid syntax"));
        }
        n = n * 10 + u64::from(c - b'0');
        if n > u64::from(u32::MAX) {
            return Err(err("value out of range"));
        }
    }
    Ok(n as u32)
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
