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

use prost_reflect::{
    DynamicMessage, FieldDescriptor, Kind, MapKey, MessageDescriptor, ReflectMessage, Value,
};
use std::collections::HashSet;

use crate::annotations::find_null_mask_field;
use crate::decode::TypeResolver;
use crate::result::Presence;

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
}

impl<'a> Default for MarshalOptions<'a> {
    fn default() -> Self {
        Self {
            indent: "  ",
            emit_defaults: false,
            type_url: None,
            type_resolver: None,
            null_fields: None,
        }
    }
}

/// Serialize `message` to a PXF text string under the given schema.
pub fn marshal(
    message: &DynamicMessage,
    desc: &MessageDescriptor,
    options: MarshalOptions<'_>,
) -> String {
    let mut enc = Encoder::new(options);
    enc.prime_null_set(message, desc, options.null_fields);
    if let Some(url) = options.type_url {
        enc.buf.push_str("@type ");
        enc.buf.push_str(url);
        enc.buf.push_str("\n\n");
    }
    enc.encode_message(message, 0);
    enc.buf
}

struct Encoder<'a> {
    buf: String,
    indent: &'a str,
    emit_defaults: bool,
    resolver: Option<&'a dyn TypeResolver>,
    null_set: Option<HashSet<String>>,
    null_mask_fd: Option<FieldDescriptor>,
    path_prefix: String,
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
        for fd in desc.fields() {
            if let Some(null_fd) = &self.null_mask_fd {
                if self.path_prefix.is_empty() && fd.number() == null_fd.number() {
                    continue;
                }
            }
            let path = format!("{}{}", self.path_prefix, fd.name());
            if self
                .null_set
                .as_ref()
                .is_some_and(|s| s.contains(&path))
            {
                self.write_field_prefix(level, fd.name());
                self.buf.push_str("null\n");
                continue;
            }

            if !self.emit_defaults && !parent.has_field(&fd) {
                continue;
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
                let sub = match parent.get_field(&fd).into_owned() {
                    Value::Message(m) => m,
                    _ => continue,
                };
                self.encode_message_field(&fd, &sub, level);
                continue;
            }

            self.write_field_prefix(level, fd.name());
            self.write_scalar_or_enum(&fd, parent);
            self.buf.push('\n');
        }
    }

    fn encode_message_field(
        &mut self,
        fd: &FieldDescriptor,
        sub: &DynamicMessage,
        level: usize,
    ) {
        let mdesc = match fd.kind() {
            Kind::Message(m) => m,
            _ => return,
        };
        let full = mdesc.full_name();

        if full == "google.protobuf.Timestamp" {
            self.write_field_prefix(level, fd.name());
            self.buf.push_str(&format_rfc3339_nano(sub));
            self.buf.push('\n');
            return;
        }
        if full == "google.protobuf.Duration" {
            self.write_field_prefix(level, fd.name());
            self.buf.push_str(&format_go_duration(sub));
            self.buf.push('\n');
            return;
        }
        if is_wrapper_full_name(full) {
            let inner_fd = mdesc
                .get_field_by_name("value")
                .expect("wrapper missing 'value'");
            self.write_field_prefix(level, fd.name());
            self.write_scalar_value_for(&inner_fd, sub);
            self.buf.push('\n');
            return;
        }
        if full == "google.protobuf.Any" && self.resolver.is_some() && self.try_encode_any(fd, sub, level) {
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

    fn encode_list_field(
        &mut self,
        parent: &DynamicMessage,
        fd: &FieldDescriptor,
        level: usize,
    ) {
        let list = match parent.get_field(fd).into_owned() {
            Value::List(items) => items,
            _ => return,
        };
        if list.is_empty() && !self.emit_defaults {
            return;
        }

        self.write_field_prefix(level, fd.name());
        self.buf.push_str("[\n");

        let element_kind = fd.kind();
        for (i, elem) in list.iter().enumerate() {
            self.write_indent(level + 1);
            match (&element_kind, elem) {
                (Kind::Message(mdesc), Value::Message(sub)) => {
                    let full = mdesc.full_name();
                    if full == "google.protobuf.Timestamp" {
                        self.buf.push_str(&format_rfc3339_nano(sub));
                    } else if full == "google.protobuf.Duration" {
                        self.buf.push_str(&format_go_duration(sub));
                    } else if is_wrapper_full_name(full) {
                        let inner_fd = mdesc
                            .get_field_by_name("value")
                            .expect("wrapper missing 'value'");
                        self.write_scalar_value_for(&inner_fd, sub);
                    } else {
                        self.buf.push_str("{\n");
                        self.encode_message(sub, level + 2);
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

    fn encode_map_field(
        &mut self,
        parent: &DynamicMessage,
        fd: &FieldDescriptor,
        level: usize,
    ) {
        let map = match parent.get_field(fd).into_owned() {
            Value::Map(m) => m,
            _ => return,
        };
        if map.is_empty() && !self.emit_defaults {
            return;
        }

        let mdesc = match fd.kind() {
            Kind::Message(m) => m,
            _ => return,
        };
        let val_fd = mdesc.map_entry_value_field();

        self.write_field_prefix(level, fd.name());
        self.buf.push_str("{\n");

        let mut entries: Vec<(String, Value)> =
            map.into_iter().map(|(k, v)| (format_map_key(&k), v)).collect();
        entries.sort_by(|a, b| a.0.cmp(&b.0));

        for (key_str, val) in entries {
            self.write_indent(level + 1);
            self.buf.push_str(&key_str);
            self.buf.push_str(": ");
            match (&val_fd.kind(), &val) {
                (Kind::Message(_), Value::Message(sub)) => {
                    self.buf.push_str("{\n");
                    self.encode_message(sub, level + 2);
                    self.write_indent(level + 1);
                    self.buf.push_str("}\n");
                }
                (Kind::Enum(_), Value::EnumNumber(n)) => {
                    self.write_enum_value(&val_fd, *n);
                    self.buf.push('\n');
                }
                _ => {
                    self.write_scalar_value(&val_fd.kind(), &val);
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

    fn write_enum_value(&mut self, fd: &FieldDescriptor, num: i32) {
        if let Kind::Enum(enum_desc) = fd.kind() {
            if let Some(ev) = enum_desc.get_value(num) {
                self.buf.push_str(ev.name());
                return;
            }
            self.buf.push_str(&num.to_string());
        }
    }

    fn write_scalar_value(&mut self, kind: &Kind, v: &Value) {
        match (kind, v) {
            (Kind::String, Value::String(s)) => self.buf.push_str(&write_quoted_string(s)),
            (Kind::Bool, Value::Bool(b)) => self.buf.push_str(if *b { "true" } else { "false" }),
            (Kind::Int32 | Kind::Sint32 | Kind::Sfixed32, Value::I32(n)) => {
                self.buf.push_str(&n.to_string())
            }
            (Kind::Int64 | Kind::Sint64 | Kind::Sfixed64, Value::I64(n)) => {
                self.buf.push_str(&n.to_string())
            }
            (Kind::Uint32 | Kind::Fixed32, Value::U32(n)) => self.buf.push_str(&n.to_string()),
            (Kind::Uint64 | Kind::Fixed64, Value::U64(n)) => self.buf.push_str(&n.to_string()),
            (Kind::Float, Value::F32(f)) => self.buf.push_str(&format_float_f32(*f)),
            (Kind::Double, Value::F64(f)) => self.buf.push_str(&format_float_f64(*f)),
            (Kind::Bytes, Value::Bytes(b)) => {
                self.buf.push_str("b\"");
                self.buf.push_str(&encode_base64(b));
                self.buf.push('"');
            }
            (Kind::Enum(_), Value::EnumNumber(_)) => {
                // Enum scalars route through write_enum_value; reaching here
                // means the caller got the dispatch wrong.
                self.buf.push_str("0");
            }
            _ => self.buf.push_str("?"),
        }
    }

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
        if url.is_empty() {
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
        let inner = match DynamicMessage::decode(inner_desc, &bytes[..]) {
            Ok(m) => m,
            Err(_) => return false,
        };

        self.write_indent(level);
        self.buf.push_str(fd.name());
        self.buf.push_str(" {\n");
        self.write_indent(level + 1);
        self.buf.push_str("@type = ");
        self.buf.push_str(&write_quoted_string(&url));
        self.buf.push('\n');
        let saved = self.path_prefix.clone();
        self.path_prefix = format!("{}{}.", saved, fd.name());
        self.encode_message(&inner, level + 1);
        self.path_prefix = saved;
        self.write_indent(level);
        self.buf.push_str("}\n");
        true
    }
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
    format!("{}", f)
}

fn format_float_f64(f: f64) -> String {
    if f.is_nan() {
        return "nan".to_string();
    }
    if f.is_infinite() {
        return if f > 0.0 { "inf" } else { "-inf" }.to_string();
    }
    format!("{}", f)
}

const B64_ALPHABET: &[u8] =
    b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";

fn encode_base64(bytes: &[u8]) -> String {
    let mut out = String::with_capacity((bytes.len() + 2) / 3 * 4);
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

fn is_valid_ident(s: &str) -> bool {
    if s.is_empty() || s == "true" || s == "false" || s == "null" {
        return false;
    }
    let bytes = s.as_bytes();
    for (i, &b) in bytes.iter().enumerate() {
        let is_letter = b.is_ascii_alphabetic() || b == b'_';
        let is_digit = b.is_ascii_digit();
        if i == 0 {
            if !is_letter {
                return false;
            }
        } else if !(is_letter || is_digit) {
            return false;
        }
    }
    true
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
    let era = if z >= 0 { z / 146_097 } else { (z - 146_096) / 146_097 };
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
