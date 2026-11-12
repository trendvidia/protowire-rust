// SPDX-License-Identifier: MIT
// Copyright (c) 2026 TrendVidia, LLC.
//! A message with a well-known type's full name is that type only when it has
//! the type's shape (draft -01 §wkt-shapes, protowire-go#224, #120). Bind-time
//! validation reports one of the wrong shape, and where validation does not run
//! it is an ordinary message, with no panic.
//!
//! `testdata/conformance/wkt-shape-oracle.tsv` is what protowire-go did with
//! each case of `wkt-shape-cases.tsv`, over the descriptor set the oracle holds
//! for it: the violations, four decodes of each document, and a marshal.
//! `scripts/gen-wkt-shape-oracle.sh` regenerates it. The cases are protowire-go's
//! own (`wkt_shape_test.go`) and a few more edges.

use std::cell::RefCell;
use std::panic::{catch_unwind, AssertUnwindSafe};

use prost::Message as _;
use prost_reflect::{DescriptorPool, DynamicMessage, Kind, MapKey, MessageDescriptor, Value};
use protowire_pxf::{
    marshal, unmarshal, unmarshal_full, validate_descriptor, MarshalOptions, SecretHook,
    UnmarshalOptions,
};

const ORACLE: &str = include_str!("../testdata/conformance/wkt-shape-oracle.tsv");

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

fn panic_text(p: Box<dyn std::any::Any + Send>) -> String {
    p.downcast_ref::<String>()
        .cloned()
        .or_else(|| p.downcast_ref::<&str>().map(|s| s.to_string()))
        .unwrap_or_default()
}

/// H with every message field holding one empty message: the singular field
/// set, the list one element long, the map one entry "k". The oracle's
/// `filled`.
fn filled(desc: &MessageDescriptor) -> DynamicMessage {
    let mut m = DynamicMessage::new(desc.clone());
    for fd in desc.fields() {
        if fd.is_map() {
            let Kind::Message(entry) = fd.kind() else {
                continue;
            };
            if let Kind::Message(value) = entry.map_entry_value_field().kind() {
                let mut map = std::collections::HashMap::new();
                map.insert(
                    MapKey::String("k".into()),
                    Value::Message(DynamicMessage::new(value)),
                );
                m.set_field(&fd, Value::Map(map));
            }
        } else if let Kind::Message(sub) = fd.kind() {
            let empty = Value::Message(DynamicMessage::new(sub));
            let value = if fd.is_list() {
                Value::List(vec![empty])
            } else {
                empty
            };
            m.set_field(&fd, value);
        }
    }
    m
}

/// One decode against the oracle's verdict: OK and the message's bytes, or
/// ERR and the error. A difference is returned, not asserted, so a run
/// reports every row that differs.
fn decode_diff(
    desc: &MessageDescriptor,
    got: std::thread::Result<Result<DynamicMessage, protowire_pxf::PxfError>>,
    want: &[&str],
) -> Option<String> {
    let got = match got {
        Err(p) => return Some(format!("panicked: {}", panic_text(p))),
        Ok(r) => r,
    };
    match (want[0], got) {
        ("ERR", Err(e)) if e.to_string() == unescape(want[1]) => None,
        ("ERR", Err(e)) => Some(format!("error\n   got: {e}\n  want: {}", unescape(want[1]))),
        ("ERR", Ok(m)) => Some(format!("accepted ({m}); want: {}", unescape(want[1]))),
        ("OK", Err(e)) => Some(format!("refused: {e}")),
        ("OK", Ok(m)) => {
            let want = DynamicMessage::decode(desc.clone(), unhex(want[1]).as_slice())
                .expect("oracle bytes");
            // Compared on the wire: a field set to its zero value is the
            // same message as one left unset, and both encode alike.
            (m.encode_to_vec() != want.encode_to_vec())
                .then(|| format!("message\n   got: {m}\n  want: {want}"))
        }
        (other, _) => Some(format!("oracle verdict {other:?}")),
    }
}

/// Checks every row, and returns the differences and the count of rows by op.
fn run() -> (Vec<String>, [usize; 5]) {
    let mut diffs = Vec::new();
    // fds, validate, decodes, marshal, decodes with the Secret hook.
    let mut counts = [0; 5];
    let mut pool = None;
    let mut h: Option<MessageDescriptor> = None;
    for line in ORACLE.lines().filter(|l| !l.starts_with('#')) {
        let c: Vec<&str> = line.split('\t').collect();
        let (id, op, doc) = (c[0], c[1], c[2]);
        if op == "fds" {
            let p = DescriptorPool::decode(unhex(c[3]).as_slice()).expect(id);
            h = p.all_messages().find(|m| m.name() == "H");
            pool = Some(p);
            counts[0] += 1;
            continue;
        }
        let _ = &pool;
        let desc = h.clone().expect("fds row first");
        let diff = match op {
            "validate" => {
                counts[1] += 1;
                let vs: Vec<String> = validate_descriptor(&desc)
                    .iter()
                    .map(|v| format!("[{}] {v}", v.kind))
                    .collect();
                let got = format!("{}\t{}", vs.len(), vs.join(" | "));
                let want = format!("{}\t{}", c[3], unescape(c[4]));
                (got != want).then(|| format!("violations\n   got: {got}\n  want: {want}"))
            }
            "unmarshal" | "unmarshal-skip" | "full" | "full-skip" => {
                counts[2] += 1;
                let opts = || UnmarshalOptions {
                    skip_validate: op.ends_with("-skip"),
                    ..Default::default()
                };
                let got = catch_unwind(AssertUnwindSafe(|| {
                    if op.starts_with("full") {
                        unmarshal_full(doc, &desc, opts()).map(|(m, _)| m)
                    } else {
                        unmarshal(doc, &desc, opts())
                    }
                }));
                decode_diff(&desc, got, &c[3..])
            }
            // A Secret of the wrong shape is an ordinary message, so the
            // hook never fires for it (#127, #120).
            "hook" | "hook-skip" => {
                counts[4] += 1;
                let calls = RefCell::new(Vec::<String>::new());
                let record = |path: &str, value: &str| {
                    calls.borrow_mut().push(format!("{path}={value}"));
                    Ok(())
                };
                let hook: &SecretHook = &record;
                let opts = UnmarshalOptions {
                    skip_validate: op == "hook-skip",
                    on_secret_field: Some(hook),
                    ..Default::default()
                };
                let got = catch_unwind(AssertUnwindSafe(|| unmarshal(doc, &desc, opts)));
                let calls = calls.into_inner().join(" ; ");
                decode_diff(&desc, got, &c[3..]).or_else(|| {
                    (calls != unescape(c[6]))
                        .then(|| format!("hook calls\n   got: {calls}\n  want: {}", c[6]))
                })
            }
            "marshal" => {
                counts[3] += 1;
                let msg = filled(&desc);
                let got = catch_unwind(AssertUnwindSafe(|| {
                    marshal(&msg, &desc, MarshalOptions::default())
                }));
                let want = (c[3], unescape(c[4]));
                match got {
                    Err(p) => Some(format!("panicked: {}", panic_text(p))),
                    Ok(Ok(text)) if want.0 == "OK" && text == want.1 => None,
                    Ok(Err(e)) if want.0 == "ERR" && e.to_string() == want.1 => None,
                    Ok(got) => Some(format!("marshal\n   got: {got:?}\n  want: {want:?}")),
                }
            }
            other => panic!("{id}: op {other:?}"),
        };
        if let Some(d) = diff {
            diffs.push(format!("{id} {op} {doc:?}: {d}"));
        }
    }
    (diffs, counts)
}

#[test]
fn every_case_meets_the_reference() {
    let (diffs, counts) = run();
    // A positive marker: every case ran, all four kinds of row among them.
    assert_eq!(counts, [45, 45, 552, 45, 108]);
    assert!(
        diffs.is_empty(),
        "{} of {} rows differ from protowire-go:\n{}",
        diffs.len(),
        counts[1] + counts[2] + counts[3],
        diffs.join("\n")
    );
}
