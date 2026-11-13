// SPDX-License-Identifier: MIT
// Copyright (c) 2026 TrendVidia, LLC.
//! Keyed repeated field errors (draft -01 §3.13) as protowire-go reports
//! them (#130): the typed error a decode refuses with
//! (`PxfError::keyed_error`, Go's `KeyedError` through `errors.As`), and
//! every one in a parsed document (`keyed_diagnostics`, Go's
//! `KeyedDiagnostics`).
//!
//! `testdata/conformance/keyed-oracle.tsv` is what protowire-go did with each
//! case of `keyed-cases.tsv`; its header names the commit, and
//! `scripts/gen-keyed-oracle.sh` regenerates it. The cases are the fixtures
//! of `testdata/keyed/`, every document of protowire-go's `keyed_test.go`
//! and `keyed_entry_name_presence_test.go`, and more shapes: several errors
//! in one document, Any bodies, map values, nesting and `@dataset` cells.

use std::collections::HashMap;
use std::io::Cursor;

use prost_reflect::{DescriptorPool, MessageDescriptor};
use protowire_pxf::{
    keyed_diagnostics, parse, unmarshal, unmarshal_full, DatasetReader, KeyedError, PoolResolver,
    PxfError, UnmarshalOptions,
};

const ORACLE: &str = include_str!("../testdata/conformance/keyed-oracle.tsv");

fn pools() -> HashMap<&'static str, DescriptorPool> {
    let sets: [(&str, &[u8]); 3] = [
        (
            "keyed/keyed.binpb",
            include_bytes!("../testdata/keyed/keyed.binpb"),
        ),
        (
            "keyed-undo-test.binpb",
            include_bytes!("../testdata/keyed-undo-test.binpb"),
        ),
        (
            "keyed-entry-name-test.binpb",
            include_bytes!("../testdata/keyed-entry-name-test.binpb"),
        ),
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

fn keyed(k: &KeyedError) -> String {
    format!(
        "{}|{}|{}|{}:{}|{}|{}",
        k.kind, k.field, k.key, k.pos.line, k.pos.column, k.pos.offset, k.msg
    )
}

/// The oracle's verdict on one decode: OK, KEYED and the keyed error then
/// the error's text, or ERR and the text.
fn verdict<T>(r: &Result<T, PxfError>) -> String {
    match r {
        Ok(_) => "OK".to_string(),
        Err(e) => match e.keyed_error() {
            Some(k) => {
                // Outside a `@dataset` cell, the keyed error prints as the
                // decode error does.
                assert_eq!(k.to_string(), e.to_string());
                format!("KEYED {} {e}", keyed(k))
            }
            None => format!("ERR {e}"),
        },
    }
}

#[derive(Debug, Default, PartialEq)]
struct Marker {
    docs: usize,
    scans: usize,
    keyed_verdicts: usize,
    diagnostics: usize,
    waits_on_178: usize,
}

#[test]
fn every_case_meets_the_reference() {
    let pools = pools();
    let mut seen = Marker::default();
    let mut failures = Vec::new();
    for line in ORACLE.lines() {
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let c: Vec<&str> = line.split('\t').collect();
        assert_eq!(c.len(), 7, "{line}");
        let pool = &pools[c[0]];
        let desc: MessageDescriptor = pool.get_message_by_name(c[1]).expect(c[1]);
        let resolver = PoolResolver(pool);
        let opts = || UnmarshalOptions {
            type_resolver: Some(&resolver),
            ..Default::default()
        };
        let doc = unescape(c[3]);
        let got: [String; 3] = match c[2] {
            "doc" => {
                seen.docs += 1;
                let diag = match parse(&doc) {
                    Err(e) => format!("PARSE {e}"),
                    Ok(parsed) => {
                        let ds = keyed_diagnostics(&parsed, &desc);
                        seen.diagnostics += ds.len();
                        if ds.is_empty() {
                            "NONE".to_string()
                        } else {
                            ds.iter().map(keyed).collect::<Vec<_>>().join(" ; ")
                        }
                    }
                };
                [
                    verdict(&unmarshal(&doc, &desc, opts())),
                    verdict(&unmarshal_full(&doc, &desc, opts())),
                    diag,
                ]
            }
            "scan" => {
                seen.scans += 1;
                let v = match DatasetReader::new(Cursor::new(doc.clone().into_bytes())) {
                    Err(e) => format!("NEW {}", verdict::<()>(&Err(e))),
                    Ok(mut tr) => {
                        let mut rows = 0;
                        loop {
                            match tr.scan_one(&desc, opts()) {
                                Ok(Some(_)) => rows += 1,
                                Ok(None) => break format!("OK {rows}"),
                                Err(e) => match e.keyed_error() {
                                    Some(k) => {
                                        // protowire-go returns a keyed error
                                        // in a cell unmoved, so its text is
                                        // the keyed error's own; this port's
                                        // is the row's, naming the column,
                                        // as protowire-cpp and -java write
                                        // it (#178).
                                        assert!(
                                            e.msg.starts_with("column ")
                                                && e.msg.ends_with(&k.msg)
                                                && e.pos.line > 1,
                                            "a cell's keyed error is not at its row: {e}"
                                        );
                                        seen.waits_on_178 += 1;
                                        break format!("ROW {rows} KEYED {} {k}", keyed(k));
                                    }
                                    None => break format!("ROW {rows} {}", verdict::<()>(&Err(e))),
                                },
                            }
                        }
                    }
                };
                [v, "-".to_string(), "-".to_string()]
            }
            op => panic!("unknown op {op:?}"),
        };
        for (i, (got, want)) in got.iter().zip(&c[4..]).enumerate() {
            let want = unescape(want);
            if got.starts_with("KEYED") || got.contains(" KEYED ") {
                seen.keyed_verdicts += 1;
            }
            if *got != want {
                failures.push(format!(
                    "{} {} {:?} [{}]\n   got: {got}\n  want: {want}",
                    c[1],
                    c[2],
                    doc,
                    ["unmarshal", "full", "diagnostics"][i]
                ));
            }
        }
    }
    assert!(
        failures.is_empty(),
        "{} differ from protowire-go:\n{}",
        failures.len(),
        failures.join("\n")
    );
    assert_eq!(
        seen,
        Marker {
            docs: 85,
            scans: 6,
            keyed_verdicts: 97,
            diagnostics: 49,
            waits_on_178: 5,
        }
    );
}
