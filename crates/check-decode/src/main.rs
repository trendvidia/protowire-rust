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
//! check-decode --format <pxf|pb|sbe|envelope> \
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

use std::path::{Path, PathBuf};
use std::process::ExitCode;

use prost_reflect::{DescriptorPool, DynamicMessage, MessageDescriptor};
use protowire_pb::wire::{Reader, Result as PbResult, WireType, Writer};
use protowire_pb::{read_message, unmarshal_with as pb_unmarshal, write_message, Message};
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
    }
    fn merge_field(&mut self, num: u32, wt: WireType, r: &mut Reader<'_>) -> PbResult<()> {
        match num {
            1 => self.child = Some(Box::new(read_message(r)?)),
            2 => self.label = r.string()?,
            _ => r.skip(wt)?,
        }
        Ok(())
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
                    let v = packed.varint()? as i32;
                    r.push_element(&mut self.values, v)?;
                }
            }
            (1, _) => {
                let v = r.varint()? as i32;
                r.push_element(&mut self.values, v)?;
            }
            _ => r.skip(wt)?,
        }
        Ok(())
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
            1 => self.value = r.varint()? as i64,
            _ => r.skip(wt)?,
        }
        Ok(())
    }
}

// Mirror of pxf/bignum.proto (protowire#279): the arbitrary-precision
// carriers, so the corpus can prove the digit cap on a field only the cap
// rejects, and bound Decimal.scale on the PB wire.

#[derive(Default, Debug)]
struct BigIntMsg {
    abs: Vec<u8>,
    negative: bool,
}
impl Message for BigIntMsg {
    fn encode_to(&self, w: &mut Writer) {
        if !self.abs.is_empty() {
            w.tag(1, WireType::LengthDelimited);
            w.bytes(&self.abs);
        }
        if self.negative {
            w.tag(2, WireType::Varint);
            w.varint(1);
        }
    }
    fn merge_field(&mut self, num: u32, wt: WireType, r: &mut Reader<'_>) -> PbResult<()> {
        match num {
            1 => self.abs = r.bytes()?,
            2 => self.negative = r.varint()? != 0,
            _ => r.skip(wt)?,
        }
        Ok(())
    }
}

#[derive(Default, Debug)]
struct DecimalMsg {
    unscaled: Vec<u8>,
    scale: i32,
    negative: bool,
}
impl Message for DecimalMsg {
    fn encode_to(&self, w: &mut Writer) {
        if !self.unscaled.is_empty() {
            w.tag(1, WireType::LengthDelimited);
            w.bytes(&self.unscaled);
        }
        if self.scale != 0 {
            w.tag(2, WireType::Varint);
            w.varint_i32(self.scale);
        }
        if self.negative {
            w.tag(3, WireType::Varint);
            w.varint(1);
        }
    }
    fn merge_field(&mut self, num: u32, wt: WireType, r: &mut Reader<'_>) -> PbResult<()> {
        match num {
            1 => self.unscaled = r.bytes()?,
            // proto3 int32: a negative scale arrives as a 10-byte
            // sign-extended varint; truncating to i32 recovers it.
            2 => self.scale = r.varint()? as i32,
            3 => self.negative = r.varint()? != 0,
            _ => r.skip(wt)?,
        }
        Ok(())
    }
}

#[derive(Default, Debug)]
struct BigFloatMsg {
    mantissa: Vec<u8>,
    exponent: i32,
    prec: u32,
    negative: bool,
}
impl Message for BigFloatMsg {
    fn encode_to(&self, w: &mut Writer) {
        if !self.mantissa.is_empty() {
            w.tag(1, WireType::LengthDelimited);
            w.bytes(&self.mantissa);
        }
        if self.exponent != 0 {
            w.tag(2, WireType::Varint);
            w.varint_i32(self.exponent);
        }
        if self.prec != 0 {
            w.tag(3, WireType::Varint);
            w.varint(u64::from(self.prec));
        }
        if self.negative {
            w.tag(4, WireType::Varint);
            w.varint(1);
        }
    }
    fn merge_field(&mut self, num: u32, wt: WireType, r: &mut Reader<'_>) -> PbResult<()> {
        match num {
            1 => self.mantissa = r.bytes()?,
            2 => self.exponent = r.varint()? as i32,
            3 => self.prec = r.varint()? as u32,
            4 => self.negative = r.varint()? != 0,
            _ => r.skip(wt)?,
        }
        Ok(())
    }
}

#[derive(Default, Debug)]
struct BigNumHolder {
    big_int: Option<BigIntMsg>,
    decimal: Option<DecimalMsg>,
    big_float: Option<BigFloatMsg>,
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
}

impl BigNumHolder {
    /// HARDENING § Mandatory limits: `pxf.Decimal.scale` is a digit count
    /// and a decoder that materialises the value computes 10^scale from
    /// it, so its magnitude is bound by MaxNumericLiteralDigits on both
    /// signs before anything is materialised. This port's `pb` layer
    /// carries no Decimal type and materialises nothing; the bound is
    /// applied here, where a consumer would first read the value.
    /// `BigFloat.exponent` is a binary exponent and has no limit.
    fn check_limits(&self, max_digits: usize) -> Result<(), String> {
        if let Some(d) = &self.decimal {
            let max = max_digits as i64;
            if i64::from(d.scale) > max || i64::from(d.scale) < -max {
                return Err(format!(
                    "pxf.Decimal scale {} exceeds MaxNumericLiteralDigits={}",
                    d.scale, max
                ));
            }
        }
        Ok(())
    }
}

// --- main -------------------------------------------------------------------

/// The `--limit NAME=VALUE` flags of one run, resolved onto each crate's
/// per-call limits with the crate defaults standing in for the rest.
/// `MaxBytesLiteralLength` reaches only the PXF decoder, and
/// `MaxNumericLiteralDigits` the PXF decoder and the pb BigNumHolder's
/// scale bound, as in the reference.
#[derive(Debug, Default, Clone)]
struct LimitFlags {
    max_message_size: Option<usize>,
    max_nesting_depth: Option<usize>,
    max_numeric_literal_digits: Option<usize>,
    max_bytes_literal_length: Option<usize>,
    max_repeated_count: Option<usize>,
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
        }
    }

    fn pb(&self) -> protowire_pb::Limits {
        let d = protowire_pb::Limits::default();
        protowire_pb::Limits {
            max_message_size: self.max_message_size.unwrap_or(d.max_message_size),
            max_nesting_depth: self.max_nesting_depth.unwrap_or(d.max_nesting_depth),
            max_repeated_count: self.max_repeated_count.unwrap_or(d.max_repeated_count),
        }
    }

    fn sbe(&self) -> protowire_sbe::Limits {
        let d = protowire_sbe::Limits::default();
        protowire_sbe::Limits {
            max_message_size: self.max_message_size.unwrap_or(d.max_message_size),
            max_repeated_count: self.max_repeated_count.unwrap_or(d.max_repeated_count),
        }
    }

    fn max_digits(&self) -> usize {
        self.max_numeric_literal_digits
            .unwrap_or(protowire_pxf::MAX_NUMERIC_LITERAL_DIGITS)
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
        "pb" => pb_decode(input, schema, limits),
        "envelope" => Err("envelope decode not yet implemented in this reference".to_string()),
        "sbe" => {
            let proto = proto.ok_or_else(|| "--proto required for format=sbe".to_string())?;
            sbe_decode(input, schema, proto, limits)
        }
        other => Err(format!("unsupported format: {other}")),
    }
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
        .map_err(|e| format!("pxf: {e}"))
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
            .map_err(|e| format!("pb: {e}"))
            .and_then(|m| {
                m.check_limits(limits.max_digits())
                    .map_err(|e| format!("pb: {e}"))
            }),
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
