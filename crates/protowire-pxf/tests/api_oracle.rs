// SPDX-License-Identifier: MIT
// Copyright (c) 2026 TrendVidia, LLC.
//! The pxf API a layered-config caller uses (#127), held to protowire-go's:
//! decoding without the post-decode pass (`skip_post_decode`, Go's
//! `SkipPostDecode`), the paths a document set (`Presence::present_fields`,
//! Go's `Result.PresentFields`), applying one default (`apply_default`, Go's
//! `ApplyDefault`), and reading the annotations (`is_required`,
//! `get_default`).
//!
//! `testdata/conformance/api-oracle.tsv` is what protowire-go did with each
//! case of `api-cases.tsv`, over the same descriptor sets; its header names
//! the commit, and `scripts/gen-api-oracle.sh` regenerates it. The cases are
//! protowire-go's `layered_config_test.go` and a few more edges, over
//! `testdata/layered-test.proto`, which holds that test's schemas.

use std::cell::RefCell;
use std::collections::HashMap;

use prost::Message as _;
use prost_reflect::{DescriptorPool, DynamicMessage, MessageDescriptor};
use protowire_pxf::annotations::{get_default, is_required};
use protowire_pxf::{
    append_value, apply_default, format_value, marshal, parse, unmarshal, unmarshal_full, Entry,
    MarshalOptions, PoolResolver, SecretHook, TypeResolver, UnmarshalOptions,
};

const ORACLE: &str = include_str!("../testdata/conformance/api-oracle.tsv");

fn pools() -> HashMap<&'static str, DescriptorPool> {
    let sets: [(&str, &[u8]); 6] = [
        (
            "keyed-entry-name-test.binpb",
            include_bytes!("../testdata/keyed-entry-name-test.binpb"),
        ),
        (
            "layered-test.binpb",
            include_bytes!("../testdata/layered-test.binpb"),
        ),
        ("d4-test.binpb", include_bytes!("../testdata/d4-test.binpb")),
        ("test.binpb", include_bytes!("../testdata/test.binpb")),
        (
            "null-number-test.binpb",
            include_bytes!("../testdata/null-number-test.binpb"),
        ),
        (
            "secret-hook-test.binpb",
            include_bytes!("../testdata/secret-hook-test.binpb"),
        ),
    ];
    sets.into_iter()
        .map(|(name, fds)| (name, DescriptorPool::decode(fds).expect(name)))
        .collect()
}

/// The oracle's escapes undone: `\\`, `\t`, `\n`, and `\xNN` for a byte
/// that is not UTF-8. protowire-go's error can quote a document's invalid
/// bytes raw, which a Rust `String` cannot hold, so such a text is compared
/// as it reads: each invalid byte as U+FFFD (#170).
fn unescape(s: &str) -> String {
    let mut out = Vec::new();
    let mut chars = s.chars();
    while let Some(c) = chars.next() {
        if c != '\\' {
            out.extend_from_slice(c.encode_utf8(&mut [0; 4]).as_bytes());
            continue;
        }
        match chars.next() {
            Some('n') => out.push(b'\n'),
            Some('t') => out.push(b'\t'),
            Some('x') => {
                let hex: String = chars.by_ref().take(2).collect();
                out.push(u8::from_str_radix(&hex, 16).expect("\\xNN"));
            }
            Some(c) => out.extend_from_slice(c.encode_utf8(&mut [0; 4]).as_bytes()),
            None => out.push(b'\\'),
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}

fn unhex(s: &str) -> Vec<u8> {
    (0..s.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(&s[i..i + 2], 16).expect("hex"))
        .collect()
}

/// Two messages are the same when their bytes decode to the same message,
/// since map order on the wire is not the contract, or when the bytes are
/// equal, since a NaN equals nothing, not even itself.
fn same(desc: &MessageDescriptor, got: &DynamicMessage, want_hex: &str) -> bool {
    let got_bytes = got.encode_to_vec();
    let want = DynamicMessage::decode(desc.clone(), unhex(want_hex).as_slice()).expect("oracle");
    let got = DynamicMessage::decode(desc.clone(), got_bytes.as_slice()).unwrap();
    got == want || got_bytes == unhex(want_hex)
}

/// Rows that differ for a reason of their own, tracked elsewhere: a float
/// field set to -0, which prost-reflect does not write because it counts it
/// as unset, where protowire-go writes it (#98, held for prost-reflect).
/// The test fails the day one of them matches, so the pin is removed.
const WAITS_ON_98: &[(&str, &str)] = &[("f32", "-0")];

/// Documents whose row differs for a reason tracked in an issue of its own,
/// by op and document. Each fails the day it matches, so its pin is removed.
const PINNED: &[(&str, &str, u32)] = &[
    // A `@dataset` cell is kept as a parsed value, which cannot hold bytes
    // that are not UTF-8; protowire-go's keeps them.
    (
        "full",
        r#"@dataset test.v1.AllTypes (string_field) ("\xff")"#,
        191,
    ),
    // protowire-go quotes its resolver's error; this port's resolver gives
    // none.
    ("marshal-any", r#"payload { @type = "\xff" }"#, 192),
    (
        "marshal-any",
        r#"payload { @type = "nope.v1.Missing" }"#,
        192,
    ),
    // protowire-go refuses a quoted type name in the header.
    (
        "full",
        r#"@type "test.v1.AllTypes" string_field = "x""#,
        193,
    ),
];

/// Checks every row, and returns the differences and the rows run by op.
fn run() -> (Vec<String>, HashMap<String, usize>) {
    let pools = pools();
    let mut diffs = Vec::new();
    let mut ops: HashMap<String, usize> = HashMap::new();
    for line in ORACLE.lines().filter(|l| !l.starts_with('#')) {
        let c: Vec<&str> = line.split('\t').collect();
        let (set, message, op) = (c[0], c[1], c[2]);
        let desc = pools[set].get_message_by_name(message).expect(message);
        *ops.entry(op.to_string()).or_default() += 1;
        let field = || desc.get_field_by_name(c[3]).expect(c[3]);
        let diff = match op {
            "full" | "full-skip" | "discard" => {
                let opts = UnmarshalOptions {
                    skip_post_decode: op == "full-skip",
                    discard_unknown: op == "discard",
                    ..Default::default()
                };
                match (unmarshal_full(c[3], &desc, opts), c[4]) {
                    (Err(e), "ERR") => (e.to_string() != unescape(c[5]))
                        .then(|| format!("error\n   got: {e}\n  want: {}", unescape(c[5]))),
                    (Err(e), _) => Some(format!("refused: {e}")),
                    (Ok(_), "ERR") => Some(format!("accepted; want: {}", unescape(c[5]))),
                    (Ok((m, presence)), _) => {
                        let mut present: Vec<&str> = presence.present_fields().collect();
                        present.sort_unstable();
                        let mut nulls: Vec<&str> = presence.null_paths().collect();
                        nulls.sort_unstable();
                        let got = (present.join(","), nulls.join(","));
                        let want = (c[6].to_string(), c[7].to_string());
                        if !same(&desc, &m, c[5]) {
                            Some(format!("message\n   got: {m}\n  want: {}", c[5]))
                        } else {
                            (got != want)
                                .then(|| format!("paths\n   got: {got:?}\n  want: {want:?}"))
                        }
                    }
                }
            }
            "marshal" | "marshal-any" => {
                // marshal-any reads and writes an Any as @type sugar.
                let resolver = PoolResolver(&pools[set]);
                let resolver = (op == "marshal-any").then_some(&resolver as &dyn TypeResolver);
                let got = unmarshal(
                    c[3],
                    &desc,
                    UnmarshalOptions {
                        type_resolver: resolver,
                        ..Default::default()
                    },
                )
                .and_then(|m| {
                    marshal(
                        &m,
                        &desc,
                        MarshalOptions {
                            type_resolver: resolver,
                            ..Default::default()
                        },
                    )
                });
                let got = match got {
                    Ok(text) => format!("OK\t{text}"),
                    Err(e) => format!("ERR\t{e}"),
                };
                let want = format!("{}\t{}", c[4], unescape(c[5]));
                (got != want).then(|| format!("marshal\n   got: {got:?}\n  want: {want:?}"))
            }
            "format" => {
                let got = match parse(c[3]) {
                    Err(e) => format!("ERR\t{e}"),
                    Ok(doc) => {
                        let vals: Vec<String> = doc
                            .entries
                            .iter()
                            .filter_map(|e| match e {
                                Entry::Assignment(a) => Some(format_value(&a.value)),
                                _ => None,
                            })
                            .collect();
                        format!("OK\t{}", vals.join(" | "))
                    }
                };
                let want = format!("{}\t{}", c[4], unescape(c[5]));
                (got != want).then(|| format!("format\n   got: {got:?}\n  want: {want:?}"))
            }
            "secret" | "secret-plain" | "secret-err" => {
                let calls = RefCell::new(Vec::<String>::new());
                let fail = op == "secret-err";
                let record = |path: &str, value: &str| {
                    calls.borrow_mut().push(format!("{path}={value}"));
                    if fail {
                        Err("vault closed".into())
                    } else {
                        Ok(())
                    }
                };
                let hook: &SecretHook = &record;
                let resolver = PoolResolver(&pools[set]);
                let opts = UnmarshalOptions {
                    type_resolver: Some(&resolver),
                    on_secret_field: Some(hook),
                    ..Default::default()
                };
                let got = if op == "secret-plain" {
                    unmarshal(c[3], &desc, opts).map(|m| (m, String::new()))
                } else {
                    unmarshal_full(c[3], &desc, opts).map(|(m, presence)| {
                        let mut present: Vec<&str> = presence.present_fields().collect();
                        present.sort_unstable();
                        (m, present.join(","))
                    })
                };
                let calls = calls.into_inner().join(" ; ");
                match (got, c[4]) {
                    (Err(e), "ERR") => {
                        let got = format!("{e}\t{calls}");
                        let want = format!("{}\t{}", unescape(c[5]), unescape(c[7]));
                        (got != want).then(|| format!("error\n   got: {got:?}\n  want: {want:?}"))
                    }
                    (Err(e), _) => Some(format!("refused: {e} (calls {calls})")),
                    (Ok(_), "ERR") => Some(format!("accepted; want: {}", unescape(c[5]))),
                    (Ok((m, present)), _) => {
                        let got = (present, calls);
                        let want = (c[6].to_string(), unescape(c[7]));
                        if !same(&desc, &m, c[5]) {
                            Some(format!("message\n   got: {m}\n  want: {}", c[5]))
                        } else {
                            (got != want)
                                .then(|| format!("paths, calls\n   got: {got:?}\n  want: {want:?}"))
                        }
                    }
                }
            }
            "apply" => {
                let mut m = DynamicMessage::new(desc.clone());
                match (apply_default(&mut m, &field(), c[4]), c[5]) {
                    (Ok(()), "OK") => (!same(&desc, &m, c[6]))
                        .then(|| format!("message\n   got: {m}\n  want: {}", c[6])),
                    (Err(e), "ERR") => (e.to_string() != unescape(c[6]))
                        .then(|| format!("error\n   got: {e}\n  want: {}", unescape(c[6]))),
                    (Ok(()), _) => Some(format!("applied ({m}); want: {}", unescape(c[6]))),
                    (Err(e), _) => Some(format!("refused: {e}")),
                }
            }
            "required" => {
                let got = is_required(&field()).to_string();
                (got != c[4]).then(|| format!("got {got}, want {}", c[4]))
            }
            "default" => {
                let def = get_default(&field());
                let got = format!("{}\t{}", def.is_some(), def.unwrap_or_default());
                let want = format!("{}\t{}", c[4], unescape(c[5]));
                (got != want).then(|| format!("got {got:?}, want {want:?}"))
            }
            other => panic!("op {other:?}"),
        };
        let pinned = match PINNED.iter().find(|(o, doc, _)| *o == op && *doc == c[3]) {
            Some((_, _, issue)) => Some(*issue),
            None => (op == "apply" && WAITS_ON_98.contains(&(c[3], c[4]))).then_some(98),
        };
        match (diff, pinned) {
            (Some(_), Some(_)) => *ops.entry("pinned".into()).or_default() += 1,
            (None, Some(issue)) => {
                diffs.push(format!("{c:?}: matches now; remove its #{issue} pin"))
            }
            (Some(d), None) => diffs.push(format!("{message} {op} {:?}: {d}", &c[3..])),
            (None, None) => {}
        }
    }
    (diffs, ops)
}

#[test]
fn every_case_meets_the_reference() {
    let (diffs, ops) = run();
    // A positive marker: every op ran as often as the cases ask.
    let count = |op: &str| ops.get(op).copied().unwrap_or(0);
    assert_eq!(
        [
            "full",
            "full-skip",
            "discard",
            "marshal",
            "marshal-any",
            "format",
            "secret",
            "secret-plain",
            "secret-err",
            "apply",
            "required",
            "default"
        ]
        .map(count),
        [95, 13, 5, 5, 6, 10, 37, 37, 6, 130, 3, 9]
    );
    assert_eq!(count("pinned"), WAITS_ON_98.len() + PINNED.len());
    assert!(
        diffs.is_empty(),
        "{} rows differ from protowire-go:\n{}",
        diffs.len(),
        diffs.join("\n")
    );
}

/// protowire-go's `TestAppendValuePreservesDst`: the text goes after what
/// the buffer held.
#[test]
fn append_value_keeps_the_buffer() {
    let doc = parse("a = 7").unwrap();
    let Entry::Assignment(a) = &doc.entries[0] else {
        panic!("an assignment")
    };
    let mut dst = String::from("prefix=");
    append_value(&mut dst, &a.value);
    assert_eq!(dst, "prefix=7");
}
