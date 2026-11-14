// SPDX-License-Identifier: MIT
// Copyright (c) 2026 TrendVidia, LLC.
//! The `Rewriter` held to protowire-go's (#131): each operation's verdict,
//! its error text, and the rewritten document, byte for byte.
//!
//! `testdata/conformance/rewrite-oracle.tsv` is what protowire-go did with
//! each case of `rewrite-cases.txt`, protowire-go's rewriter tests as data
//! (vendored from protowire-java, which replays the same rows). Its header
//! names the commit, and `scripts/gen-rewrite-oracle.sh` regenerates it;
//! `scripts/rewrite-oracle/main.go` says how a row is encoded and how an
//! operation's arguments become values, entries and nodes. This test does
//! the same with this port's types.
//!
//! An operation this port cannot express is counted, not run: an argument
//! that is Go's nil, or a negative offset, which Rust's types rule out.
//! protowire-go refuses each of them, so the rest of its case still runs.

use protowire_pxf::{parse_tolerant, Block, Document, Entry, IntVal, Position, Rewriter, Value};

const ORACLE: &str = include_str!("../testdata/conformance/rewrite-oracle.tsv");

fn b64(s: &str) -> Vec<u8> {
    let mut out = Vec::new();
    let (mut acc, mut bits) = (0u32, 0);
    for c in s.bytes() {
        let v = match c {
            b'A'..=b'Z' => c - b'A',
            b'a'..=b'z' => c - b'a' + 26,
            b'0'..=b'9' => c - b'0' + 52,
            b'+' => 62,
            b'/' => 63,
            b'=' => break,
            _ => panic!("base64 {c}"),
        };
        acc = acc << 6 | v as u32;
        bits += 6;
        if bits >= 8 {
            bits -= 8;
            out.push((acc >> bits) as u8);
        }
    }
    out
}

fn b64_encode(b: &[u8]) -> String {
    const A: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::new();
    for chunk in b.chunks(3) {
        let n = chunk.len();
        let v = (chunk[0] as u32) << 16
            | (*chunk.get(1).unwrap_or(&0) as u32) << 8
            | *chunk.get(2).unwrap_or(&0) as u32;
        for i in 0..4 {
            if i <= n {
                out.push(A[(v >> (18 - 6 * i) & 63) as usize] as char);
            } else {
                out.push('=');
            }
        }
    }
    out
}

fn text(s: &str) -> String {
    String::from_utf8(b64(s)).unwrap()
}

/// Why a row was not run.
#[derive(Debug, PartialEq, Eq, Hash, Clone, Copy, PartialOrd, Ord)]
enum Skip {
    /// An argument is Go's nil.
    Nil,
    /// A span offset is negative.
    Negative,
}

/// The value `x = <text>` holds, read tolerantly as Go's runner reads it.
fn value(text: &str) -> Result<Value, Skip> {
    if text == "<nil>" {
        return Err(Skip::Nil);
    }
    let (doc, _) = parse_tolerant(&format!("x = {text}"));
    match doc.entries.into_iter().next() {
        Some(Entry::Assignment(a)) => Ok(a.value),
        other => panic!("value {text:?}: {other:?}"),
    }
}

fn entries(text: &str) -> Vec<Entry> {
    if text.is_empty() {
        return Vec::new();
    }
    parse_tolerant(text).0.entries
}

/// The node a selector names, as the runner's `find`: `value:<key>#<n>` is
/// the n-th assignment's or map entry's value with that key, `block:<name>#<n>`
/// the n-th block with that name, in a depth-first walk.
enum Found<'d> {
    Value(&'d Value),
    Block(&'d Block),
    Unspanned,
}

fn find<'d>(doc: &'d Document, sel: &str) -> Result<Found<'d>, Skip> {
    match sel {
        "<nil>" => return Err(Skip::Nil),
        "<unspanned>" => return Ok(Found::Unspanned),
        _ => {}
    }
    let (kind, rest) = sel.split_once(':').unwrap();
    let (key, nth) = rest.split_once('#').unwrap();
    let mut n: i64 = nth.parse().unwrap();
    let mut got = None;
    fn walk<'d>(es: &'d [Entry], kind: &str, key: &str, n: &mut i64, got: &mut Option<Found<'d>>) {
        fn visit<'d>(
            v: &'d Value,
            kind: &str,
            key: &str,
            n: &mut i64,
            got: &mut Option<Found<'d>>,
        ) {
            match v {
                Value::Block(b) => walk(&b.entries, kind, key, n, got),
                Value::List(l) => {
                    for el in &l.elements {
                        visit(el, kind, key, n, got);
                    }
                }
                _ => {}
            }
        }
        for e in es {
            match e {
                Entry::Assignment(a) => {
                    if kind == "value" && a.key == key {
                        if *n == 0 && got.is_none() {
                            *got = Some(Found::Value(&a.value));
                        }
                        *n -= 1;
                    }
                    visit(&a.value, kind, key, n, got);
                }
                Entry::MapEntry(m) => {
                    if kind == "value" && m.key == key {
                        if *n == 0 && got.is_none() {
                            *got = Some(Found::Value(&m.value));
                        }
                        *n -= 1;
                    }
                    visit(&m.value, kind, key, n, got);
                }
                Entry::Block(b) => {
                    if kind == "block" && b.name == key {
                        if *n == 0 && got.is_none() {
                            *got = Some(Found::Block(b));
                        }
                        *n -= 1;
                    }
                    walk(&b.entries, kind, key, n, got);
                }
                Entry::TypeEntry(_) => {}
            }
        }
    }
    walk(&doc.entries, kind, key, &mut n, &mut got);
    Ok(got.unwrap_or_else(|| panic!("selector {sel:?} matches nothing")))
}

fn unspanned_value() -> Value {
    Value::Int(IntVal {
        pos: Position::default(),
        end: Position::default(),
        raw: "9".into(),
        prec: String::new(),
    })
}

fn unspanned_block() -> Block {
    Block {
        pos: Position::default(),
        end: Position::default(),
        name: "x".into(),
        name_quoted: false,
        entries: Vec::new(),
        leading_comments: Vec::new(),
        trailing_comments: Vec::new(),
    }
}

/// Why an operation cannot be expressed here, if it cannot.
fn skip_reason(o: &[&str]) -> Result<(), Skip> {
    match o[0] {
        "setspan" if o[1].starts_with('-') || o[2].starts_with('-') => Err(Skip::Negative),
        _ if o[1..].contains(&"<nil>") => Err(Skip::Nil),
        _ => Ok(()),
    }
}

/// Applies one operation. `Ok(Err(_))` is the Rewriter's refusal.
fn apply(r: &mut Rewriter, o: &[&str]) -> Result<Result<(), String>, Skip> {
    let doc = r.document();
    let res = match o[0] {
        "set" => r.set(o[1], &value(o[2])?),
        "remove" => r.remove(o[1]),
        "replace" => {
            let with = value(o[2])?;
            match find(&doc, o[1])? {
                Found::Value(old) => r.replace_value(old, &with),
                Found::Unspanned => r.replace_value(&unspanned_value(), &with),
                Found::Block(_) => panic!("replace of a block"),
            }
        }
        "setspan" => {
            let start: i64 = o[1].parse().unwrap();
            let end: i64 = o[2].parse().unwrap();
            if start < 0 || end < 0 {
                return Err(Skip::Negative);
            }
            r.set_span(start as usize, end as usize, o[3].as_bytes())
        }
        "append" => {
            if o[2] == "<nil>" {
                return Err(Skip::Nil);
            }
            let e = entries(o[2]).into_iter().next().unwrap();
            match find(&doc, o[1])? {
                Found::Block(b) => r.append_entry(b, &e),
                Found::Unspanned => r.append_entry(&unspanned_block(), &e),
                Found::Value(_) => panic!("append to a value"),
            }
        }
        "keyedby" => {
            r.keyed_by_field(o[1], o[2]);
            Ok(())
        }
        "setkeyed" => r.set_keyed(o[1], o[2], o[3], &value(o[4])?),
        "rmkeyed" => r.remove_keyed(o[1], o[2], o[3]),
        "rmel" => r.remove_keyed_element(o[1], o[2]),
        "insel" => r.insert_keyed_element(o[1], o[2], o[3], &entries(o[4])),
        "rename" => r.rename_keyed_element(o[1], o[2], o[3]),
        "move" => r.move_keyed_element(o[1], o[2], o[3]),
        other => panic!("unknown operation {other:?}"),
    };
    Ok(res.map_err(|e| e.to_string()))
}

/// What this port's Rewriter does with a case, in the oracle's encoding: a
/// result for each operation, `None` for one it cannot express, then the
/// document's.
fn run(doc: &str, ops: &[Vec<&str>]) -> Vec<Option<String>> {
    let mut r = match Rewriter::new(doc) {
        Ok(r) => r,
        Err(e) => return vec![Some(format!("new-err {e}"))],
    };
    let mut res = Vec::new();
    for o in ops {
        res.push(match apply(&mut r, o) {
            Ok(Ok(())) => Some("ok".to_string()),
            Ok(Err(e)) => Some(format!("err {e}")),
            Err(_) => None,
        });
    }
    res.push(Some(match r.bytes() {
        Ok(out) => format!("bytes {}", b64_encode(&out)),
        Err(e) => format!("err {e}"),
    }));
    res
}

#[test]
fn every_case_meets_the_reference() {
    let mut ran = 0;
    let mut skipped = std::collections::BTreeMap::new();
    let mut failures = Vec::new();
    for line in ORACLE.lines().filter(|l| !l.starts_with('#')) {
        let c: Vec<&str> = line.split('\t').collect();
        assert_eq!(c.len(), 4, "{line}");
        let (name, doc, ops, want) = (c[0], text(c[1]), text(c[2]), text(c[3]));
        let ops: Vec<Vec<&str>> = if ops.is_empty() {
            Vec::new()
        } else {
            ops.split('\x1e')
                .map(|o| o.split('\x1f').collect())
                .collect()
        };
        // An operation this port cannot express stages nothing in
        // protowire-go (it is refused), so the others' results and the
        // document stand without it.
        for o in &ops {
            if let Err(why) = skip_reason(o) {
                *skipped.entry(why).or_insert(0) += 1;
            }
        }
        let got = run(&doc, &ops);
        let want: Vec<&str> = want.split('\x1e').collect();
        assert_eq!(got.len(), want.len(), "{name}");
        let (got, want): (Vec<_>, Vec<_>) = got
            .into_iter()
            .zip(want)
            .filter_map(|(g, w)| g.map(|g| (g, w.to_string())))
            .unzip();
        ran += 1;
        if got != want {
            failures.push(format!(
                "{name}\n  ops:  {ops:?}\n  want: {want:?}\n  got:  {got:?}"
            ));
        }
    }
    assert!(
        failures.is_empty(),
        "{} of {ran} cases differ from protowire-go:\n{}",
        failures.len(),
        failures.join("\n")
    );
    // A positive marker that every row was read and run, so a change to the
    // oracle or to the skip rules shows here.
    assert_eq!(
        (ran, skipped.into_iter().collect::<Vec<_>>()),
        (412, vec![(Skip::Nil, 5), (Skip::Negative, 1)]),
        "cases run, operations skipped"
    );
}
