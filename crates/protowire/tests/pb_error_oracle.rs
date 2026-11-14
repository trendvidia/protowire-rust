// SPDX-License-Identifier: MIT
// Copyright (c) 2026 TrendVidia, LLC.
//! pb decode errors held to protowire-go's `pb.Unmarshal` (#144): each row of
//! `testdata/conformance/pb-error-oracle.tsv` is an input and what
//! protowire-go said about it. The rows are every prefix of a full encoding
//! of each message and a list of hand-made damage: cut-off values, varints
//! past 64 bits, bad tags and wire types, unknown groups, text that is not
//! UTF-8, and the limits. `scripts/gen-pb-error-oracle.sh` regenerates the
//! file; `scripts/pb-error-oracle/main.go` holds the Go structs, which the
//! structs here mirror, field names and all.
//!
//! The umbrella crate hosts the test because it reads the envelope too.

use std::collections::BTreeMap;

use protowire::envelope::Envelope;
use protowire::pb::{
    read_map_entry, read_message, unmarshal_with, BigFloat, BigInt, Decimal, Limits, Message,
    Reader, Result, WireType, Writer,
};

const ORACLE: &str = include_str!("../testdata/conformance/pb-error-oracle.tsv");

/// The names of fields 1, 2, … of a message, as `Message::field_name` wants
/// them.
fn nth(names: &'static [&'static str], num: u32) -> Option<&'static str> {
    names.get(num.checked_sub(1)? as usize).copied()
}

#[derive(Default, Debug)]
struct Inner {
    name: String,
    n: i64,
}

impl Message for Inner {
    fn encode_to(&self, _: &mut Writer) {}

    fn merge_field(&mut self, num: u32, wt: WireType, r: &mut Reader<'_>) -> Result<()> {
        match num {
            1 => self.name = r.string()?,
            2 => self.n = r.int64()?,
            _ => r.skip(wt)?,
        }
        Ok(())
    }

    fn field_name(num: u32) -> Option<&'static str> {
        nth(&["Name", "N"], num)
    }
}

/// The Go runner's `All`: one field of each kind, each read by its kind
/// whatever the wire type says, as protowire-go's struct codec reads it.
#[derive(Default, Debug)]
struct All {
    b: bool,
    i32: i32,
    i64: i64,
    u32: u32,
    u64: u64,
    s64: i64,
    f32: f32,
    f64: f64,
    str: String,
    raw: Vec<u8>,
    msg: Option<Inner>,
    ints: Vec<i64>,
    strs: Vec<String>,
    kids: Vec<Inner>,
    smap: BTreeMap<String, String>,
    imap: BTreeMap<i64, Inner>,
    big: Option<BigInt>,
    dec: Option<Decimal>,
    float: Option<BigFloat>,
}

impl Message for All {
    fn encode_to(&self, _: &mut Writer) {}

    fn merge_field(&mut self, num: u32, wt: WireType, r: &mut Reader<'_>) -> Result<()> {
        match num {
            1 => self.b = r.bool()?,
            2 => self.i32 = r.int32()?,
            3 => self.i64 = r.int64()?,
            4 => self.u32 = r.uint32()?,
            5 => self.u64 = r.uint64()?,
            6 => self.s64 = r.zigzag64()?,
            7 => self.f32 = r.float()?,
            8 => self.f64 = r.double()?,
            9 => self.str = r.string()?,
            10 => self.raw = r.bytes()?,
            11 => self.msg = Some(read_message(r)?),
            12 if wt == WireType::LengthDelimited => {
                let mut packed = r.packed()?;
                while !packed.eof() {
                    let v = packed.int64()?;
                    r.push_element(&mut self.ints, v)?;
                }
            }
            12 => {
                let v = r.int64()?;
                r.push_element(&mut self.ints, v)?;
            }
            13 => {
                let v = r.string()?;
                r.push_element(&mut self.strs, v)?;
            }
            14 => {
                let v = read_message(r)?;
                r.push_element(&mut self.kids, v)?;
            }
            15 => {
                let (k, v) = read_map_entry(r, |r| r.string(), |r| r.string())?;
                if !self.smap.contains_key(&k) {
                    r.check_map(self.smap.len())?;
                }
                self.smap.insert(k, v);
            }
            16 => {
                let (k, v) = read_map_entry(r, |r| r.int64(), read_message)?;
                if !self.imap.contains_key(&k) {
                    r.check_map(self.imap.len())?;
                }
                self.imap.insert(k, v);
            }
            17 => self.big = Some(read_message(r)?),
            18 => self.dec = Some(read_message(r)?),
            19 => self.float = Some(read_message(r)?),
            _ => r.skip(wt)?,
        }
        Ok(())
    }

    fn field_name(num: u32) -> Option<&'static str> {
        nth(
            &[
                "B", "I32", "I64", "U32", "U64", "S64", "F32", "F64", "Str", "Raw", "Msg", "Ints",
                "Strs", "Kids", "SMap", "IMap", "Big", "Dec", "Float",
            ],
            num,
        )
    }
}

/// The Go runner's `Node`, which nests itself.
#[derive(Default, Debug)]
struct Node {
    child: Option<Box<Node>>,
    v: i32,
}

impl Message for Node {
    fn encode_to(&self, _: &mut Writer) {}

    fn merge_field(&mut self, num: u32, wt: WireType, r: &mut Reader<'_>) -> Result<()> {
        match num {
            1 => self.child = Some(Box::new(read_message(r)?)),
            2 => self.v = r.int32()?,
            _ => r.skip(wt)?,
        }
        Ok(())
    }

    fn field_name(num: u32) -> Option<&'static str> {
        nth(&["Child", "V"], num)
    }
}

/// Inputs whose verdict differs, by message and hex. Each fails the day it
/// matches, so its pin is removed.
///
/// A known field with wire type 6 or 7: protowire-go reads it by its kind
/// and accepts it. Those wire types name no encoding, and [`WireType`] has
/// no variant for them, so this port refuses the field as `corrupt field N`.
const PINNED: &[(&str, &str)] = &[("All", "1e01"), ("All", "4f0161")];

fn limits(spec: &str) -> Limits {
    let mut limits = Limits::default();
    if spec == "-" {
        return limits;
    }
    let (name, value) = spec.split_once('=').expect(spec);
    let value: usize = value.parse().expect(spec);
    match name {
        "repeated" => limits.max_repeated_count = value,
        "size" => limits.max_message_size = value,
        "depth" => limits.max_nesting_depth = value,
        _ => panic!("limit {spec}"),
    }
    limits
}

fn unhex(s: &str) -> Vec<u8> {
    (0..s.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(&s[i..i + 2], 16).expect("hex"))
        .collect()
}

/// What this port says about one input: `OK`, or `ERR` and the error.
fn verdict(message: &str, data: &[u8], limits: Limits) -> String {
    fn of<M: Message>(r: Result<M>) -> String {
        match r {
            Ok(_) => "OK".into(),
            Err(e) => format!("ERR\t{e}"),
        }
    }
    match message {
        "All" => of(unmarshal_with::<All>(data, limits)),
        "Envelope" => of(unmarshal_with::<Envelope>(data, limits)),
        "Node" => of(unmarshal_with::<Node>(data, limits)),
        _ => panic!("message {message}"),
    }
}

#[test]
fn every_input_meets_the_reference() {
    let mut rows: BTreeMap<(&str, bool), usize> = BTreeMap::new();
    let mut diffs = Vec::new();
    let mut pinned = 0;
    for line in ORACLE.lines().filter(|l| !l.starts_with('#')) {
        let c: Vec<&str> = line.split('\t').collect();
        let (message, spec, data) = (c[0], c[1], unhex(c[2]));
        let want = c[3..].join("\t");
        let got = verdict(message, &data, limits(spec));
        *rows.entry((message, want == "OK")).or_default() += 1;
        if PINNED.contains(&(message, c[2])) {
            pinned += 1;
            if got == want {
                diffs.push(format!("{message} {}: matches now; remove its pin", c[2]));
            }
            continue;
        }
        if got != want {
            diffs.push(format!(
                "{message} {spec} {}\n   got: {got}\n  want: {want}",
                c[2]
            ));
        }
    }
    // A positive marker: every row ran, by message and by protowire-go's
    // verdict.
    let count = |message, ok| rows.get(&(message, ok)).copied().unwrap_or(0);
    assert_eq!(
        [
            count("All", true),
            count("All", false),
            count("Envelope", true),
            count("Envelope", false),
            count("Node", true),
            count("Node", false),
        ],
        [25, 185, 5, 54, 0, 3],
        "rows by message and verdict"
    );
    assert_eq!(pinned, PINNED.len());
    assert!(
        diffs.is_empty(),
        "{} rows differ from protowire-go:\n{}",
        diffs.len(),
        diffs.join("\n")
    );
}
