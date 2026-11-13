// SPDX-License-Identifier: MIT
// Copyright (c) 2026 TrendVidia, LLC.
//! The `@dataset` writer (`append_dataset_header`, `append_dataset_row`),
//! held to protowire-go's `AppendDatasetHeader` and `AppendDatasetRow` (#129).
//!
//! `testdata/conformance/dataset-write-oracle.tsv` is what protowire-go did
//! with each case of `dataset-write-cases.tsv`: the cases of its
//! `dataset_write_test.go` and `dataset_write_required_list_test.go`, and rows
//! made by those tests' random generators. For each message it holds the
//! message's bytes, the header and row protowire-go wrote (or its error), the
//! message its reader read back from that table, and its `Marshal` text. Its
//! header names the commit, and `scripts/gen-dataset-write-oracle.sh`
//! regenerates it.
//!
//! Each row here decodes the message from those bytes, writes it with this
//! port, and requires protowire-go's header, row and error texts. It then
//! reads protowire-go's table back with this port's reader, through
//! `DatasetReader::scan_one` and through `parse` + `bind_row`, and requires
//! the message protowire-go's reader read.

use std::collections::HashMap;
use std::io::Cursor;

use prost_reflect::{DescriptorPool, DynamicMessage, MessageDescriptor, ReflectMessage, Value};
use protowire_pxf::{
    append_dataset_header, append_dataset_row, bind_row, marshal, parse, DatasetReader,
    MarshalOptions, UnmarshalOptions,
};

const ORACLE: &str = include_str!("../testdata/conformance/dataset-write-oracle.tsv");

fn pools() -> HashMap<&'static str, DescriptorPool> {
    let sets: [(&str, &[u8]); 5] = [
        (
            "dataset-write-test.binpb",
            include_bytes!("../testdata/dataset-write-test.binpb"),
        ),
        (
            "dataset-write-required-list-test.binpb",
            include_bytes!("../testdata/dataset-write-required-list-test.binpb"),
        ),
        (
            "dataset-write-null-test.binpb",
            include_bytes!("../testdata/dataset-write-null-test.binpb"),
        ),
        (
            "scan-options-bad-test.binpb",
            include_bytes!("../testdata/scan-options-bad-test.binpb"),
        ),
        (
            "oneof-two-defaults-test.binpb",
            include_bytes!("../testdata/oneof-two-defaults-test.binpb"),
        ),
    ];
    sets.into_iter()
        .map(|(name, fds)| (name, DescriptorPool::decode(fds).expect(name)))
        .collect()
}

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

/// One of the oracle's verdicts: the text, or the error.
fn want(verdict: &str, text: &str) -> Result<String, String> {
    match verdict {
        "OK" => Ok(unescape(text)),
        "ERR" => Err(unescape(text)),
        other => panic!("verdict {other:?}"),
    }
}

/// Messages are equal when every field has the same presence and the same
/// value, floats compared by their bits: a NaN equals the same NaN, and `-0`
/// is not `+0`. A map's order is not compared.
fn same_message(a: &DynamicMessage, b: &DynamicMessage) -> bool {
    a.descriptor() == b.descriptor()
        && a.descriptor().fields().all(|fd| {
            a.has_field(&fd) == b.has_field(&fd) && same_value(&a.get_field(&fd), &b.get_field(&fd))
        })
}

fn same_value(a: &Value, b: &Value) -> bool {
    match (a, b) {
        (Value::F32(x), Value::F32(y)) => x.to_bits() == y.to_bits(),
        (Value::F64(x), Value::F64(y)) => x.to_bits() == y.to_bits(),
        (Value::Message(x), Value::Message(y)) => same_message(x, y),
        (Value::List(x), Value::List(y)) => {
            x.len() == y.len() && x.iter().zip(y).all(|(x, y)| same_value(x, y))
        }
        (Value::Map(x), Value::Map(y)) => {
            x.len() == y.len()
                && x.iter()
                    .all(|(k, v)| y.get(k).is_some_and(|w| same_value(v, w)))
        }
        _ => a == b,
    }
}

/// `m` with the paths of its root `_null` mask sorted.
fn mask_sorted(m: &DynamicMessage) -> DynamicMessage {
    let mut m = m.clone();
    if let Some(Value::Message(mut mask)) = m.get_field_by_name("_null").map(|v| v.into_owned()) {
        if let Some(Value::List(mut paths)) =
            mask.get_field_by_name("paths").map(|v| v.into_owned())
        {
            paths.sort_by(|a, b| a.as_str().cmp(&b.as_str()));
            mask.set_field_by_name("paths", Value::List(paths));
            m.set_field_by_name("_null", Value::Message(mask));
        }
    }
    m
}

/// The one row of `table`, read through `scan_one` and through `parse` +
/// `bind_row`, which must agree.
fn read_back(md: &MessageDescriptor, table: &str) -> Result<DynamicMessage, String> {
    let mut tr = DatasetReader::new(Cursor::new(table.as_bytes().to_vec())).expect("reader");
    assert_eq!(tr.type_name(), md.full_name(), "{table}");
    let scanned = tr
        .scan_one(md, UnmarshalOptions::default())
        .map(|m| m.expect("a row"));
    assert!(
        matches!(tr.scan_one(md, UnmarshalOptions::default()), Ok(None)),
        "one row: {table}"
    );
    let doc = parse(table).expect("parses");
    let ds = &doc.datasets[0];
    let bound = bind_row(md, &ds.columns, &ds.rows[0], UnmarshalOptions::default());
    match (scanned, bound) {
        (Ok(s), Ok(b)) => {
            assert!(same_message(&s, &b), "scan_one vs bind_row: {table}");
            Ok(s)
        }
        (Err(s), Err(b)) => {
            assert_eq!(
                s.to_string(),
                b.to_string(),
                "scan_one vs bind_row: {table}"
            );
            Err(s.to_string())
        }
        (s, b) => panic!("scan_one {s:?} vs bind_row {b:?}: {table}"),
    }
}

/// One oracle row, parsed.
struct Case<'a> {
    set: &'a str,
    message: &'a str,
    columns: Vec<&'a str>,
    build: &'a str,
    /// The message, `None` for a header case.
    msg_hex: Option<&'a str>,
    header: Result<String, String>,
    row: Option<Result<String, String>>,
    back: Option<Result<&'a str, String>>,
    marshal: Option<Result<String, String>>,
}

fn cases() -> Vec<Case<'static>> {
    ORACLE
        .lines()
        .filter(|l| !l.starts_with('#'))
        .map(|line| {
            let c: Vec<&str> = line.split('\t').collect();
            assert_eq!(c.len(), 14, "{line}");
            let given = |i: usize| (c[i] != "-").then(|| want(c[i], c[i + 1]));
            Case {
                set: c[0],
                message: c[1],
                columns: if c[2] == "-" {
                    Vec::new()
                } else {
                    c[2].split(',').collect()
                },
                build: c[3],
                msg_hex: (c[5] != "-").then_some(c[5]),
                header: want(c[6], c[7]),
                row: given(8),
                back: (c[10] != "-").then(|| match c[10] {
                    "OK" => Ok(c[11]),
                    _ => Err(unescape(c[11])),
                }),
                marshal: given(12),
            }
        })
        .collect()
}

#[test]
fn every_case_meets_the_reference() {
    let pools = pools();
    let (mut rows, mut refused, mut read) = (0, 0, 0);
    // Rows read back with the `_null` paths in another order: #174.
    let mut mask_order = Vec::new();
    for case in cases() {
        let pool = &pools[case.set];
        let md = pool.get_message_by_name(case.message).expect(case.message);
        let what = format!(
            "{} ({}) {}",
            case.message,
            case.columns.join(","),
            case.build
        );

        let mut header = String::new();
        let got = append_dataset_header(&mut header, &md, &case.columns).map(|()| header.clone());
        assert_eq!(
            got.map_err(|e| e.to_string()),
            case.header,
            "header: {what}"
        );

        let Some(hex) = case.msg_hex else {
            continue;
        };
        let msg = DynamicMessage::decode(md.clone(), unhex(hex).as_slice()).expect("oracle bytes");
        rows += 1;

        let mut row = String::new();
        let got = append_dataset_row(&mut row, &msg, &case.columns).map(|()| row.clone());
        let want_row = case.row.expect("a row verdict");
        assert_eq!(
            got.clone().map_err(|e| e.to_string()),
            want_row,
            "row: {what}\n{msg}"
        );
        refused += usize::from(want_row.is_err());

        if let Some(marshalled) = case.marshal {
            let got = marshal(&msg, &md, MarshalOptions::default());
            assert_eq!(
                got.map_err(|e| e.to_string()),
                marshalled,
                "marshal: {what}"
            );
        }

        let Some(back) = case.back else {
            continue;
        };
        let table = format!(
            "{}{}",
            case.header.as_ref().unwrap(),
            want_row.as_ref().unwrap()
        );
        let got = read_back(&md, &table);
        match back {
            Ok(hex) => {
                let expected = if hex == "=" {
                    msg.clone()
                } else {
                    DynamicMessage::decode(md.clone(), unhex(hex).as_slice()).expect("read back")
                };
                let got = got.unwrap_or_else(|e| panic!("read back: {what}: {e}\n{table}"));
                if !same_message(&got, &expected)
                    && same_message(&mask_sorted(&got), &mask_sorted(&expected))
                {
                    mask_order.push(what.clone());
                    continue;
                }
                assert!(
                    same_message(&got, &expected),
                    "read back: {what}\n{table}\n got: {got}\nwant: {expected}"
                );
            }
            Err(e) => assert_eq!(got.map(|m| m.to_string()), Err(e), "read back: {what}"),
        }
        read += 1;
    }
    // A positive marker: the rows ran, refused ones and read-back ones
    // among them.
    assert_eq!((rows, refused, read), (304, 15, 288));
    // Pinned divergence, #174: this port's reader sorts the `_null` paths,
    // where protowire-go keeps document order. The writer's row is
    // protowire-go's; only the mask the reader builds from it differs. This
    // fails once the reader keeps document order: drop the allowance then.
    assert_eq!(
        mask_order,
        ["dswnull.v1.N (w,dw,rw,n,o) full w = null dw = null rw = 4 o = null ;; unmarshal w = \"set anyway\" n = 3 ;; mask n ;; mask rw"]
    );
}

/// The random rows of protowire-go's round trip, as protowire-go wrote them.
fn round_trip_rows() -> (
    MessageDescriptor,
    Vec<&'static str>,
    Vec<DynamicMessage>,
    Vec<Case<'static>>,
) {
    let pools = pools();
    let md = pools["dataset-write-test.binpb"]
        .get_message_by_name("dsw.v1.Row")
        .unwrap();
    let cases: Vec<_> = cases()
        .into_iter()
        .filter(|c| c.build == "random 142 100")
        .collect();
    assert_eq!(cases.len(), 100);
    let msgs = cases
        .iter()
        .map(|c| DynamicMessage::decode(md.clone(), unhex(c.msg_hex.unwrap()).as_slice()).unwrap())
        .collect();
    (md, cases[0].columns.clone(), msgs, cases)
}

/// Appending rows to a file that already holds a header and rows gives the
/// same bytes as writing them all at once: nothing earlier changes. The
/// whole table reads back row by row as protowire-go's reader read each
/// (protowire-go's `TestDatasetWrite_AppendNeedsNoRewrite`).
#[test]
fn append_needs_no_rewrite() {
    let (md, cols, msgs, cases) = round_trip_rows();
    let mut whole = String::new();
    append_dataset_header(&mut whole, &md, &cols).unwrap();
    for m in &msgs {
        append_dataset_row(&mut whole, m, &cols).unwrap();
    }

    let mut file = String::new();
    append_dataset_header(&mut file, &md, &cols).unwrap();
    for m in &msgs[..20] {
        append_dataset_row(&mut file, m, &cols).unwrap();
    }
    let before = file.clone();
    for m in &msgs[20..] {
        append_dataset_row(&mut file, m, &cols).unwrap();
    }
    assert!(file.starts_with(&before));
    assert_eq!(file, whole);
    // One row per line, value cells included.
    assert_eq!(file.lines().count(), 1 + msgs.len());

    let mut tr = DatasetReader::new(Cursor::new(file.into_bytes())).unwrap();
    for (i, case) in cases.iter().enumerate() {
        let got = tr
            .scan_one(&md, UnmarshalOptions::default())
            .unwrap()
            .expect("a row");
        let want = match case.back.as_ref().unwrap() {
            Ok("=") => msgs[i].clone(),
            Ok(hex) => DynamicMessage::decode(md.clone(), unhex(hex).as_slice()).unwrap(),
            Err(e) => panic!("row {i}: {e}"),
        };
        assert!(same_message(&got, &want), "row {i}");
    }
    assert!(matches!(
        tr.scan_one(&md, UnmarshalOptions::default()),
        Ok(None)
    ));
}

/// A refused header or row leaves `dst` as it was (protowire-go's
/// `TestDatasetWrite_RefusesWhatACellCannotHold` and `TestDatasetWrite_Required`).
#[test]
fn a_refusal_leaves_dst_unchanged() {
    let pools = pools();
    let pool = &pools["dataset-write-test.binpb"];
    let row = pool.get_message_by_name("dsw.v1.Row").unwrap();
    let req = pool.get_message_by_name("dsw.v1.Req").unwrap();
    let none: [&str; 0] = [];
    for (md, cols) in [
        (&row, &none[..]),
        (&row, &["inner", "inner.x"][..]),
        (&row, &["s", "s"][..]),
        (&req, &["id", "note"][..]),
    ] {
        let mut dst = String::from("keep");
        assert!(append_dataset_header(&mut dst, md, cols).is_err());
        assert_eq!(dst, "keep");
        assert!(append_dataset_row(&mut dst, &DynamicMessage::new(md.clone()), cols).is_err());
        assert_eq!(dst, "keep");
    }
    // A row refused after its first cells are written: `name` is unset and
    // required, and `id` before it is written.
    let mut dst = String::from("keep");
    let cols = ["id".to_string(), "name".to_string()];
    let err = append_dataset_row(&mut dst, &DynamicMessage::new(req), &cols).unwrap_err();
    assert_eq!(
        err.to_string(),
        r#"pxf: @dataset column "name": unset, and the field is (pxf.required)"#
    );
    assert!(!err.has_position());
    assert_eq!(dst, "keep");
}
