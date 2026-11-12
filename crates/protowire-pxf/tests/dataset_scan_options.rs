// SPDX-License-Identifier: MIT
// Copyright (c) 2026 TrendVidia, LLC.
//! protowire-go's `encoding/pxf/dataset_scan_options_test.go`, ported: a
//! `@dataset` row is bound under every decode option, through
//! `DatasetReader::scan_one` and through `parse` + `bind_row`, and the two
//! paths must agree.
//!
//! Go's test was written for a bug (protowire-go#280) where `Scan` dropped
//! most options. Here `scan_one` takes the options on each call, so that
//! bug cannot happen; the cases still hold each option's effect on a row,
//! and the texts, to protowire-go's.

use std::any::Any;
use std::cell::RefCell;
use std::io::Cursor;

use prost_reflect::{DescriptorPool, DynamicMessage, MessageDescriptor, Value};
use protowire_check::{BoxError, Report, Violation};
use protowire_pxf::{bind_row, parse, DatasetReader, PxfError, UnmarshalOptions};

const FDS: &[u8] = include_bytes!("../testdata/scan-options-test.binpb");
const BAD_FDS: &[u8] = include_bytes!("../testdata/scan-options-bad-test.binpb");

fn desc(fds: &[u8], name: &str) -> MessageDescriptor {
    DescriptorPool::decode(fds)
        .expect("decode the fixture")
        .get_message_by_name(name)
        .unwrap_or_else(|| panic!("no message {name}"))
}

fn row() -> MessageDescriptor {
    desc(FDS, "so.v1.Row")
}

/// Bind the one row of `table` under `o`, through `scan_one` and through
/// `parse` + `bind_row`, and require the two to agree. Returns the bound
/// message or the error.
fn bind_under(
    o: UnmarshalOptions<'_>,
    md: &MessageDescriptor,
    table: &str,
) -> Result<DynamicMessage, PxfError> {
    let doc = parse(table).expect("parses");
    let ds = &doc.datasets[0];
    assert_eq!(ds.rows.len(), 1);
    let via_bind = bind_row(md, &ds.columns, &ds.rows[0], o);

    let mut tr = DatasetReader::new(Cursor::new(table.as_bytes().to_vec())).expect("reader");
    let via_scan = tr.scan_one(md, o).map(|m| m.expect("a row"));
    assert_eq!(
        via_bind.as_ref().err().map(ToString::to_string),
        via_scan.as_ref().err().map(ToString::to_string),
        "bind_row vs scan_one"
    );
    let msg = via_bind?;
    assert_eq!(msg, via_scan.unwrap(), "bind_row vs scan_one");
    assert!(matches!(tr.scan_one(md, o), Ok(None)), "after the row");
    Ok(msg)
}

fn string_field(m: &DynamicMessage, name: &str) -> String {
    m.get_field_by_name(name)
        .expect("field")
        .as_str()
        .expect("a string")
        .to_string()
}

#[test]
fn skip_post_decode() {
    let md = row();
    let table = "@dataset so.v1.Row ( id )\n(\"x\")\n";

    let err = bind_under(UnmarshalOptions::default(), &md, table).unwrap_err();
    assert_eq!(err.to_string(), r#"2:1: required field "req" is absent"#);

    let o = UnmarshalOptions {
        skip_post_decode: true,
        ..Default::default()
    };
    let m = bind_under(o, &md, table).expect("binds");
    // No post-decode pass: the required field is not checked, and the
    // default is not applied.
    assert_eq!(string_field(&m, "dflt"), "");
}

#[test]
fn discard_unknown() {
    let md = row();
    let table = "@dataset so.v1.Row ( id, req, inner )\n(\"x\", 1, { n = 2 nope = 3 })\n";

    let err = bind_under(UnmarshalOptions::default(), &md, table).unwrap_err();
    assert_eq!(
        err.to_string(),
        r#"2:1: column inner: unknown field "nope" in so.v1.Inner"#
    );

    let o = UnmarshalOptions {
        discard_unknown: true,
        ..Default::default()
    };
    let m = bind_under(o, &md, table).expect("binds");
    let inner = m.get_field_by_name("inner").expect("inner");
    let n = inner
        .as_message()
        .expect("a message")
        .get_field_by_name("n");
    assert_eq!(n.as_deref(), Some(&Value::I32(2)));
}

fn pw_value(m: &DynamicMessage) -> String {
    let pw = m.get_field_by_name("pw").expect("pw");
    string_field(pw.as_message().expect("a message"), "value")
}

#[test]
fn on_secret_field() {
    let md = row();
    let table = "@dataset so.v1.Row ( id, req, pw )\n(\"x\", 1, \"s3cret\")\n";

    let m = bind_under(UnmarshalOptions::default(), &md, table).expect("binds");
    assert_eq!(pw_value(&m), "s3cret");

    // bind_under binds the row twice, so the hook fires once per path.
    let got = RefCell::new(Vec::new());
    let hook = |path: &str, value: &str| -> Result<(), BoxError> {
        got.borrow_mut().push(format!("{path}={value}"));
        Ok(())
    };
    let o = UnmarshalOptions {
        on_secret_field: Some(&hook),
        ..Default::default()
    };
    let m = bind_under(o, &md, table).expect("binds");
    assert_eq!(*got.borrow(), ["pw=s3cret", "pw=s3cret"]);
    assert_eq!(pw_value(&m), "");
}

fn deny_all(_: &dyn Any) -> Result<Report, BoxError> {
    Ok(Report {
        violations: vec![Violation {
            path: "id".into(),
            rule_id: "test.deny".into(),
            message: "denied".into(),
        }],
    })
}

#[test]
fn validator() {
    let md = row();
    let table = "@dataset so.v1.Row ( id, req )\n(\"x\", 1)\n";

    bind_under(UnmarshalOptions::default(), &md, table).expect("binds");

    // A validator's error is returned as it is, with no row position: only
    // a decode error is moved to the row.
    let o = UnmarshalOptions {
        validator: Some(&deny_all),
        ..Default::default()
    };
    let err = bind_under(o, &md, table).unwrap_err();
    assert_eq!(err.to_string(), "check: id: denied (test.deny)");
    let ce = err.check_error().expect("the check error");
    assert_eq!(ce.report.violations[0].rule_id, "test.deny");
}

#[test]
fn skip_validate() {
    let md = desc(BAD_FDS, "sobad.v1.Bad");
    let table = "@dataset sobad.v1.Bad ( id )\n(\"x\")\n";

    // protowire-go's text, with this fixture's file name for its "sobad.proto".
    let err = bind_under(UnmarshalOptions::default(), &md, table).unwrap_err();
    assert_eq!(
        err.to_string(),
        "PXF schema violations:\n  scan-options-bad-test.proto: message field \"sobad.v1.Bad.null\" uses PXF-reserved name \"null\" (draft §3.13)"
    );

    let o = UnmarshalOptions {
        skip_validate: true,
        ..Default::default()
    };
    let m = bind_under(o, &md, table).expect("binds");
    assert_eq!(string_field(&m, "id"), "x");
}

// A hook's error is a decode error: it carries the row's position and
// names the column, wherever the column stands.
#[test]
fn on_secret_field_error() {
    let md = row();
    let hook = |_: &str, _: &str| -> Result<(), BoxError> { Err("hook says no".into()) };
    let o = UnmarshalOptions {
        on_secret_field: Some(&hook),
        ..Default::default()
    };
    for table in [
        "@dataset so.v1.Row ( id, req, pw )\n(\"x\", 1, \"s3cret\")\n",
        "@dataset so.v1.Row ( pw, id, req )\n(\"s3cret\", \"x\", 1)\n",
    ] {
        let err = bind_under(o, &md, table).unwrap_err();
        assert_eq!(
            err.to_string(),
            r#"2:1: column pw: pxf.Secret hook for "pw": hook says no"#,
            "{table}"
        );
    }
}

// The column paths are checked before the schema: a row type that breaks
// both gets the column's error.
#[test]
fn column_paths_before_schema() {
    let md = desc(BAD_FDS, "sobad.v1.Bad");
    let err = bind_under(
        UnmarshalOptions::default(),
        &md,
        "@dataset sobad.v1.Bad ( nope )\n(\"x\")\n",
    )
    .unwrap_err();
    assert_eq!(
        err.to_string(),
        r#"2:1: @dataset column "nope": sobad.v1.Bad has no field "nope""#
    );
}
