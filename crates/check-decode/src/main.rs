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
//!              --input  <path>
//!
//! Exit 0 → input was accepted
//! Exit 1 → input was rejected (clean error)
//! Other  → bug in the decoder (panic / abort / OOM / hang / ...)
//! ```
//!
//! Rust port handles `--proto <path>.proto` by reading the sibling
//! `<path>.binpb` (FileDescriptorSet); `prost-reflect` does not parse
//! `.proto` text at runtime. The corpus generator produces both files
//! together; if `<stem>.binpb` is missing, decode falls back to a
//! schema-name-only error.

use std::path::{Path, PathBuf};
use std::process::ExitCode;

use prost_reflect::{DescriptorPool, MessageDescriptor};
use protowire_pb::wire::{Reader, Result as PbResult, WireType, Writer};
use protowire_pb::{read_message, unmarshal as pb_unmarshal, write_message, Message};
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

// --- main -------------------------------------------------------------------

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().collect();
    let mut format: Option<String> = None;
    let mut schema: Option<String> = None;
    let mut proto: Option<PathBuf> = None;
    let mut input: Option<PathBuf> = None;
    let mut i = 1;
    while i < args.len() {
        let key = args[i].as_str();
        let val = args.get(i + 1).cloned();
        match key {
            "--format" => format = val,
            "--schema" => schema = val,
            "--proto" => proto = val.map(PathBuf::from),
            "--input" => input = val.map(PathBuf::from),
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

    match run(&format, &schema, proto.as_deref(), &input) {
        Ok(()) => ExitCode::from(0),
        Err(e) => {
            eprintln!("reject: {e}");
            ExitCode::from(1)
        }
    }
}

fn run(format: &str, schema: &str, proto: Option<&Path>, input: &Path) -> Result<(), String> {
    match format {
        "pxf" => {
            let proto = proto.ok_or_else(|| "--proto required for format=pxf".to_string())?;
            pxf_decode(input, schema, proto)
        }
        "pb" => pb_decode(input, schema),
        "envelope" => Err("envelope decode not yet implemented in this reference".to_string()),
        "sbe" => Err("sbe decode not yet implemented in this reference".to_string()),
        other => Err(format!("unsupported format: {other}")),
    }
}

fn pxf_decode(input: &Path, schema: &str, proto: &Path) -> Result<(), String> {
    let text = std::fs::read_to_string(input).map_err(|e| format!("read input: {e}"))?;
    let desc = load_descriptor(proto, schema)?;
    pxf_unmarshal(&text, &desc, UnmarshalOptions::default())
        .map(|_| ())
        .map_err(|e| format!("pxf: {e}"))
}

fn pb_decode(input: &Path, schema: &str) -> Result<(), String> {
    let bytes = std::fs::read(input).map_err(|e| format!("read input: {e}"))?;
    match schema {
        "adversarial.v1.Tree" => pb_unmarshal::<Tree>(&bytes)
            .map(|_| ())
            .map_err(|e| format!("pb: {e:?}")),
        "adversarial.v1.StringHolder" => pb_unmarshal::<StringHolder>(&bytes)
            .map(|_| ())
            .map_err(|e| format!("pb: {e:?}")),
        "adversarial.v1.BytesHolder" => pb_unmarshal::<BytesHolder>(&bytes)
            .map(|_| ())
            .map_err(|e| format!("pb: {e:?}")),
        "adversarial.v1.BigIntHolder" => pb_unmarshal::<BigIntHolder>(&bytes)
            .map(|_| ())
            .map_err(|e| format!("pb: {e:?}")),
        other => Err(format!("unknown schema for pb: {other}")),
    }
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
