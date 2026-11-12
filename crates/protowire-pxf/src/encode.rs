// SPDX-License-Identifier: MIT
// Copyright (c) 2026 TrendVidia, LLC.
//! Schema-bound PXF text encoder.
//!
//! Mirrors `protowire/encoding/pxf/encode.go` and the TS port's
//! `pxf/encode.ts`. Walks a `prost_reflect::DynamicMessage` alongside its
//! descriptor and emits PXF text covering scalars, enums, nested messages
//! (with WKT shortcuts), repeated lists, key-sorted maps,
//! `google.protobuf.Any` sugar via a [`TypeResolver`], and the `_null`
//! `FieldMask` channel for emitting explicit `null` literals.
//!
//! The output is deterministic up to map iteration: map entries are sorted
//! lexicographically by their formatted key string, matching the Go and TS
//! ports byte-for-byte.

use prost::Message as _;
use prost_reflect::{
    DynamicMessage, FieldDescriptor, Kind, MapKey, MessageDescriptor, ReflectMessage, Value,
};
use std::collections::HashSet;
use std::sync::Arc;

use crate::annotations::find_null_mask_field;
use crate::bigfloat::format_big_float;
use crate::bignum::{format_big_int, format_decimal};
use crate::decode::{is_secret, shown_key, unmarshal_full, TypeResolver, UnmarshalOptions};
use crate::errors::PxfError;
use crate::keyed::{ident_safe_entry_name, key_field};
use crate::limits::{Limits, MAX_NESTING_DEPTH};
use crate::post_decode_plan::{post_decode_plan, PlanField};
use crate::result::Presence;
use crate::token::Position;

#[derive(Clone, Copy)]
pub struct MarshalOptions<'a> {
    /// Indent string per level. Defaults to two spaces.
    pub indent: &'a str,
    /// Emit fields whose value is the proto3 default (zero) instead of skipping them.
    pub emit_defaults: bool,
    /// When `Some`, prefix the output with `@type <type_url>` and a blank line.
    pub type_url: Option<&'a str>,
    /// Required to encode `google.protobuf.Any` with sugar syntax.
    pub type_resolver: Option<&'a dyn TypeResolver>,
    /// Alternative null source for messages without a top-level `_null`
    /// `FieldMask`. Paths in this [`Presence`] that are marked null are
    /// emitted as `null` literals.
    pub null_fields: Option<&'a Presence>,
    /// The deepest document written, counted as a decoder counts it: the
    /// root is depth 0, and every `{` or `[` is one descent. A message that
    /// needs more is an error, because a reader at the same limit refuses
    /// the document (draft -01 § Recursion, protowire#502). An Any past the
    /// limit is not: it stays as `type_url` and `value`. Defaults to
    /// [`MAX_NESTING_DEPTH`]; raise it only with
    /// `UnmarshalOptions::limits.max_nesting_depth` on every reader.
    pub max_nesting_depth: usize,
}

impl<'a> Default for MarshalOptions<'a> {
    fn default() -> Self {
        Self {
            indent: "  ",
            emit_defaults: false,
            type_url: None,
            type_resolver: None,
            null_fields: None,
            max_nesting_depth: MAX_NESTING_DEPTH,
        }
    }
}

/// Serialize `message` to a PXF text string under the given schema.
pub fn marshal(
    message: &DynamicMessage,
    desc: &MessageDescriptor,
    options: MarshalOptions<'_>,
) -> Result<String, PxfError> {
    let mut enc = Encoder::new(options);
    enc.prime_null_set(message, desc, options.null_fields);
    if let Some(url) = options.type_url {
        enc.buf.push_str("@type ");
        enc.buf.push_str(url);
        enc.buf.push_str("\n\n");
    }
    enc.encode_message(message, 0);
    match enc.error {
        Some(fault) => Err(PxfError::new(Position::default(), fault.message())),
        None => Ok(enc.buf),
    }
}

/// Why `marshal` has no document to return.
enum Fault {
    /// A value with no literal.
    NoLiteral(String),
    /// A block or list past the writer's `max_nesting_depth`.
    TooDeep(usize),
    /// An unset `(pxf.required)` field with presence: no text reads back as
    /// it, since a reader rejects it absent and rejects it `null`.
    RequiredUnset(String),
}

impl Fault {
    fn message(self) -> String {
        match self {
            Fault::NoLiteral(msg) => msg,
            Fault::TooDeep(limit) => {
                format!("pxf: nesting depth exceeds MaxNestingDepth={limit}")
            }
            Fault::RequiredUnset(field) => {
                format!("pxf: {field} is unset, and the field is (pxf.required)")
            }
        }
    }
}

struct Encoder<'a> {
    buf: String,
    indent: &'a str,
    emit_defaults: bool,
    resolver: Option<&'a dyn TypeResolver>,
    null_set: Option<HashSet<String>>,
    null_mask_fd: Option<FieldDescriptor>,
    path_prefix: String,
    /// Key field to omit from the NEXT `encode_message` call: the body of
    /// a keyed-block entry already carries the key as its entry name
    /// (draft -01 §3.13).
    skip_key_fd: Option<FieldDescriptor>,
    /// The deepest level a document may reach.
    max_depth: usize,
    /// The first value that has no literal, or the first block past
    /// `max_depth`. Encoding carries on, writing nothing for it (and not
    /// descending past the limit), and `marshal` returns this instead of
    /// the text.
    error: Option<Fault>,
}

impl<'a> Encoder<'a> {
    fn new(options: MarshalOptions<'a>) -> Self {
        Self {
            buf: String::new(),
            indent: options.indent,
            emit_defaults: options.emit_defaults,
            resolver: options.type_resolver,
            null_set: None,
            null_mask_fd: None,
            path_prefix: String::new(),
            skip_key_fd: None,
            max_depth: options.max_nesting_depth,
            error: None,
        }
    }

    /// Whether a block or list whose contents sit at `level` may be
    /// written; past the limit the refusal is recorded. The level of a body
    /// is the decoder's depth there (HARDENING.md § Recursion).
    fn check_depth(&mut self, level: usize) -> bool {
        if level > self.max_depth {
            self.error.get_or_insert(Fault::TooDeep(self.max_depth));
            return false;
        }
        true
    }

    /// `message_shorthand`, recording a value that has no literal.
    fn shorthand(&mut self, mdesc: &MessageDescriptor, sub: &DynamicMessage) -> Option<String> {
        match message_shorthand(mdesc, sub)? {
            Ok(text) => Some(text),
            Err(msg) => {
                self.error.get_or_insert(Fault::NoLiteral(msg));
                Some(String::new())
            }
        }
    }

    /// Discover the top-level `_null` `FieldMask` (if any) and snapshot its
    /// paths into a set. Falls back to `MarshalOptions.null_fields` for
    /// schemas that don't carry a `_null` field.
    fn prime_null_set(
        &mut self,
        root: &DynamicMessage,
        desc: &MessageDescriptor,
        fallback: Option<&Presence>,
    ) {
        self.null_mask_fd = find_null_mask_field(desc);
        if let Some(fd) = self.null_mask_fd.clone() {
            if root.has_field(&fd) {
                let mask = root.get_field(&fd);
                if let Value::Message(fm) = mask.as_ref() {
                    if let Some(paths_fd) = fm.descriptor().get_field_by_name("paths") {
                        if let Value::List(items) = fm.get_field(&paths_fd).into_owned() {
                            let mut set: HashSet<String> = HashSet::new();
                            for item in items {
                                if let Value::String(s) = item {
                                    set.insert(s);
                                }
                            }
                            self.null_set = Some(set);
                        }
                    }
                }
                return;
            }
        }
        if let Some(fb) = fallback {
            let set: HashSet<String> = fb.null_paths().map(|s| s.to_string()).collect();
            self.null_set = Some(set);
        }
    }

    /// Extend the path of the null lookup for the `i`th element of a list,
    /// as the decoder does, returning the path to restore. Only a null set
    /// reads the path.
    fn enter_index(&mut self, name: &str, i: usize) -> String {
        if self.null_set.is_none() {
            return String::new();
        }
        let saved = std::mem::take(&mut self.path_prefix);
        self.path_prefix = format!("{saved}{name}[{i}].");
        saved
    }

    /// [`Self::enter_index`] for the value of map key `key`.
    fn enter_key(&mut self, name: &str, key: &MapKey) -> String {
        if self.null_set.is_none() {
            return String::new();
        }
        let saved = std::mem::take(&mut self.path_prefix);
        self.path_prefix = format!("{saved}{name}[{}].", shown_key(key));
        saved
    }

    fn write_indent(&mut self, level: usize) {
        for _ in 0..level {
            self.buf.push_str(self.indent);
        }
    }

    fn write_field_prefix(&mut self, level: usize, name: &str) {
        self.write_indent(level);
        self.buf.push_str(name);
        self.buf.push_str(" = ");
    }

    fn encode_message(&mut self, parent: &DynamicMessage, level: usize) {
        let desc = parent.descriptor();
        // A pending skip applies to exactly this body.
        let skip_key = self.skip_key_fd.take();
        if !self.check_depth(level) {
            return;
        }
        // The fields' annotations, read at the first unset field.
        let mut plan: Option<Arc<[PlanField]>> = None;
        for fd in declaration_order(&desc) {
            if let Some(null_fd) = &self.null_mask_fd {
                if self.path_prefix.is_empty() && fd.number() == null_fd.number() {
                    continue;
                }
            }
            if let Some(k) = &skip_key {
                if fd.number() == k.number() {
                    continue;
                }
            }
            let path = format!("{}{}", self.path_prefix, fd.name());
            if self.null_set.as_ref().is_some_and(|s| s.contains(&path)) {
                self.write_field_prefix(level, fd.name());
                self.buf.push_str("null\n");
                continue;
            }

            if !parent.has_field(&fd) && !is_negative_zero(parent, &fd) {
                let plan = plan.get_or_insert_with(|| post_decode_plan(&desc));
                match unset_field(parent, &fd, plan) {
                    Unset::Skip if !self.emit_defaults => continue,
                    Unset::Null => {
                        self.write_field_prefix(level, fd.name());
                        self.buf.push_str("null\n");
                        continue;
                    }
                    Unset::Write if fd.is_list() || fd.is_map() => {
                        if !self.check_depth(level + 1) {
                            continue;
                        }
                        self.write_field_prefix(level, fd.name());
                        self.buf.push_str(if fd.is_map() { "{}\n" } else { "[]\n" });
                        continue;
                    }
                    Unset::Required if !self.emit_defaults => {
                        self.error
                            .get_or_insert(Fault::RequiredUnset(fd.full_name().to_string()));
                        continue;
                    }
                    // The zero value, written out below. With `emit_defaults`
                    // so is every other unset field, as before: an unset
                    // required field with presence then reads back set to
                    // zero, which a reader accepts.
                    _ => {}
                }
            }

            if fd.is_map() {
                self.encode_map_field(parent, &fd, level);
                continue;
            }
            if fd.is_list() {
                self.encode_list_field(parent, &fd, level);
                continue;
            }
            if let Kind::Message(_) = fd.kind() {
                if !parent.has_field(&fd) {
                    continue;
                }
                // Borrowed: a clone here would copy the whole subtree at
                // every level, recursively, before the depth check is
                // reached.
                let value = parent.get_field(&fd);
                let Value::Message(sub) = value.as_ref() else {
                    continue;
                };
                self.encode_message_field(&fd, sub, level);
                continue;
            }

            self.write_field_prefix(level, fd.name());
            self.write_scalar_or_enum(&fd, parent);
            self.buf.push('\n');
        }
    }

    fn encode_message_field(&mut self, fd: &FieldDescriptor, sub: &DynamicMessage, level: usize) {
        let mdesc = match fd.kind() {
            Kind::Message(m) => m,
            _ => return,
        };
        let full = crate::wkt::dispatch_name(&mdesc);

        if let Some(text) = self.shorthand(&mdesc, sub) {
            self.write_field_prefix(level, fd.name());
            self.buf.push_str(&text);
            self.buf.push('\n');
            return;
        }
        if full == "google.protobuf.Any"
            && self.resolver.is_some()
            && self.try_encode_any(fd, sub, level)
        {
            return;
        }

        self.write_indent(level);
        self.buf.push_str(fd.name());
        self.buf.push_str(" {\n");
        let saved = self.path_prefix.clone();
        self.path_prefix = format!("{}{}.", saved, fd.name());
        self.encode_message(sub, level + 1);
        self.path_prefix = saved;
        self.write_indent(level);
        self.buf.push_str("}\n");
    }

    fn encode_list_field(&mut self, parent: &DynamicMessage, fd: &FieldDescriptor, level: usize) {
        let value = parent.get_field(fd);
        let Value::List(list) = value.as_ref() else {
            return;
        };
        if list.is_empty() && !self.emit_defaults {
            return;
        }

        // Keyed repeated field (draft -01 §3.13): emit the keyed block form
        // whenever every element's key is present, non-empty and distinct;
        // otherwise the anonymous list form (elements with absent or
        // duplicate keys can only be represented anonymously).
        if let Some(key_fd) = key_field(fd) {
            if !list.is_empty() && keyed_form_eligible(list, &key_fd) {
                self.encode_keyed_list(fd, list, &key_fd, level);
                return;
            }
        }

        if !self.check_depth(level + 1) {
            return;
        }
        self.write_field_prefix(level, fd.name());
        self.buf.push_str("[\n");

        let element_kind = fd.kind();
        for (i, elem) in list.iter().enumerate() {
            self.write_indent(level + 1);
            match (&element_kind, elem) {
                (Kind::Message(mdesc), Value::Message(sub)) => {
                    if let Some(text) = self.shorthand(mdesc, sub) {
                        self.buf.push_str(&text);
                    } else {
                        self.buf.push_str("{\n");
                        let saved = self.enter_index(fd.name(), i);
                        self.encode_message(sub, level + 2);
                        self.path_prefix = saved;
                        self.write_indent(level + 1);
                        self.buf.push('}');
                    }
                }
                (Kind::Enum(_), Value::EnumNumber(n)) => {
                    self.write_enum_value(fd, *n);
                }
                (_, v) => {
                    self.write_scalar_value(&fd.kind(), v);
                }
            }
            if i + 1 < list.len() {
                self.buf.push(',');
            }
            self.buf.push('\n');
        }

        self.write_indent(level);
        self.buf.push_str("]\n");
    }

    /// Emit a keyed repeated field in the keyed block form: one named block
    /// per element, in list order. Entry names are written unquoted when
    /// identifier-safe and quoted otherwise; the key field is not
    /// additionally emitted inside the entry's block.
    fn encode_keyed_list(
        &mut self,
        fd: &FieldDescriptor,
        list: &[Value],
        key_fd: &FieldDescriptor,
        level: usize,
    ) {
        if !self.check_depth(level + 1) {
            return;
        }
        self.write_indent(level);
        self.buf.push_str(fd.name());
        self.buf.push_str(" {\n");
        for (i, elem) in list.iter().enumerate() {
            let Value::Message(sub) = elem else { continue };
            let key = match sub.get_field(key_fd).into_owned() {
                Value::String(s) => s,
                _ => String::new(),
            };
            self.write_indent(level + 1);
            if ident_safe_entry_name(&key) {
                self.buf.push_str(&key);
            } else {
                self.buf.push_str(&write_quoted_string(&key));
            }
            self.buf.push_str(" {\n");
            self.skip_key_fd = Some(key_fd.clone());
            let saved = self.enter_index(fd.name(), i);
            self.encode_message(sub, level + 2);
            self.path_prefix = saved;
            self.write_indent(level + 1);
            self.buf.push_str("}\n");
        }
        self.write_indent(level);
        self.buf.push_str("}\n");
    }

    fn encode_map_field(&mut self, parent: &DynamicMessage, fd: &FieldDescriptor, level: usize) {
        let value = parent.get_field(fd);
        let Value::Map(map) = value.as_ref() else {
            return;
        };
        if map.is_empty() && !self.emit_defaults {
            return;
        }

        let mdesc = match fd.kind() {
            Kind::Message(m) => m,
            _ => return,
        };
        let val_fd = mdesc.map_entry_value_field();

        if !self.check_depth(level + 1) {
            return;
        }
        self.write_field_prefix(level, fd.name());
        self.buf.push_str("{\n");

        let mut entries: Vec<(String, &MapKey, &Value)> =
            map.iter().map(|(k, v)| (format_map_key(k), k, v)).collect();
        entries.sort_by(|a, b| a.0.cmp(&b.0));

        for (key_str, key, val) in entries {
            self.write_indent(level + 1);
            self.buf.push_str(&key_str);
            self.buf.push_str(": ");
            match (&val_fd.kind(), &val) {
                (Kind::Message(mdesc), Value::Message(sub)) => {
                    // A message value takes the same shorthand it takes in
                    // singular and list positions, as the reference writes
                    // it (`alpha: 100`, `at: 2026-01-01T00:00:00Z`).
                    if let Some(text) = self.shorthand(mdesc, sub) {
                        self.buf.push_str(&text);
                        self.buf.push('\n');
                    } else {
                        self.buf.push_str("{\n");
                        let saved = self.enter_key(fd.name(), key);
                        self.encode_message(sub, level + 2);
                        self.path_prefix = saved;
                        self.write_indent(level + 1);
                        self.buf.push_str("}\n");
                    }
                }
                (Kind::Enum(_), Value::EnumNumber(n)) => {
                    self.write_enum_value(&val_fd, *n);
                    self.buf.push('\n');
                }
                _ => {
                    self.write_scalar_value(&val_fd.kind(), val);
                    self.buf.push('\n');
                }
            }
        }

        self.write_indent(level);
        self.buf.push_str("}\n");
    }

    fn write_scalar_or_enum(&mut self, fd: &FieldDescriptor, parent: &DynamicMessage) {
        if let Kind::Enum(_) = fd.kind() {
            let n = match parent.get_field(fd).into_owned() {
                Value::EnumNumber(n) => n,
                _ => 0,
            };
            self.write_enum_value(fd, n);
            return;
        }
        self.write_scalar_value_for(fd, parent);
    }

    fn write_scalar_value_for(&mut self, fd: &FieldDescriptor, parent: &DynamicMessage) {
        let v = parent.get_field(fd).into_owned();
        self.write_scalar_value(&fd.kind(), &v);
    }

    fn write_scalar_value(&mut self, kind: &Kind, v: &Value) {
        self.buf.push_str(&scalar_text(kind, v));
    }

    fn write_enum_value(&mut self, fd: &FieldDescriptor, num: i32) {
        if let Kind::Enum(enum_desc) = fd.kind() {
            if let Some(ev) = enum_desc.get_value(num) {
                self.buf.push_str(ev.name());
                return;
            }
            self.buf.push_str(&num.to_string());
        }
    }
}

/// The PXF text of a scalar value of `kind`.
fn scalar_text(kind: &Kind, v: &Value) -> String {
    match (kind, v) {
        (Kind::String, Value::String(s)) => write_quoted_string(s),
        (Kind::Bool, Value::Bool(b)) => (if *b { "true" } else { "false" }).to_string(),
        (Kind::Int32 | Kind::Sint32 | Kind::Sfixed32, Value::I32(n)) => n.to_string(),
        (Kind::Int64 | Kind::Sint64 | Kind::Sfixed64, Value::I64(n)) => n.to_string(),
        (Kind::Uint32 | Kind::Fixed32, Value::U32(n)) => n.to_string(),
        (Kind::Uint64 | Kind::Fixed64, Value::U64(n)) => n.to_string(),
        (Kind::Float, Value::F32(f)) => format_float_f32(*f),
        (Kind::Double, Value::F64(f)) => format_float_f64(*f),
        (Kind::Bytes, Value::Bytes(b)) => format!("b\"{}\"", encode_base64(b)),
        (Kind::Enum(_), Value::EnumNumber(_)) => {
            // Enum scalars route through write_enum_value; reaching here
            // means the caller got the dispatch wrong.
            "0".to_string()
        }
        _ => "?".to_string(),
    }
}

/// The single-literal form of a message value, for the message types that
/// have one: `google.protobuf.Timestamp` (RFC 3339), `google.protobuf
/// .Duration` (Go-style), the nine `*Value` wrappers (the bare scalar),
/// `pxf.BigInt` (integer), `pxf.Decimal` (decimal literal, scale
/// preserved) and `pxf.BigFloat` (the reference's `%g` rendering). `None`
/// for every other message, which is written as a block. Used in singular, list and map-value positions alike, so the
/// three agree with each other and with the reference.
fn message_shorthand(
    mdesc: &MessageDescriptor,
    sub: &DynamicMessage,
) -> Option<Result<String, String>> {
    // A message with a well-known type's name and not its shape is written
    // as a block (draft -01 §wkt-shapes): its name matches none here.
    let full = crate::wkt::dispatch_name(mdesc);
    if full == "google.protobuf.Timestamp" {
        return Some(Ok(format_rfc3339_nano(sub)));
    }
    // A Duration past the literal's range, a signed 64-bit count of
    // nanoseconds, is written as a block with its exact seconds and nanos
    // (draft -01 § Durations).
    if full == "google.protobuf.Duration" && duration_fits_literal(sub) {
        return Some(Ok(format_go_duration(sub)));
    }
    if is_wrapper_full_name(full) {
        let inner_fd = mdesc
            .get_field_by_name("value")
            .expect("wrapper missing 'value'");
        let v = sub.get_field(&inner_fd).into_owned();
        return Some(Ok(scalar_text(&inner_fd.kind(), &v)));
    }
    // A pxf.Secret is its value's string literal, unless it carries a hint
    // or fingerprint, which the literal would drop (draft -01 § pxf.Secret).
    if is_secret(mdesc) && !secret_has_metadata(sub) {
        let value_fd = mdesc
            .get_field_by_name("value")
            .expect("checked by is_secret");
        let v = sub.get_field(&value_fd).into_owned();
        return Some(Ok(scalar_text(&Kind::String, &v)));
    }
    if full == "pxf.BigInt" {
        let abs = bytes_field(sub, "abs");
        let negative = bool_field(sub, "negative");
        return Some(format_big_int(&abs, negative));
    }
    if full == "pxf.Decimal" {
        let unscaled = bytes_field(sub, "unscaled");
        let scale = match sub
            .descriptor()
            .get_field_by_name("scale")
            .map(|fd| sub.get_field(&fd).into_owned())
        {
            Some(Value::I32(n)) => n,
            _ => 0,
        };
        let negative = bool_field(sub, "negative");
        return Some(format_decimal(&unscaled, scale, negative));
    }
    if full == "pxf.BigFloat" {
        let mantissa = bytes_field(sub, "mantissa");
        let exponent = match sub
            .descriptor()
            .get_field_by_name("exponent")
            .map(|fd| sub.get_field(&fd).into_owned())
        {
            Some(Value::I32(n)) => n,
            _ => 0,
        };
        let prec = match sub
            .descriptor()
            .get_field_by_name("prec")
            .map(|fd| sub.get_field(&fd).into_owned())
        {
            Some(Value::U32(n)) => n,
            _ => 0,
        };
        let negative = bool_field(sub, "negative");
        // A message the literal form cannot render fails, as the reference
        // does: a producer never writes a value past a bound.
        return Some(format_big_float(&mantissa, exponent, prec, negative));
    }
    None
}

/// Whether a `pxf.Secret` sets a `hint` or a `fingerprint`.
fn secret_has_metadata(secret: &DynamicMessage) -> bool {
    ["hint", "fingerprint"].iter().any(|name| {
        secret
            .get_field_by_name(name)
            .is_some_and(|v| v.as_str().is_some_and(|s| !s.is_empty()))
    })
}

fn bytes_field(msg: &DynamicMessage, name: &str) -> Vec<u8> {
    match msg
        .descriptor()
        .get_field_by_name(name)
        .map(|fd| msg.get_field(&fd).into_owned())
    {
        Some(Value::Bytes(b)) => b.to_vec(),
        _ => Vec::new(),
    }
}

fn bool_field(msg: &DynamicMessage, name: &str) -> bool {
    matches!(
        msg.descriptor()
            .get_field_by_name(name)
            .map(|fd| msg.get_field(&fd).into_owned()),
        Some(Value::Bool(true))
    )
}

impl<'a> Encoder<'a> {
    /// Try Any sugar; returns true on success. When the resolver can't find
    /// the URL or the bytes don't decode, returns false so the caller falls
    /// back to the plain `{ type_url = …, value = … }` block path.
    fn try_encode_any(
        &mut self,
        fd: &FieldDescriptor,
        any_msg: &DynamicMessage,
        level: usize,
    ) -> bool {
        let any_desc = any_msg.descriptor();
        let Some(type_url_fd) = any_desc.get_field_by_name("type_url") else {
            return false;
        };
        let Some(value_fd) = any_desc.get_field_by_name("value") else {
            return false;
        };

        let url = match any_msg.get_field(&type_url_fd).into_owned() {
            Value::String(s) => s,
            _ => return false,
        };
        if url.is_empty() || level >= self.max_depth {
            return false;
        }
        let bytes = match any_msg.get_field(&value_fd).into_owned() {
            Value::Bytes(b) => b,
            _ => return false,
        };

        let resolver = self.resolver.expect("resolver checked at call site");
        let Some(inner_desc) = resolver.find_message_by_url(&url) else {
            return false;
        };
        let inner = match DynamicMessage::decode(inner_desc.clone(), &bytes[..]) {
            Ok(m) => m,
            Err(_) => return false,
        };

        // The body goes to a buffer of its own: an Any whose value is too
        // deep to unpack here, or whose body would not read back as the
        // same bytes, is written as its own fields instead.
        let out = std::mem::take(&mut self.buf);
        let prior = self.error.take();
        let saved = self.path_prefix.clone();
        self.path_prefix = format!("{}{}.", saved, fd.name());
        self.encode_message(&inner, level + 1);
        self.path_prefix = saved;
        let body = std::mem::replace(&mut self.buf, out);
        let fault = std::mem::replace(&mut self.error, prior);
        match fault {
            // The value is too deep to unpack here, or its body would be
            // rejected for an unset required field; its bytes are neither.
            Some(Fault::TooDeep(_) | Fault::RequiredUnset(_)) => return false,
            Some(f) => {
                self.error.get_or_insert(f);
                return true;
            }
            None => {}
        }
        if !self.any_body_reads_back(&body, &inner_desc, level, &bytes) {
            return false;
        }
        self.write_indent(level);
        self.buf.push_str(fd.name());
        self.buf.push_str(" {\n");
        self.write_indent(level + 1);
        self.buf.push_str("@type = ");
        self.buf.push_str(&write_quoted_string(&url));
        self.buf.push('\n');
        self.buf.push_str(&body);
        self.write_indent(level);
        self.buf.push_str("}\n");
        true
    }

    /// Whether `body`, the entries of an Any block whose own entry sits at
    /// `level`, decodes to exactly `want`, the bytes of the Any's value
    /// (draft -01 § Any Blocks, protowire#501). A reader compares an Any by
    /// its bytes, and a body loses what text cannot say at any level of the
    /// value: unknown fields, field or map order, an explicitly written
    /// zero, a Timestamp no literal holds; and a reader's post-decode pass
    /// fills an unset `@default` or rejects an unset `@required`. So the
    /// reader decides: the body is decoded as `unmarshal_full` would, with
    /// the nesting it has left at that depth, re-encoded canonically, and
    /// compared. Mirrors protowire-go's `anyBodyReadsBack`.
    fn any_body_reads_back(
        &self,
        body: &str,
        desc: &MessageDescriptor,
        level: usize,
        want: &[u8],
    ) -> bool {
        // The body's entries sit at level + 1; a document's at 0.
        let Some(max_nesting_depth) = self.max_depth.checked_sub(level + 1) else {
            return false;
        };
        let options = UnmarshalOptions {
            type_resolver: self.resolver,
            limits: Limits {
                max_nesting_depth,
                ..Limits::default()
            },
            ..Default::default()
        };
        let Ok((got, _)) = unmarshal_full(body, desc, options) else {
            return false;
        };
        let mut packed = Vec::with_capacity(want.len());
        encode_canonical(&got, &mut packed);
        packed == want
    }
}

/// Append `msg`'s wire bytes with map entries in key order, as protobuf-go's
/// deterministic marshal and protoc write them; prost-reflect keeps a map
/// in a `HashMap` and writes it in that order, which differs from run to
/// run. Fields go in number order. Unknown fields are not written: a
/// message decoded from text has none. Used for an Any's value, which a
/// reader compares by its bytes, both where the decoder packs a body and
/// where the encoder checks that one reads back.
pub(crate) fn encode_canonical(msg: &DynamicMessage, out: &mut Vec<u8>) {
    let desc = msg.descriptor();
    let mut fields: Vec<(FieldDescriptor, &Value)> = msg.fields().collect();
    // `fields()` leaves out a float or double holding -0.0: prost-reflect
    // takes it for the proto3 default, comparing by `==`
    // (andrewhickman/prost-reflect#208). protobuf-go writes it (#98).
    if let Some(dm) = negative_zero_fields(msg) {
        fields.extend(dm);
    }
    fields.sort_by_key(|(fd, _)| fd.number());
    for (fd, value) in fields {
        match (value, fd.kind()) {
            (Value::Map(map), Kind::Message(entry_desc)) => {
                let key_fd = entry_desc.map_entry_key_field();
                let value_fd = entry_desc.map_entry_value_field();
                let mut entries: Vec<_> = map.iter().collect();
                entries.sort_by(|a, b| a.0.cmp(b.0));
                for (k, v) in entries {
                    let mut entry = Vec::new();
                    let mut key = DynamicMessage::new(entry_desc.clone());
                    key.set_field(&key_fd, Value::from(k.clone()));
                    // A map entry carries its key and value whether or not
                    // they are zero, as every protobuf encoder writes it.
                    write_field_bytes(&key, &key_fd, &mut entry);
                    match v {
                        Value::Message(sub) => write_message_bytes(2, sub, &mut entry),
                        _ => {
                            let mut val = DynamicMessage::new(entry_desc.clone());
                            val.set_field(&value_fd, v.clone());
                            write_field_bytes(&val, &value_fd, &mut entry);
                        }
                    }
                    prost::encoding::encode_key(
                        fd.number(),
                        prost::encoding::WireType::LengthDelimited,
                        out,
                    );
                    prost::encoding::encode_varint(entry.len() as u64, out);
                    out.extend_from_slice(&entry);
                }
            }
            (Value::List(items), Kind::Message(_)) => {
                for item in items {
                    if let Value::Message(sub) = item {
                        write_message_bytes(fd.number(), sub, out);
                    }
                }
            }
            (Value::Message(sub), _) => write_message_bytes(fd.number(), sub, out),
            _ => {
                if write_negative_zero(&fd, value, out) {
                    continue;
                }
                let mut one = DynamicMessage::new(desc.clone());
                one.set_field(&fd, value.clone());
                one.encode_raw(out);
            }
        }
    }
}

/// The singular float and double fields of `msg` that hold -0.0, which
/// `DynamicMessage::fields` leaves out; `None` when there are none.
fn negative_zero_fields(msg: &DynamicMessage) -> Option<Vec<(FieldDescriptor, &Value)>> {
    let mut out = Vec::new();
    for fd in msg.descriptor().fields() {
        if fd.is_list() || fd.is_map() || msg.has_field(&fd) {
            continue;
        }
        if !matches!(fd.kind(), Kind::Float | Kind::Double) {
            continue;
        }
        // Stored values come back borrowed; an absent one is the default.
        if let std::borrow::Cow::Borrowed(v) = msg.get_field(&fd) {
            if is_negative_zero_value(v) {
                out.push((fd, v));
            }
        }
    }
    (!out.is_empty()).then_some(out)
}

fn is_negative_zero_value(v: &Value) -> bool {
    match v {
        Value::F64(x) => *x == 0.0 && x.is_sign_negative(),
        Value::F32(x) => *x == 0.0 && x.is_sign_negative(),
        _ => false,
    }
}

/// Writes `value` as field `fd` if it is a float or double -0.0, which
/// prost-reflect's encoder drops; returns whether it did.
fn write_negative_zero(fd: &FieldDescriptor, value: &Value, out: &mut Vec<u8>) -> bool {
    use prost::encoding::{encode_key, WireType};
    match value {
        Value::F64(x) if is_negative_zero_value(value) => {
            encode_key(fd.number(), WireType::SixtyFourBit, out);
            out.extend_from_slice(&x.to_bits().to_le_bytes());
            true
        }
        Value::F32(x) if is_negative_zero_value(value) => {
            encode_key(fd.number(), WireType::ThirtyTwoBit, out);
            out.extend_from_slice(&x.to_bits().to_le_bytes());
            true
        }
        _ => false,
    }
}

/// Write `field` of `msg` whether or not its value is the default: proto3
/// leaves a zero out of a message, but a map entry carries its key and
/// value either way, as every protobuf encoder writes it.
fn write_field_bytes(msg: &DynamicMessage, field: &FieldDescriptor, out: &mut Vec<u8>) {
    use prost::encoding::{encode_key, WireType};
    if write_negative_zero(field, &msg.get_field(field), out) {
        return;
    }
    let before = out.len();
    msg.encode_raw(out);
    if out.len() > before {
        return;
    }
    let (wire_type, zero): (WireType, &[u8]) = match field.kind() {
        Kind::String | Kind::Bytes => (WireType::LengthDelimited, &[0]),
        Kind::Float | Kind::Fixed32 | Kind::Sfixed32 => (WireType::ThirtyTwoBit, &[0; 4]),
        Kind::Double | Kind::Fixed64 | Kind::Sfixed64 => (WireType::SixtyFourBit, &[0; 8]),
        _ => (WireType::Varint, &[0]),
    };
    encode_key(field.number(), wire_type, out);
    out.extend_from_slice(zero);
}

/// A length-delimited submessage at `number`, written canonically.
fn write_message_bytes(number: u32, sub: &DynamicMessage, out: &mut Vec<u8>) {
    let mut body = Vec::new();
    encode_canonical(sub, &mut body);
    prost::encoding::encode_key(number, prost::encoding::WireType::LengthDelimited, out);
    prost::encoding::encode_varint(body.len() as u64, out);
    out.extend_from_slice(&body);
}

// ---------------------------------------------------------------------------
// Formatting helpers
// ---------------------------------------------------------------------------

const HEX: &[u8; 16] = b"0123456789abcdef";

fn write_quoted_string(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 2);
    out.push('"');
    for ch in s.chars() {
        match ch {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if (c as u32) < 0x20 => {
                let n = c as u32;
                out.push_str("\\x");
                out.push(HEX[((n >> 4) & 0xf) as usize] as char);
                out.push(HEX[(n & 0xf) as usize] as char);
            }
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

fn format_float_f32(f: f32) -> String {
    if f.is_nan() {
        return "nan".to_string();
    }
    if f.is_infinite() {
        return if f > 0.0 { "inf" } else { "-inf" }.to_string();
    }
    go_shortest_g(&format!("{f:e}"))
}

fn format_float_f64(f: f64) -> String {
    if f.is_nan() {
        return "nan".to_string();
    }
    if f.is_infinite() {
        return if f > 0.0 { "inf" } else { "-inf" }.to_string();
    }
    go_shortest_g(&format!("{f:e}"))
}

/// Whether `fd` holds a float or double negative zero that `has_field`
/// reports unset. prost-reflect compares a proto3 scalar with its default
/// by `==`, and `-0.0 == 0.0`, so a decoded `-0` looks absent although the
/// message keeps it. protobuf-go compares the bits, so protowire-go writes
/// `f = -0`; leaving it out would read back as `+0`.
fn is_negative_zero(parent: &DynamicMessage, fd: &FieldDescriptor) -> bool {
    if fd.is_list() || fd.is_map() {
        return false;
    }
    match parent.get_field(fd).as_ref() {
        Value::F64(v) => *v == 0.0 && v.is_sign_negative(),
        Value::F32(v) => *v == 0.0 && v.is_sign_negative(),
        _ => false,
    }
}

/// Lays out a finite float as Go's `strconv.FormatFloat(f, 'g', -1, bits)`
/// does, which is what protowire-go writes. `sci` is Rust's `{:e}` of the
/// value: the same shortest round-trip digits, as `d.ddde<exp>`.
///
/// Go uses the exponent form when the decimal exponent is below -4 or at
/// least 6 (`1e+21`, `1.5e-07`, `1.234567e+06`), with a sign and at least
/// two exponent digits; otherwise it writes the digits plainly (`100000`,
/// `0.0001`). Rust's `{}` never uses an exponent, so it wrote
/// `1000000000000000000000` where the reference writes `1e+21`.
fn go_shortest_g(sci: &str) -> String {
    let (neg, rest) = match sci.strip_prefix('-') {
        Some(r) => (true, r),
        None => (false, sci),
    };
    let (mant, exp) = rest
        .split_once('e')
        .expect("{:e} of a finite float has an exponent");
    let exp: i32 = exp.parse().expect("{:e} exponent is an integer");
    let digits: String = mant.chars().filter(|c| *c != '.').collect();
    let mut out = String::with_capacity(digits.len() + 8);
    if neg {
        out.push('-');
    }
    if !(-4..6).contains(&exp) {
        out.push_str(&digits[..1]);
        if digits.len() > 1 {
            out.push('.');
            out.push_str(&digits[1..]);
        }
        out.push('e');
        out.push(if exp < 0 { '-' } else { '+' });
        let a = exp.unsigned_abs();
        if a < 10 {
            out.push('0');
        }
        out.push_str(&a.to_string());
    } else if exp < 0 {
        out.push_str("0.");
        for _ in 0..(-exp - 1) {
            out.push('0');
        }
        out.push_str(&digits);
    } else {
        let int_len = exp as usize + 1;
        if digits.len() <= int_len {
            out.push_str(&digits);
            for _ in digits.len()..int_len {
                out.push('0');
            }
        } else {
            out.push_str(&digits[..int_len]);
            out.push('.');
            out.push_str(&digits[int_len..]);
        }
    }
    out
}

/// How `marshal` writes a field the message does not set, under the
/// field's annotations (protowire-go's `unsetField`, protowire-go#259). A
/// reader applies `(pxf.default)` to an absent field and checks
/// `(pxf.required)`, so leaving an annotated field out would not read back
/// as the message.
enum Unset {
    /// Leave it out: it has no annotation, or no text would read back as it.
    Skip,
    /// Write its zero value, `[]` or `{}`: it has no presence, so the zero
    /// counts as supplied and the default does not replace it.
    Write,
    /// Write `null`: a message-typed field with a default, which `null`
    /// keeps unset and keeps the default off.
    Null,
    /// Refuse the message: an unset required field with presence.
    Required,
}

fn unset_field(parent: &DynamicMessage, fd: &FieldDescriptor, plan: &[PlanField]) -> Unset {
    let Ok(i) = plan.binary_search_by_key(&fd.number(), |p| p.number) else {
        return Unset::Skip;
    };
    let (required, has_default) = (plan[i].required, plan[i].default.is_some());
    if !required && !has_default {
        return Unset::Skip;
    }
    if !fd.supports_presence() {
        return Unset::Write;
    }
    if required {
        return Unset::Required;
    }
    // A set sibling keeps the default off, so the member reads back unset.
    if let Some(oneof) = fd.containing_oneof() {
        if !oneof.is_synthetic() && oneof.fields().any(|m| parent.has_field(&m)) {
            return Unset::Skip;
        }
    }
    // An unset `optional` scalar with a default stays out and reads back as
    // its default: the owner's choice on protowire-go#267, which draft -01
    // § Common Schema Layer names as an exception to the round trip.
    if matches!(fd.kind(), Kind::Message(_)) {
        Unset::Null
    } else {
        Unset::Skip
    }
}

/// `desc`'s fields in the order the schema declares them, as protowire-go
/// writes them (protoreflect's `Fields()`). prost-reflect's `fields()` goes
/// in number order, which moves a field declared after a higher-numbered
/// one, such as a oneof numbered past the fields that follow it.
fn declaration_order(desc: &MessageDescriptor) -> impl Iterator<Item = FieldDescriptor> + '_ {
    desc.descriptor_proto()
        .field
        .iter()
        .filter_map(|f| desc.get_field(f.number() as u32))
}

const B64_ALPHABET: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";

fn encode_base64(bytes: &[u8]) -> String {
    let mut out = String::with_capacity(bytes.len().div_ceil(3) * 4);
    let mut i = 0;
    while i + 3 <= bytes.len() {
        let triple =
            ((bytes[i] as u32) << 16) | ((bytes[i + 1] as u32) << 8) | (bytes[i + 2] as u32);
        out.push(B64_ALPHABET[((triple >> 18) & 0x3f) as usize] as char);
        out.push(B64_ALPHABET[((triple >> 12) & 0x3f) as usize] as char);
        out.push(B64_ALPHABET[((triple >> 6) & 0x3f) as usize] as char);
        out.push(B64_ALPHABET[(triple & 0x3f) as usize] as char);
        i += 3;
    }
    let rem = bytes.len() - i;
    if rem == 1 {
        let triple = (bytes[i] as u32) << 16;
        out.push(B64_ALPHABET[((triple >> 18) & 0x3f) as usize] as char);
        out.push(B64_ALPHABET[((triple >> 12) & 0x3f) as usize] as char);
        out.push('=');
        out.push('=');
    } else if rem == 2 {
        let triple = ((bytes[i] as u32) << 16) | ((bytes[i + 1] as u32) << 8);
        out.push(B64_ALPHABET[((triple >> 18) & 0x3f) as usize] as char);
        out.push(B64_ALPHABET[((triple >> 12) & 0x3f) as usize] as char);
        out.push(B64_ALPHABET[((triple >> 6) & 0x3f) as usize] as char);
        out.push('=');
    }
    out
}

/// Whether every element of `list` has a non-empty key value that is
/// distinct within the collection — the draft -01 §3.13 condition for
/// emitting the keyed block form.
fn keyed_form_eligible(list: &[Value], key_fd: &FieldDescriptor) -> bool {
    let mut seen = HashSet::with_capacity(list.len());
    for elem in list {
        let Value::Message(sub) = elem else {
            return false;
        };
        let key = match sub.get_field(key_fd).into_owned() {
            Value::String(s) => s,
            _ => return false,
        };
        if key.is_empty() || !seen.insert(key) {
            return false;
        }
    }
    true
}

/// Whether a string map key is identifier-safe: it matches the identifier
/// production (ident-start followed by ident-part bytes, dots included)
/// and is not one of the value keywords `null` / `true` / `false`, which
/// bare are no key at all or a bool key. The marshaller writes a string
/// map key bare exactly when this holds, and the formatter unquotes a
/// quoted one under the same test, so the two agree on every key both can
/// produce (protowire#306). It is the test keyed entry names use — one
/// identifier-safe rule for the document, as draft -01 § Entries and Keys
/// defines it (protowire#313; the alphabet used to stop at `[A-Za-z0-9_]`,
/// so `"a.b"` was written quoted).
fn is_valid_ident(s: &str) -> bool {
    ident_safe_entry_name(s)
}

fn format_map_key(k: &MapKey) -> String {
    match k {
        MapKey::String(s) => {
            if is_valid_ident(s) {
                s.clone()
            } else {
                write_quoted_string(s)
            }
        }
        MapKey::Bool(b) => if *b { "true" } else { "false" }.to_string(),
        MapKey::I32(n) => n.to_string(),
        MapKey::I64(n) => n.to_string(),
        MapKey::U32(n) => n.to_string(),
        MapKey::U64(n) => n.to_string(),
    }
}

fn is_wrapper_full_name(full: &str) -> bool {
    matches!(
        full,
        "google.protobuf.DoubleValue"
            | "google.protobuf.FloatValue"
            | "google.protobuf.Int64Value"
            | "google.protobuf.UInt64Value"
            | "google.protobuf.Int32Value"
            | "google.protobuf.UInt32Value"
            | "google.protobuf.BoolValue"
            | "google.protobuf.StringValue"
            | "google.protobuf.BytesValue"
    )
}

/// Format a Timestamp message as RFC 3339 with optional fractional seconds
/// (no trailing zeros), matching Go's `time.Format(time.RFC3339Nano)`.
fn format_rfc3339_nano(ts: &DynamicMessage) -> String {
    let s_fd = ts.descriptor().get_field_by_name("seconds");
    let n_fd = ts.descriptor().get_field_by_name("nanos");
    let seconds = s_fd
        .as_ref()
        .map(|f| match ts.get_field(f).into_owned() {
            Value::I64(s) => s,
            _ => 0,
        })
        .unwrap_or(0);
    let nanos = n_fd
        .as_ref()
        .map(|f| match ts.get_field(f).into_owned() {
            Value::I32(n) => n,
            _ => 0,
        })
        .unwrap_or(0);

    let days = seconds.div_euclid(86_400);
    let secs_in_day = seconds.rem_euclid(86_400);
    let (year, month, day) = civil_from_days(days);
    let hour = (secs_in_day / 3600) as u32;
    let minute = ((secs_in_day % 3600) / 60) as u32;
    let second = (secs_in_day % 60) as u32;

    let date = format!(
        "{:04}-{:02}-{:02}T{:02}:{:02}:{:02}",
        year, month, day, hour, minute, second
    );
    if nanos == 0 {
        return format!("{}Z", date);
    }
    let abs = nanos.unsigned_abs();
    let frac_full = format!("{:09}", abs);
    let frac = frac_full.trim_end_matches('0');
    format!("{}.{}Z", date, frac)
}

/// Inverse of `days_from_civil`: convert days since 1970-01-01 to (year,
/// month, day) in the proleptic Gregorian calendar (Howard Hinnant).
fn civil_from_days(z: i64) -> (i32, u32, u32) {
    let z = z + 719_468;
    let era = if z >= 0 {
        z / 146_097
    } else {
        (z - 146_096) / 146_097
    };
    let doe = (z - era * 146_097) as u32;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe as i32 + (era * 400) as i32;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = if m <= 2 { y + 1 } else { y };
    (y, m, d)
}

const NANOS_PER_SECOND: i128 = 1_000_000_000;
const NANOS_PER_MICRO: i128 = 1_000;
const NANOS_PER_MILLI: i128 = 1_000_000;

/// Format a Duration message as a Go-style duration string. Mirrors
/// `time.Duration.String()`: leading-zero `h`/`m` units omitted, sub-second
/// durations use the smallest unit (`ns` / `µs` / `ms`) that gives a non-zero
/// leading digit, and `0s` is the canonical zero.
/// Whether a Duration's value, seconds × 10^9 + nanos, fits the literal's
/// signed 64-bit count of nanoseconds.
fn duration_fits_literal(d: &DynamicMessage) -> bool {
    let int = |name: &str| match d.get_field_by_name(name).map(|v| v.into_owned()) {
        Some(Value::I64(n)) => i128::from(n),
        Some(Value::I32(n)) => i128::from(n),
        _ => 0,
    };
    let total = int("seconds") * 1_000_000_000 + int("nanos");
    i64::try_from(total).is_ok()
}

fn format_go_duration(d: &DynamicMessage) -> String {
    let s_fd = d.descriptor().get_field_by_name("seconds");
    let n_fd = d.descriptor().get_field_by_name("nanos");
    let seconds = s_fd
        .as_ref()
        .map(|f| match d.get_field(f).into_owned() {
            Value::I64(s) => s,
            _ => 0,
        })
        .unwrap_or(0);
    let nanos = n_fd
        .as_ref()
        .map(|f| match d.get_field(f).into_owned() {
            Value::I32(n) => n,
            _ => 0,
        })
        .unwrap_or(0);

    let mut total: i128 = (seconds as i128) * NANOS_PER_SECOND + (nanos as i128);
    if total == 0 {
        return "0s".to_string();
    }
    let neg = total < 0;
    if neg {
        total = -total;
    }

    let body = if total < NANOS_PER_SECOND {
        if total < NANOS_PER_MICRO {
            format!("{}ns", total)
        } else if total < NANOS_PER_MILLI {
            format!("{}µs", trim_fraction(total, NANOS_PER_MICRO))
        } else {
            format!("{}ms", trim_fraction(total, NANOS_PER_MILLI))
        }
    } else {
        let secs_part = total / NANOS_PER_SECOND;
        let frac_nanos = total % NANOS_PER_SECOND;
        let sec = secs_part % 60;
        let min_total = secs_part / 60;
        let minute = min_total % 60;
        let hour = min_total / 60;

        let sec_str = trim_fraction(sec * NANOS_PER_SECOND + frac_nanos, NANOS_PER_SECOND);
        if hour > 0 {
            format!("{}h{}m{}s", hour, minute, sec_str)
        } else if minute > 0 {
            format!("{}m{}s", minute, sec_str)
        } else {
            format!("{}s", sec_str)
        }
    };
    if neg {
        format!("-{}", body)
    } else {
        body
    }
}

/// Format `value / unit` with up to (digits-of-unit-1) fractional places, with
/// trailing zeros trimmed. Returns `"5"` not `"5.000"`.
fn trim_fraction(value: i128, unit: i128) -> String {
    let whole = value / unit;
    let remainder = value % unit;
    if remainder == 0 {
        return whole.to_string();
    }
    let unit_str = unit.to_string();
    let frac_digits = unit_str.len() - 1;
    let rem_str = format!("{:0>width$}", remainder, width = frac_digits);
    let trimmed = rem_str.trim_end_matches('0');
    format!("{}.{}", whole, trimmed)
}

#[cfg(test)]
mod float_text_tests {
    use super::{format_float_f32, format_float_f64};

    // Each right-hand side is what protowire-go writes:
    // strconv.FormatFloat(v, 'g', -1, bits), printed by Go 1.25.
    #[test]
    fn doubles_are_written_as_protowire_go_writes_them() {
        let cases: &[(f64, &str)] = &[
            (0.0, "0"),
            (-0.0, "-0"),
            (1.0, "1"),
            (-1.0, "-1"),
            (0.1, "0.1"),
            (0.0001, "0.0001"),
            (0.00001, "1e-05"),
            (1.5e-7, "1.5e-07"),
            (100000.0, "100000"),
            (999999.0, "999999"),
            (1e6, "1e+06"),
            (1234567.0, "1.234567e+06"),
            (123456.7, "123456.7"),
            (1e21, "1e+21"),
            (1e-7, "1e-07"),
            (5e-324, "5e-324"),
            (f64::MAX, "1.7976931348623157e+308"),
            (-2.5e-5, "-2.5e-05"),
            (0.000123, "0.000123"),
            (1.23456, "1.23456"),
            (1e100, "1e+100"),
            (12345678901234567890.0, "1.2345678901234567e+19"),
            (f64::INFINITY, "inf"),
            (f64::NEG_INFINITY, "-inf"),
            (f64::NAN, "nan"),
        ];
        for (v, want) in cases {
            assert_eq!(format_float_f64(*v), *want, "{v:?}");
        }
    }

    #[test]
    fn floats_are_written_as_protowire_go_writes_them() {
        let cases: &[(f32, &str)] = &[
            (1e10, "1e+10"),
            (1e-6, "1e-06"),
            (0.1, "0.1"),
            (16777216.0, "1.6777216e+07"),
            (f32::MAX, "3.4028235e+38"),
            (f32::MIN_POSITIVE, "1.1754944e-38"),
            (1.4e-45, "1e-45"),
            (123456.7, "123456.7"),
        ];
        for (v, want) in cases {
            assert_eq!(format_float_f32(*v), *want, "{v:?}");
        }
    }
}
