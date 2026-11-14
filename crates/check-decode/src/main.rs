// SPDX-License-Identifier: MIT
// Copyright (c) 2026 TrendVidia, LLC.
//! Per-port reference for the protowire HARDENING.md conformance corpus.
//!
//! Driven by `protowire/scripts/cross_security_check.sh`. See:
//! - `protowire/docs/HARDENING.md`
//! - `protowire/testdata/adversarial/README.md`
//!
//! Contract:
//!
//! ```text
//! check-decode --format <pxf|pxf-pb|pb|pb-render|pb-text|sbe|envelope> \
//!              --schema <fully.qualified.MessageType> \
//!              --proto  <path-to-adversarial.proto> \
//!              --input  <path> \
//!              [--limit NAME=VALUE]...
//!
//! Exit 0 → input was accepted
//! Exit 1 → input was rejected (clean error)
//! Exit 2 → usage error
//! Other  → bug in the decoder (panic / abort / OOM / hang / ...)
//! ```
//!
//! `pxf-pb` also writes the message's PB bytes on stdout, map entries in
//! key order, for the write gate (`protowire/scripts/cross_write_check.sh`,
//! protowire#509).
//!
//! `pb-text` reads PB bytes with the stock runtime and writes the message's
//! PXF on stdout, for the text gate (`protowire/scripts/cross_text_check.sh`,
//! protowire#503). `pb-render` does the same and discards the text.
//!
//! `--limit NAME=VALUE`, repeatable, lowers a HARDENING limit for this run
//! so the corpus can prove a 64 MiB cap with a 2 KiB fixture
//! (protowire#299): every name in HARDENING § Mandatory limits but
//! `MaxVarintBytes` is accepted, and each reaches the port's per-call
//! configuration (`protowire_pxf::Limits`, `protowire_pb::Limits`,
//! `protowire_sbe::Limits`); an unknown name or a non-positive value is a
//! usage error.
//!
//! Rust port handles `--proto <path>.proto` by reading the sibling
//! `<path>.binpb` (FileDescriptorSet); `prost-reflect` does not parse
//! `.proto` text at runtime. The corpus generator produces both files
//! together; if `<stem>.binpb` is missing, decode falls back to a
//! schema-name-only error.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::process::ExitCode;

use prost_reflect::{DescriptorPool, DynamicMessage, MessageDescriptor};
use protowire_pb::wire::{Reader, Result as PbResult, WireType, Writer};
use protowire_pb::{
    read_map_entry, read_message, unmarshal_with as pb_unmarshal, write_map_entry, write_message,
    Message,
};
use protowire_pxf::{unmarshal as pxf_unmarshal, UnmarshalOptions};

// --- Hand-mirrored Go-style message impls for adversarial.proto -------------
// protowire-pb's `Message` trait is hand-implemented per type (no derive, no
// descriptor-driven dynamic dispatch), so the four adversarial schemas are
// re-encoded here. Drift between this file and adversarial.proto must be
// caught by the conformance run itself: a wrong field number flips the
// manifest's accept/reject expectations.

#[derive(Default, Debug)]
struct Tree {
    child: Option<Box<Tree>>,
    label: String,
    branches: BTreeMap<String, Tree>,
    tags: BTreeMap<String, String>,
}

impl Message for Tree {
    fn encode_to(&self, w: &mut Writer) {
        if let Some(c) = &self.child {
            write_message(w, 1, c.as_ref());
        }
        if !self.label.is_empty() {
            w.tag(2, WireType::LengthDelimited);
            w.string(&self.label);
        }
        for (k, v) in &self.branches {
            write_map_entry(w, 4, k.as_str(), v);
        }
        for (k, v) in &self.tags {
            write_map_entry(w, 5, k.as_str(), v.as_str());
        }
    }
    fn merge_field(&mut self, num: u32, wt: WireType, r: &mut Reader<'_>) -> PbResult<()> {
        match num {
            1 => self.child = Some(Box::new(read_message(r)?)),
            2 => self.label = r.string()?,
            // Each map entry is one descent (protowire#449).
            4 => {
                let (k, v) = read_map_entry(r, |r| r.string(), read_message)?;
                if !self.branches.contains_key(&k) {
                    r.check_map(self.branches.len())?;
                }
                self.branches.insert(k, v);
            }
            5 => {
                let (k, v) = read_map_entry(r, |r| r.string(), |r| r.string())?;
                if !self.tags.contains_key(&k) {
                    r.check_map(self.tags.len())?;
                }
                self.tags.insert(k, v);
            }
            _ => r.skip(wt)?,
        }
        Ok(())
    }
    // The field names of protowire-go's scripts/check_decode, here and below.
    fn field_name(num: u32) -> Option<&'static str> {
        match num {
            1 => Some("Child"),
            2 => Some("Label"),
            4 => Some("Branches"),
            5 => Some("Tags"),
            _ => None,
        }
    }
}

#[derive(Default, Debug)]
struct StringHolder {
    value: String,
}
impl Message for StringHolder {
    fn encode_to(&self, w: &mut Writer) {
        if !self.value.is_empty() {
            w.tag(1, WireType::LengthDelimited);
            w.string(&self.value);
        }
    }
    fn merge_field(&mut self, num: u32, wt: WireType, r: &mut Reader<'_>) -> PbResult<()> {
        match num {
            1 => self.value = r.string()?,
            _ => r.skip(wt)?,
        }
        Ok(())
    }
    fn field_name(num: u32) -> Option<&'static str> {
        (num == 1).then_some("Value")
    }
}

#[derive(Default, Debug)]
struct BytesHolder {
    value: Vec<u8>,
}
impl Message for BytesHolder {
    fn encode_to(&self, w: &mut Writer) {
        if !self.value.is_empty() {
            w.tag(1, WireType::LengthDelimited);
            w.bytes(&self.value);
        }
    }
    fn merge_field(&mut self, num: u32, wt: WireType, r: &mut Reader<'_>) -> PbResult<()> {
        match num {
            1 => self.value = r.bytes()?,
            _ => r.skip(wt)?,
        }
        Ok(())
    }
    fn field_name(num: u32) -> Option<&'static str> {
        (num == 1).then_some("Value")
    }
}

/// ListHolder carries the repeated field the MaxRepeatedCount fixtures
/// bound (protowire#299): `repeated int32 values = 1`, packed on the wire
/// as protoc writes it, read unpacked too.
#[derive(Default, Debug)]
struct ListHolder {
    values: Vec<i32>,
}
impl Message for ListHolder {
    fn encode_to(&self, w: &mut Writer) {
        if self.values.is_empty() {
            return;
        }
        let mut inner = Writer::new();
        for v in &self.values {
            inner.varint_i32(*v);
        }
        let payload = inner.finish();
        w.tag(1, WireType::LengthDelimited);
        w.bytes(&payload);
    }
    fn merge_field(&mut self, num: u32, wt: WireType, r: &mut Reader<'_>) -> PbResult<()> {
        match (num, wt) {
            (1, WireType::LengthDelimited) => {
                let mut packed = r.packed()?;
                while !packed.eof() {
                    let v = packed.int32()?;
                    r.push_element(&mut self.values, v)?;
                }
            }
            (1, _) => {
                let v = r.int32()?;
                r.push_element(&mut self.values, v)?;
            }
            _ => r.skip(wt)?,
        }
        Ok(())
    }
    fn field_name(num: u32) -> Option<&'static str> {
        (num == 1).then_some("Values")
    }
}

#[derive(Default, Debug)]
struct BigIntHolder {
    value: i64,
}
impl Message for BigIntHolder {
    fn encode_to(&self, w: &mut Writer) {
        if self.value != 0 {
            w.tag(1, WireType::Varint);
            w.varint(self.value as u64);
        }
    }
    fn merge_field(&mut self, num: u32, wt: WireType, r: &mut Reader<'_>) -> PbResult<()> {
        match num {
            1 => self.value = r.int64()?,
            _ => r.skip(wt)?,
        }
        Ok(())
    }
    fn field_name(num: u32) -> Option<&'static str> {
        (num == 1).then_some("Value")
    }
}

// adversarial.v1.BigNumHolder, through protowire-pb's big-number types
// (#142): reading one checks Decimal.scale and BigFloat's prec and binary
// exponent under the call's Limits, as protowire-go's pb.Unmarshal does, so
// the corpus holds the library to the bounds rather than this harness.

#[derive(Default, Debug)]
struct BigNumHolder {
    big_int: Option<protowire_pb::BigInt>,
    decimal: Option<protowire_pb::Decimal>,
    big_float: Option<protowire_pb::BigFloat>,
}
impl Message for BigNumHolder {
    fn encode_to(&self, w: &mut Writer) {
        if let Some(m) = &self.big_int {
            write_message(w, 1, m);
        }
        if let Some(m) = &self.decimal {
            write_message(w, 2, m);
        }
        if let Some(m) = &self.big_float {
            write_message(w, 3, m);
        }
    }
    fn merge_field(&mut self, num: u32, wt: WireType, r: &mut Reader<'_>) -> PbResult<()> {
        match num {
            1 => self.big_int = Some(read_message(r)?),
            2 => self.decimal = Some(read_message(r)?),
            3 => self.big_float = Some(read_message(r)?),
            _ => r.skip(wt)?,
        }
        Ok(())
    }
    fn field_name(num: u32) -> Option<&'static str> {
        ["BigInt", "Decimal", "BigFloat"]
            .get(num.checked_sub(1)? as usize)
            .copied()
    }
}

// --- main -------------------------------------------------------------------

/// The `--limit NAME=VALUE` flags of one run, resolved onto each crate's
/// per-call limits with the crate defaults standing in for the rest.
/// `MaxBytesLiteralLength` reaches only the PXF decoder, and
/// `MaxNumericLiteralDigits` the PXF decoder and the pb BigNumHolder's
/// scale and prec bounds, as in the reference.
#[derive(Debug, Default, Clone)]
struct LimitFlags {
    max_message_size: Option<usize>,
    max_nesting_depth: Option<usize>,
    max_numeric_literal_digits: Option<usize>,
    max_bytes_literal_length: Option<usize>,
    max_repeated_count: Option<usize>,
    max_big_float_exponent: Option<usize>,
}

impl LimitFlags {
    fn set(&mut self, arg: &str) -> Result<(), String> {
        let Some((name, value)) = arg.split_once('=') else {
            return Err(format!("--limit wants NAME=VALUE, got {arg:?}"));
        };
        let n: usize = match value.parse() {
            Ok(n) if n > 0 => n,
            _ => {
                return Err(format!(
                    "--limit {name}: want a positive integer, got {value:?}"
                ))
            }
        };
        match name {
            "MaxMessageSize" => self.max_message_size = Some(n),
            "MaxNestingDepth" => self.max_nesting_depth = Some(n),
            "MaxNumericLiteralDigits" => self.max_numeric_literal_digits = Some(n),
            "MaxBytesLiteralLength" => self.max_bytes_literal_length = Some(n),
            "MaxRepeatedCount" => self.max_repeated_count = Some(n),
            "MaxBigFloatExponent" => self.max_big_float_exponent = Some(n),
            _ => return Err(format!("--limit: unknown limit {name:?}")),
        }
        Ok(())
    }

    fn pxf(&self) -> protowire_pxf::Limits {
        let d = protowire_pxf::Limits::default();
        protowire_pxf::Limits {
            max_message_size: self.max_message_size.unwrap_or(d.max_message_size),
            max_nesting_depth: self.max_nesting_depth.unwrap_or(d.max_nesting_depth),
            max_numeric_literal_digits: self
                .max_numeric_literal_digits
                .unwrap_or(d.max_numeric_literal_digits),
            max_bytes_literal_length: self
                .max_bytes_literal_length
                .unwrap_or(d.max_bytes_literal_length),
            max_repeated_count: self.max_repeated_count.unwrap_or(d.max_repeated_count),
            max_big_float_exponent: self
                .max_big_float_exponent
                .unwrap_or(d.max_big_float_exponent),
        }
    }

    fn pb(&self) -> protowire_pb::Limits {
        let d = protowire_pb::Limits::default();
        protowire_pb::Limits {
            max_message_size: self.max_message_size.unwrap_or(d.max_message_size),
            max_nesting_depth: self.max_nesting_depth.unwrap_or(d.max_nesting_depth),
            max_repeated_count: self.max_repeated_count.unwrap_or(d.max_repeated_count),
            max_numeric_literal_digits: self
                .max_numeric_literal_digits
                .unwrap_or(d.max_numeric_literal_digits),
            max_big_float_exponent: self
                .max_big_float_exponent
                .unwrap_or(d.max_big_float_exponent),
        }
    }

    fn sbe(&self) -> protowire_sbe::Limits {
        let d = protowire_sbe::Limits::default();
        protowire_sbe::Limits {
            max_message_size: self.max_message_size.unwrap_or(d.max_message_size),
            max_repeated_count: self.max_repeated_count.unwrap_or(d.max_repeated_count),
        }
    }
}

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().collect();
    let mut format: Option<String> = None;
    let mut schema: Option<String> = None;
    let mut proto: Option<PathBuf> = None;
    let mut input: Option<PathBuf> = None;
    let mut limits = LimitFlags::default();
    let mut i = 1;
    while i < args.len() {
        let key = args[i].as_str();
        let val = args.get(i + 1).cloned();
        match key {
            "--format" => format = val,
            "--schema" => schema = val,
            "--proto" => proto = val.map(PathBuf::from),
            "--input" => input = val.map(PathBuf::from),
            "--limit" => {
                let Some(v) = val.as_deref() else {
                    eprintln!("check-decode: --limit wants NAME=VALUE");
                    return ExitCode::from(2);
                };
                if let Err(e) = limits.set(v) {
                    eprintln!("check-decode: {e}");
                    return ExitCode::from(2);
                }
            }
            other => {
                eprintln!("check-decode: unknown arg {other:?}");
                return ExitCode::from(2);
            }
        }
        i += 2;
    }

    let format = match format {
        Some(f) => f,
        None => {
            eprintln!("check-decode: --format required");
            return ExitCode::from(2);
        }
    };
    let schema = match schema {
        Some(s) => s,
        None => {
            eprintln!("check-decode: --schema required");
            return ExitCode::from(2);
        }
    };
    let input = match input {
        Some(p) => p,
        None => {
            eprintln!("check-decode: --input required");
            return ExitCode::from(2);
        }
    };

    match run(&format, &schema, proto.as_deref(), &input, &limits) {
        Ok(()) => ExitCode::from(0),
        Err(e) => {
            eprintln!("reject: {e}");
            ExitCode::from(1)
        }
    }
}

fn run(
    format: &str,
    schema: &str,
    proto: Option<&Path>,
    input: &Path,
    limits: &LimitFlags,
) -> Result<(), String> {
    match format {
        "pxf" => {
            let proto = proto.ok_or_else(|| "--proto required for format=pxf".to_string())?;
            pxf_decode(input, schema, proto, limits)
        }
        "pxf-pb" => {
            let proto = proto.ok_or_else(|| "--proto required for format=pxf-pb".to_string())?;
            pxf_pb(input, schema, proto, limits)
        }
        "pb" => pb_decode(input, schema, limits),
        "pb-render" => {
            let proto = proto.ok_or_else(|| "--proto required for format=pb-render".to_string())?;
            pb_render(input, schema, proto, false)
        }
        "pb-text" => {
            let proto = proto.ok_or_else(|| "--proto required for format=pb-text".to_string())?;
            pb_render(input, schema, proto, true)
        }
        "envelope" => Err("envelope decode not yet implemented in this reference".to_string()),
        "sbe" => {
            let proto = proto.ok_or_else(|| "--proto required for format=sbe".to_string())?;
            sbe_decode(input, schema, proto, limits)
        }
        other => Err(format!("unsupported format: {other}")),
    }
}

/// The producer's side of the bounds (HARDENING.md § Arbitrary-precision
/// magnitudes, § Recursion): a stock reader reads the bytes, storing the
/// fields and bounding nothing, and they are then written as PXF. The
/// hardened decoders are bypassed on purpose, or the row would be refused
/// before the encoder ran (protowire#448).
///
/// The reader allows nesting to [`STOCK_RECURSION_LIMIT`], protobuf-go's
/// default, rather than prost's fixed 100, so a 101-deep row reaches the
/// producer, which is what must refuse it (protowire#502). It runs on a
/// thread with a stack big enough for that depth.
fn pb_render(input: &Path, schema: &str, proto: &Path, print: bool) -> Result<(), String> {
    let bytes = std::fs::read(input).map_err(|e| format!("read input: {e}"))?;
    let desc = load_descriptor(proto, schema)?;
    std::thread::Builder::new()
        .stack_size(256 << 20)
        .spawn(move || {
            let mut msg = DynamicMessage::new(desc.clone());
            stock_merge(&mut msg, &bytes, 0).map_err(|e| format!("protobuf runtime: {e}"))?;
            let out = protowire_pxf::marshal(&msg, &desc, protowire_pxf::MarshalOptions::default())
                .map_err(|e| e.to_string())
                .and_then(|text| {
                    if print {
                        use std::io::Write as _;
                        std::io::stdout()
                            .write_all(text.as_bytes())
                            .map_err(|e| format!("write stdout: {e}"))
                    } else {
                        Ok(())
                    }
                });
            // Dropping a deep message recurses once per level; the process
            // is about to exit.
            std::mem::forget(msg);
            out
        })
        .map_err(|e| format!("spawn reader: {e}"))?
        .join()
        .map_err(|_| "reader thread panicked".to_string())?
}

/// protobuf-go's default recursion limit, which its `proto.Unmarshal`
/// applies where protowire-go's `pb-render` reads.
const STOCK_RECURSION_LIMIT: usize = 10_000;

/// Merge `buf` into `msg` as prost does, but with the recursion limit at
/// [`STOCK_RECURSION_LIMIT`]: a message-typed field (a map entry included)
/// is read here, one level down, and every other field is handed to
/// prost-reflect's own merge, which never descends.
fn stock_merge(msg: &mut DynamicMessage, mut buf: &[u8], depth: usize) -> Result<(), String> {
    use prost::encoding::{decode_key, decode_varint, skip_field, DecodeContext, WireType as PW};
    use prost::Message as _;
    use prost_reflect::{Kind, ReflectMessage, Value};

    if depth > STOCK_RECURSION_LIMIT {
        return Err("recursion limit reached".into());
    }
    let desc = msg.descriptor();
    while !buf.is_empty() {
        let start = buf;
        let (tag, wire_type) = decode_key(&mut buf).map_err(|e| e.to_string())?;
        let fd = desc.get_field(tag);
        let sub_desc = match (&fd, wire_type) {
            (Some(fd), PW::LengthDelimited) => match fd.kind() {
                Kind::Message(m) => Some(m),
                _ => None,
            },
            _ => None,
        };
        let (Some(fd), Some(sub_desc)) = (fd, sub_desc) else {
            skip_field(wire_type, tag, &mut buf, DecodeContext::default())
                .map_err(|e| e.to_string())?;
            let field = &start[..start.len() - buf.len()];
            msg.merge(field).map_err(|e| e.to_string())?;
            continue;
        };
        let len = decode_varint(&mut buf).map_err(|e| e.to_string())?;
        let len = usize::try_from(len)
            .ok()
            .filter(|&n| n <= buf.len())
            .ok_or("length exceeds buffer")?;
        let (body, rest) = buf.split_at(len);
        buf = rest;
        if fd.is_map() {
            let mut entry = DynamicMessage::new(sub_desc.clone());
            stock_merge(&mut entry, body, depth + 1)?;
            // An absent key or value is its field's default, as in prost.
            let key_fd = sub_desc.map_entry_key_field();
            let value_fd = sub_desc.map_entry_value_field();
            let key = entry
                .take_field(&key_fd)
                .unwrap_or_else(|| Value::default_value_for_field(&key_fd))
                .into_map_key()
                .ok_or("map key of no key type")?;
            let value = entry
                .take_field(&value_fd)
                .unwrap_or_else(|| Value::default_value_for_field(&value_fd));
            if let Value::Map(map) = msg.get_field_mut(&fd) {
                map.insert(key, value);
            }
        } else if fd.is_list() {
            let mut elem = DynamicMessage::new(sub_desc);
            stock_merge(&mut elem, body, depth + 1)?;
            if let Value::List(list) = msg.get_field_mut(&fd) {
                list.push(Value::Message(elem));
            }
        } else if let Value::Message(sub) = msg.get_field_mut(&fd) {
            stock_merge(sub, body, depth + 1)?;
        }
    }
    Ok(())
}

fn pxf_decode(input: &Path, schema: &str, proto: &Path, limits: &LimitFlags) -> Result<(), String> {
    let text = std::fs::read_to_string(input).map_err(|e| format!("read input: {e}"))?;
    let desc = load_descriptor(proto, schema)?;
    let options = UnmarshalOptions {
        limits: limits.pxf(),
        ..Default::default()
    };
    pxf_unmarshal(&text, &desc, options)
        .map(|_| ())
        .map_err(|e| e.to_string())
}

/// The PB bytes for a PXF document, on stdout, for the write gate
/// (`protowire/scripts/cross_write_check.sh`, protowire#509). It decodes
/// with [`protowire_pxf::unmarshal_full`], which validates `(pxf.required)`
/// and applies `(pxf.default)`, as protowire-go's harness does, and writes
/// map entries in key order, as Go's deterministic writer puts them;
/// prost-reflect writes a map in its `HashMap`'s order.
fn pxf_pb(input: &Path, schema: &str, proto: &Path, limits: &LimitFlags) -> Result<(), String> {
    use std::io::Write as _;
    let text = std::fs::read_to_string(input).map_err(|e| format!("read input: {e}"))?;
    let desc = load_descriptor(proto, schema)?;
    let options = UnmarshalOptions {
        limits: limits.pxf(),
        ..Default::default()
    };
    let (msg, _) =
        protowire_pxf::unmarshal_full(&text, &desc, options).map_err(|e| e.to_string())?;
    let mut out = Vec::new();
    deterministic::encode(&msg, &mut out);
    std::io::stdout()
        .write_all(&out)
        .map_err(|e| format!("write stdout: {e}"))
}

/// protobuf-go's deterministic marshal, for a [`DynamicMessage`]: fields in
/// number order, map entries in key order (false before true, integers by
/// value, strings by UTF-8 bytes, which is `MapKey`'s order), each entry
/// carrying its key and value whether or not they are zero, and unknown
/// fields last.
mod deterministic {
    use prost::encoding::{encode_key, encode_varint, WireType};
    use prost::Message as _;
    use prost_reflect::{DynamicMessage, FieldDescriptor, Kind, ReflectMessage, Value};

    pub fn encode(msg: &DynamicMessage, out: &mut Vec<u8>) {
        let desc = msg.descriptor();
        let mut fields: Vec<(FieldDescriptor, &Value)> = msg.fields().collect();
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
                        field_always(&key, &key_fd, &mut entry);
                        match v {
                            Value::Message(sub) => message(2, sub, &mut entry),
                            _ => {
                                let mut val = DynamicMessage::new(entry_desc.clone());
                                val.set_field(&value_fd, v.clone());
                                field_always(&val, &value_fd, &mut entry);
                            }
                        }
                        encode_key(fd.number(), WireType::LengthDelimited, out);
                        encode_varint(entry.len() as u64, out);
                        out.extend_from_slice(&entry);
                    }
                }
                (Value::List(items), Kind::Message(_)) => {
                    for item in items {
                        if let Value::Message(sub) = item {
                            message(fd.number(), sub, out);
                        }
                    }
                }
                (Value::Message(sub), _) => message(fd.number(), sub, out),
                _ => {
                    let mut one = DynamicMessage::new(desc.clone());
                    one.set_field(&fd, value.clone());
                    one.encode_raw(out);
                }
            }
        }
        for unknown in msg.unknown_fields() {
            unknown.encode(out);
        }
    }

    fn message(number: u32, sub: &DynamicMessage, out: &mut Vec<u8>) {
        let mut body = Vec::new();
        encode(sub, &mut body);
        encode_key(number, WireType::LengthDelimited, out);
        encode_varint(body.len() as u64, out);
        out.extend_from_slice(&body);
    }

    /// A map entry's key or value, written even when it is zero.
    fn field_always(msg: &DynamicMessage, field: &FieldDescriptor, out: &mut Vec<u8>) {
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
}

fn pb_decode(input: &Path, schema: &str, limits: &LimitFlags) -> Result<(), String> {
    let bytes = std::fs::read(input).map_err(|e| format!("read input: {e}"))?;
    let lim = limits.pb();
    match schema {
        "adversarial.v1.Tree" => pb_unmarshal::<Tree>(&bytes, lim)
            .map(|_| ())
            .map_err(|e| format!("pb: {e}")),
        "adversarial.v1.StringHolder" => pb_unmarshal::<StringHolder>(&bytes, lim)
            .map(|_| ())
            .map_err(|e| format!("pb: {e}")),
        "adversarial.v1.BytesHolder" => pb_unmarshal::<BytesHolder>(&bytes, lim)
            .map(|_| ())
            .map_err(|e| format!("pb: {e}")),
        "adversarial.v1.BigIntHolder" => pb_unmarshal::<BigIntHolder>(&bytes, lim)
            .map(|_| ())
            .map_err(|e| format!("pb: {e}")),
        "adversarial.v1.ListHolder" => pb_unmarshal::<ListHolder>(&bytes, lim)
            .map(|_| ())
            .map_err(|e| format!("pb: {e}")),
        "adversarial.v1.BigNumHolder" => pb_unmarshal::<BigNumHolder>(&bytes, lim)
            .map(|_| ())
            .map_err(|e| format!("pb: {e}")),
        other => Err(format!("unknown schema for pb: {other}")),
    }
}

/// The SBE leg binds the codec from the same descriptor set the PXF leg
/// uses (adversarial.proto carries the sbe options), so the corpus's SBE
/// rows are decoded rather than refused for want of a codec — a refusal
/// reads as a rejection to the harness, which passed every expect-reject
/// row without decoding a byte.
fn sbe_decode(input: &Path, schema: &str, proto: &Path, limits: &LimitFlags) -> Result<(), String> {
    let bytes = std::fs::read(input).map_err(|e| format!("read input: {e}"))?;
    let desc = load_descriptor(proto, schema)?;
    // SbeError messages already carry the "sbe:" prefix.
    let codec = protowire_sbe::Codec::from_files_with_limits(&[desc.parent_file()], limits.sbe())
        .map_err(|e| e.to_string())?;
    let mut msg = DynamicMessage::new(desc);
    protowire_sbe::unmarshal(&codec, &mut msg, &bytes).map_err(|e| e.to_string())
}

fn load_descriptor(proto: &Path, schema: &str) -> Result<MessageDescriptor, String> {
    let fds_path = proto.with_extension("binpb");
    let fds_bytes = std::fs::read(&fds_path).map_err(|e| {
        format!(
            "read {} (sibling FileDescriptorSet of {}): {}",
            fds_path.display(),
            proto.display(),
            e
        )
    })?;
    let pool =
        DescriptorPool::decode(fds_bytes.as_slice()).map_err(|e| format!("decode FDS: {e}"))?;
    pool.get_message_by_name(schema)
        .ok_or_else(|| format!("schema {schema:?} not in {}", fds_path.display()))
}
