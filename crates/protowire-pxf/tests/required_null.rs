// SPDX-License-Identifier: MIT
// Copyright (c) 2026 TrendVidia, LLC.
//! A required field holds a value, so `null` on it is refused as absence
//! is, whatever the field's type and presence (#106, protowire-go#268,
//! trendvidia/protowire#569). protowire-go's `TestRequiredRejectsNull`,
//! ported: a body, a nested body, a `@dataset` cell and a dotted cell, for
//! each spelling of required (`(pxf.required)`, `@required`) and each shape
//! (an `optional` scalar, a wrapper, a message, a plain scalar).
//!
//! The schema is `testdata/carrier/required_null.proto`, built by the
//! reference compiler (see that directory's README). It runs on both of its
//! descriptor sets: one where `@required` is a known extension, and one
//! where it reaches the reader as an unknown field.

use prost_reflect::{DescriptorPool, MessageDescriptor};
use protowire_pxf::{bind_row, parse, unmarshal, unmarshal_full, UnmarshalOptions};

const SETS: [(&str, &[u8]); 2] = [
    (
        "known",
        include_bytes!("../testdata/carrier/required_null.binpb"),
    ),
    (
        "bare",
        include_bytes!("../testdata/carrier/required_null.bare.binpb"),
    ),
];

const FIELDS: [&str; 5] = ["r", "a", "w", "s", "plain"];

/// The literal each field gets, which serves as a body value and a cell.
fn value(f: &str) -> &'static str {
    if f == "s" {
        "{}"
    } else {
        "0"
    }
}

fn body(null: &str) -> String {
    FIELDS
        .iter()
        .map(|f| format!("{f} = {}\n", if *f == null { "null" } else { value(f) }))
        .collect()
}

fn row(null: &str) -> String {
    let cells: Vec<&str> = FIELDS
        .iter()
        .map(|f| if *f == null { "null" } else { value(f) })
        .collect();
    format!("({})\n", cells.join(", "))
}

/// Binds every row of a `@dataset` document, as protowire-go's `bindRows`.
fn bind_rows(desc: &MessageDescriptor, doc: &str) -> Result<(), String> {
    let doc = parse(doc).map_err(|e| e.to_string())?;
    for ds in &doc.datasets {
        for r in &ds.rows {
            bind_row(desc, &ds.columns, r, UnmarshalOptions::default())
                .map_err(|e| e.to_string())?;
        }
    }
    Ok(())
}

/// Every path a field can be bound on, each with its result.
fn paths(
    req: &MessageDescriptor,
    outer: &MessageDescriptor,
    null: &str,
) -> Vec<(&'static str, Result<(), String>)> {
    let opts = UnmarshalOptions::default();
    let dotted: Vec<String> = FIELDS.iter().map(|f| format!("in.{f}")).collect();
    vec![
        (
            "unmarshal",
            unmarshal(&body(null), req, opts)
                .map(|_| ())
                .map_err(|e| e.to_string()),
        ),
        (
            "full",
            unmarshal_full(&body(null), req, opts)
                .map(|_| ())
                .map_err(|e| e.to_string()),
        ),
        (
            "nested",
            unmarshal_full(&format!("in {{\n{}}}\n", body(null)), outer, opts)
                .map(|_| ())
                .map_err(|e| e.to_string()),
        ),
        (
            "cell",
            bind_rows(
                req,
                &format!(
                    "@dataset requirednull.v1.Req ({})\n{}",
                    FIELDS.join(", "),
                    row(null)
                ),
            ),
        ),
        (
            "dotted",
            bind_rows(
                outer,
                &format!(
                    "@dataset requirednull.v1.Outer ({})\n{}",
                    dotted.join(", "),
                    row(null)
                ),
            ),
        ),
    ]
}

#[test]
fn a_required_field_rejects_null() {
    for (set, bytes) in SETS {
        let pool = DescriptorPool::decode(bytes).expect(set);
        let req = pool.get_message_by_name("requirednull.v1.Req").unwrap();
        let outer = pool.get_message_by_name("requirednull.v1.Outer").unwrap();

        // Zero values satisfy every one of them.
        for (path, got) in paths(&req, &outer, "") {
            assert_eq!(got, Ok(()), "{set}/{path}: zero values");
        }

        for f in FIELDS {
            let want = format!("null is not allowed on required field \"{f}\"");
            for (path, got) in paths(&req, &outer, f) {
                let e = got.expect_err(&format!("{set}/{path}: {f} = null is accepted"));
                let want = if path == "cell" {
                    format!("column {f}: {want}")
                } else {
                    want.clone()
                };
                assert!(e.contains(&want), "{set}/{path}: {f}: {e}");
            }
        }
    }
}
