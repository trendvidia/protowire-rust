// SPDX-License-Identifier: MIT
// Copyright (c) 2026 TrendVidia, LLC.
//! A descriptor compiled before protowire v1.12.0 carries its options at the
//! extension numbers retired then (STABILITY.md promise 3, draft -01 says to
//! diagnose it, #133). Bind-time validation reports each one, and the SBE
//! codec and XML export say so rather than reporting the option missing.
//!
//! `testdata/conformance/retired-oracle.tsv` is what protowire-go did with
//! each case of `retired-cases.tsv`, over the descriptor set the oracle holds
//! for it: `ValidateFile`, `Unmarshal`, `sbe.NewCodec` and `sbe.ProtoToXML`.
//! `scripts/gen-retired-oracle.sh` regenerates it. The cases are protowire-go's
//! own (`encoding/pxf/retired_test.go`, `encoding/sbe/retired_test.go`) and a
//! few more edges. The test lives here because it holds both crates.

use prost::Message as _;
use prost_reflect::{DescriptorPool, DynamicMessage};
use protowire::pxf::{unmarshal, validate_file, UnmarshalOptions};
use protowire::sbe::{proto_to_xml, Codec};

const ORACLE: &str = include_str!("../testdata/conformance/retired-oracle.tsv");

fn unhex(s: &str) -> Vec<u8> {
    (0..s.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(&s[i..i + 2], 16).expect("hex"))
        .collect()
}

/// The oracle's escapes undone: `\\`, `\t` and `\n`.
fn unescape(s: &str) -> String {
    let mut out = String::new();
    let mut chars = s.chars();
    while let Some(c) = chars.next() {
        if c != '\\' {
            out.push(c);
            continue;
        }
        match chars.next() {
            Some('n') => out.push('\n'),
            Some('t') => out.push('\t'),
            Some(c) => out.push(c),
            None => out.push('\\'),
        }
    }
    out
}

/// A verdict as the oracle writes it: `OK` and a payload, or `ERR` and the
/// error.
fn verdict<T, E: std::fmt::Display>(r: Result<T, E>, ok: impl FnOnce(T) -> String) -> String {
    match r {
        Ok(v) => format!("OK\t{}", ok(v)),
        Err(e) => format!("ERR\t{e}"),
    }
}

/// Checks every row, and returns the differences and how many rows ran.
fn run() -> (Vec<String>, usize) {
    let mut diffs = Vec::new();
    let mut rows = 0;
    let mut pool: Option<DescriptorPool> = None;
    for line in ORACLE.lines().filter(|l| !l.starts_with('#')) {
        let c: Vec<&str> = line.split('\t').collect();
        let (id, op) = (c[0], c[1]);
        if op == "fds" {
            pool = Some(DescriptorPool::decode(unhex(c[2]).as_slice()).expect(id));
            continue;
        }
        rows += 1;
        let pool = pool.as_ref().expect("fds row first");
        let uses = pool.get_file_by_name("uses.proto");
        let (file, msg, doc) = match uses {
            Some(f) => (f, "uses.v1.U", r#"m { x = "v" }"#),
            None => (
                pool.get_file_by_name("stale.proto").expect("stale.proto"),
                "stale.v1.M",
                r#"x = "v""#,
            ),
        };
        let got = match op {
            "validate" => {
                let vs: Vec<String> = validate_file(&file)
                    .iter()
                    .map(|v| format!("[{}] {v}", v.kind))
                    .collect();
                format!("{}\t{}", vs.len(), vs.join(" | "))
            }
            "unmarshal" => {
                let desc = pool.get_message_by_name(msg).expect(msg);
                verdict(unmarshal(doc, &desc, UnmarshalOptions::default()), |m| {
                    hex(&m)
                })
            }
            "sbe-codec" => verdict(Codec::from_files(std::slice::from_ref(&file)), |_| {
                String::new()
            }),
            "sbe-xml" => verdict(proto_to_xml(&file), |x| x),
            other => panic!("{id}: op {other:?}"),
        };
        let want = format!("{}\t{}", c[2], unescape(c.get(3).copied().unwrap_or("")));
        if got != want {
            diffs.push(format!("{id} {op}\n   got: {got}\n  want: {want}"));
        }
    }
    (diffs, rows)
}

fn hex(m: &DynamicMessage) -> String {
    m.encode_to_vec()
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

#[test]
fn every_case_meets_the_reference() {
    let (diffs, rows) = run();
    // A positive marker: 16 cases, four rows each, all ran.
    assert_eq!(rows, 64);
    assert!(
        diffs.is_empty(),
        "{} of {rows} rows differ from protowire-go:\n{}",
        diffs.len(),
        diffs.join("\n")
    );
}
