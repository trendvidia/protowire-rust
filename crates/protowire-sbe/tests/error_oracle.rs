// SPDX-License-Identifier: MIT
// Copyright (c) 2026 TrendVidia, LLC.
//! SBE errors held to protowire-go's, word for word (#144): each row of
//! `testdata/conformance/sbe-error-oracle.tsv` is an input and what
//! protowire-go said about it. The decode rows are every prefix of a full
//! encoding of each `sbe-test.proto` message and hand-made damage to them,
//! read by `unmarshal` and by `Codec::view`; then View accessor reads,
//! SOFH streams, and XML schemas for `xml_to_proto`, every prefix of one among
//! them. `scripts/gen-sbe-error-oracle.sh` regenerates the file;
//! `scripts/sbe-error-oracle/main.go` says what each column holds.
//!
//! protowire-go's View accessors panic where this port's return an error,
//! and a panic's text is compared with the error's.

use std::collections::BTreeMap;
use std::io::Cursor;

use prost_reflect::{DescriptorPool, DynamicMessage};
use protowire_sbe::{unmarshal, xml_to_proto, Codec, Limits, View};

const FDS: &[u8] = include_bytes!("../testdata/sbe-test.binpb");
const ORACLE: &str = include_str!("../testdata/conformance/sbe-error-oracle.tsv");

/// Rows whose verdict differs, by op and input hex. Each fails the day it
/// matches, so its pin is removed.
///
/// An XML schema that is not UTF-8: protowire-go's `XMLToProto` takes bytes
/// and refuses it (`XML syntax error on line 1: invalid UTF-8`), while
/// [`xml_to_proto`] takes a `&str`, so such a schema cannot reach it. The
/// test reads these rows as `NOT UTF-8`.
const PINNED: &[(&str, &str)] = &[
    ("xml", "ff"),
    (
        "xml",
        "3c6d657373616765536368656d61207061636b6167653d22ff222f3e",
    ),
];

fn unhex(s: &str) -> Vec<u8> {
    (0..s.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(&s[i..i + 2], 16).expect("hex"))
        .collect()
}

fn bytes_text(b: &[u8]) -> String {
    let hex = |b: &[u8]| b.iter().map(|x| format!("{x:02x}")).collect::<String>();
    if b.len() <= 64 {
        return hex(b);
    }
    let mut h: u64 = 0xcbf29ce484222325;
    for &x in b {
        h ^= x as u64;
        h = h.wrapping_mul(0x100000001b3);
    }
    format!("{}..len={},fnv={h:016x}", hex(&b[..16]), b.len())
}

/// The error's text as the oracle writes it.
fn text(e: impl std::fmt::Display) -> String {
    e.to_string().replace('\t', " ").replace('\n', "\\n")
}

/// Go's `strconv.Quote`, for the strings the oracle reads.
fn quote(s: &str) -> String {
    let mut out = String::from("\"");
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            c if (c as u32) < 0x20 || c as u32 == 0x7f => {
                out.push_str(&format!("\\x{:02x}", c as u32))
            }
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

struct Fixture {
    pool: DescriptorPool,
}

impl Fixture {
    fn codec(&self, limits: &str) -> Codec {
        let mut l = Limits::default();
        if limits != "-" {
            let (name, v) = limits.split_once('=').expect(limits);
            let v: usize = v.parse().expect(limits);
            match name {
                "size" => l.max_message_size = v,
                "repeated" => l.max_repeated_count = v,
                _ => panic!("limit {limits}"),
            }
        }
        let file = self.pool.get_file_by_name("sbe-test.proto").expect("file");
        Codec::from_files_with_limits(&[file], l).expect("codec")
    }

    fn message(&self, name: &str) -> DynamicMessage {
        let md = self
            .pool
            .all_messages()
            .find(|m| m.name() == name)
            .unwrap_or_else(|| panic!("no message {name}"));
        DynamicMessage::new(md)
    }
}

/// A View accessor read, as `call` in the Go runner does it.
fn call(view: View<'_>, acc: &str, path: &str) -> String {
    let parts: Vec<&str> = path.split('.').collect();
    let mut v = view;
    for p in &parts[..parts.len() - 1] {
        let next = match p.split_once('#') {
            Some((g, i)) => v.group(g).and_then(|g| g.entry(i.parse().expect("entry"))),
            None => v.composite(p),
        };
        v = match next {
            Ok(v) => v,
            Err(e) => return format!("PANIC\t{e}"),
        };
    }
    let f = parts[parts.len() - 1];
    let out = match acc {
        "Int" => v.int_field(f).map(|n| n.to_string()),
        "Uint" => v.uint_field(f).map(|n| n.to_string()),
        "Float" => v.float_field(f).map(|n| n.to_string()),
        "Bool" => v.bool_field(f).map(|b| b.to_string()),
        "Enum" => v.enum_field(f).map(|n| n.to_string()),
        "String" => v.string_field(f).map(|s| quote(&s)),
        "Bytes" => v
            .bytes_field(f)
            .map(|b| b.iter().map(|x| format!("{x:02x}")).collect()),
        "Composite" => v.composite(f).map(|_| "view".to_string()),
        "Len" => v.group(f).map(|g| g.len().to_string()),
        _ => panic!("accessor {acc}"),
    };
    match out {
        Ok(s) => format!("OK\t{s}"),
        Err(e) => format!("PANIC\t{e}"),
    }
}

fn stream(codec: &Codec, max: usize, data: &[u8]) -> String {
    let mut d = codec.new_decoder(Cursor::new(data));
    if max > 0 {
        d.set_max_frame_size(max);
    }
    let mut out = Vec::new();
    let mut last: Option<String> = None;
    for _ in 0..10 {
        let mut msg = DynamicMessage::new(
            codec
                .template("test.v1.Simple")
                .expect("Simple")
                .desc
                .clone(),
        );
        match d.decode(&mut msg) {
            Ok(false) => {
                out.push("EOF".to_string());
                return out.join("|");
            }
            Ok(true) => {
                last = None;
                out.push("OK".to_string());
            }
            Err(e) => {
                let t = text(&e);
                if last.as_deref() == Some(t.as_str()) {
                    return out.join("|");
                }
                out.push(format!("ERR:{t}"));
                last = Some(t);
            }
        }
    }
    panic!("stream did not end")
}

fn verdict(fx: &Fixture, c: &[&str], data: &[u8]) -> String {
    let (op, subject, arg) = (c[0], c[1], c[2]);
    let res = |r: Result<(), protowire_sbe::SbeError>| match r {
        Ok(()) => "OK".to_string(),
        Err(e) => format!("ERR\t{}", text(e)),
    };
    match op {
        "unmarshal" => {
            let mut msg = fx.message(subject);
            res(unmarshal(&fx.codec(arg), &mut msg, data))
        }
        "view" => res(fx.codec(arg).view(data).map(|_| ())),
        "call" => {
            let codec = fx.codec("-");
            let view = codec.view(data).expect("view");
            let (acc, path) = arg.split_once(':').expect("accessor");
            call(view, acc, path)
        }
        "stream" => stream(&fx.codec("-"), arg.parse().expect("max"), data),
        "xml" => match std::str::from_utf8(data) {
            Err(_) => "NOT UTF-8".to_string(),
            Ok(s) => match xml_to_proto(s) {
                Ok(out) => format!("OK\t{}", bytes_text(out.as_bytes())),
                Err(e) => format!("ERR\t{}", text(e)),
            },
        },
        _ => panic!("op {op}"),
    }
}

#[test]
fn every_input_meets_the_reference() {
    let fx = Fixture {
        pool: DescriptorPool::decode(FDS).expect("decode sbe-test.binpb"),
    };
    let mut rows: BTreeMap<&str, usize> = BTreeMap::new();
    let mut diffs = Vec::new();
    let mut pinned = 0;
    for line in ORACLE.lines().filter(|l| !l.starts_with('#')) {
        let c: Vec<&str> = line.split('\t').collect();
        let data = unhex(c[3]);
        let want = c[4..].join("\t");
        let got = verdict(&fx, &c, &data);
        *rows.entry(c[0]).or_default() += 1;
        if PINNED.contains(&(c[0], c[3])) {
            pinned += 1;
            if got == want {
                diffs.push(format!("{} {}: matches now; remove its pin", c[0], c[3]));
            }
            continue;
        }
        if got != want {
            diffs.push(format!(
                "{} {} {} {}\n   got: {got}\n  want: {want}",
                c[0], c[1], c[2], c[3]
            ));
        }
    }
    // A positive marker: every row ran, by op.
    assert_eq!(
        rows.into_iter().collect::<Vec<_>>(),
        [
            ("call", 60),
            ("stream", 75),
            ("unmarshal", 233),
            ("view", 233),
            ("xml", 1001),
        ],
        "rows by op"
    );
    assert_eq!(pinned, PINNED.len());
    assert!(
        diffs.is_empty(),
        "{} rows differ from protowire-go:\n{}",
        diffs.len(),
        diffs.join("\n")
    );
}
