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

use std::collections::HashMap;

use prost::Message as _;
use prost_reflect::{DescriptorPool, DynamicMessage, MessageDescriptor};
use protowire_pxf::annotations::{get_default, is_required};
use protowire_pxf::{
    apply_default, marshal, unmarshal, unmarshal_full, MarshalOptions, UnmarshalOptions,
};

const ORACLE: &str = include_str!("../testdata/conformance/api-oracle.tsv");

fn pools() -> HashMap<&'static str, DescriptorPool> {
    let sets: [(&str, &[u8]); 2] = [
        (
            "layered-test.binpb",
            include_bytes!("../testdata/layered-test.binpb"),
        ),
        ("d4-test.binpb", include_bytes!("../testdata/d4-test.binpb")),
    ];
    sets.into_iter()
        .map(|(name, fds)| (name, DescriptorPool::decode(fds).expect(name)))
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

fn unhex(s: &str) -> Vec<u8> {
    (0..s.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(&s[i..i + 2], 16).expect("hex"))
        .collect()
}

/// Whether protowire-go's error ends in a standard-library error it wraps
/// (`strconv`, `encoding/base64`, `time`). This port does not reproduce those
/// texts. Whether error parity reaches that far is open with the owner
/// (#144), so until it is decided these rows are compared up to the field the
/// error names, and their count is pinned.
fn go_library_tail(want: &str) -> bool {
    ["strconv.", "illegal base64 data", "parsing time "]
        .iter()
        .any(|m| want.contains(m))
}

/// An error's text up to and including the field it names:
/// `invalid default int32 "x" for field "i32"`.
fn head(e: &str) -> &str {
    let at = e.find(" for field \"").expect("names a field") + " for field \"".len();
    let end = at + e[at..].find('"').expect("closing quote") + 1;
    &e[..end]
}

/// Two messages are the same when their bytes decode to the same message:
/// map order on the wire is not the contract.
fn same(desc: &MessageDescriptor, got: &DynamicMessage, want_hex: &str) -> bool {
    let want = DynamicMessage::decode(desc.clone(), unhex(want_hex).as_slice()).expect("oracle");
    let got = DynamicMessage::decode(desc.clone(), got.encode_to_vec().as_slice()).unwrap();
    got == want
}

/// Checks every row, and returns the differences and the rows run by op.
fn run() -> (Vec<String>, HashMap<String, usize>, usize) {
    let pools = pools();
    let mut diffs = Vec::new();
    let mut tails = 0;
    let mut ops: HashMap<String, usize> = HashMap::new();
    for line in ORACLE.lines().filter(|l| !l.starts_with('#')) {
        let c: Vec<&str> = line.split('\t').collect();
        let (set, message, op) = (c[0], c[1], c[2]);
        let desc = pools[set].get_message_by_name(message).expect(message);
        *ops.entry(op.to_string()).or_default() += 1;
        let field = || desc.get_field_by_name(c[3]).expect(c[3]);
        let diff = match op {
            "full" | "full-skip" => {
                let opts = UnmarshalOptions {
                    skip_post_decode: op == "full-skip",
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
            "marshal" => {
                let got = unmarshal(c[3], &desc, UnmarshalOptions::default())
                    .and_then(|m| marshal(&m, &desc, MarshalOptions::default()));
                let got = match got {
                    Ok(text) => format!("OK\t{text}"),
                    Err(e) => format!("ERR\t{e}"),
                };
                let want = format!("{}\t{}", c[4], unescape(c[5]));
                (got != want).then(|| format!("marshal\n   got: {got:?}\n  want: {want:?}"))
            }
            "apply" => {
                let mut m = DynamicMessage::new(desc.clone());
                match (apply_default(&mut m, &field(), c[4]), c[5]) {
                    (Ok(()), "OK") => (!same(&desc, &m, c[6]))
                        .then(|| format!("message\n   got: {m}\n  want: {}", c[6])),
                    (Err(e), "ERR") if go_library_tail(c[6]) => {
                        tails += 1;
                        let (got, want) = (e.to_string(), unescape(c[6]));
                        (head(&got) != head(&want))
                            .then(|| format!("error head\n   got: {got}\n  want: {want}"))
                    }
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
        if let Some(d) = diff {
            diffs.push(format!("{message} {op} {:?}: {d}", &c[3..]));
        }
    }
    (diffs, ops, tails)
}

#[test]
fn every_case_meets_the_reference() {
    let (diffs, ops, tails) = run();
    // A positive marker: every op ran as often as the cases ask.
    let count = |op: &str| ops.get(op).copied().unwrap_or(0);
    assert_eq!(
        [
            "full",
            "full-skip",
            "marshal",
            "apply",
            "required",
            "default"
        ]
        .map(count),
        [13, 13, 1, 46, 3, 9]
    );
    // The rows compared up to their field, for a standard-library tail.
    assert_eq!(tails, 8);
    assert!(
        diffs.is_empty(),
        "{} rows differ from protowire-go:\n{}",
        diffs.len(),
        diffs.join("\n")
    );
}
