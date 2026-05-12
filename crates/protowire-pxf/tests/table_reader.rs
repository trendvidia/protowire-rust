// SPDX-License-Identifier: MIT
// Copyright (c) 2026 TrendVidia, LLC.
//! Tests for `TableReader` (streaming @table consumption) and
//! `bind_row` (per-row proto binding). PR 4 of the v0.72-v0.75 Rust
//! catch-up.

use std::io::Cursor;

use prost_reflect::{DescriptorPool, MessageDescriptor, ReflectMessage, Value};
use protowire_pxf::ast::Value as AstValue;
use protowire_pxf::{bind_row, PxfError, TableReader, UnmarshalOptions};

const TEST_FDS: &[u8] = include_bytes!("../testdata/test.binpb");

fn pool() -> DescriptorPool {
    DescriptorPool::decode(TEST_FDS).expect("decode test.binpb")
}

fn all_types() -> MessageDescriptor {
    pool()
        .get_message_by_name("test.v1.AllTypes")
        .expect("missing test.v1.AllTypes")
}

fn reader(src: &str) -> Result<TableReader<Cursor<Vec<u8>>>, PxfError> {
    TableReader::new(Cursor::new(src.as_bytes().to_vec()))
}

// ---- header parsing -----------------------------------------------------

#[test]
fn header_exposes_type_and_columns() {
    let tr = reader("@table trades.v1.Trade ( px, qty )\n( 100, 5 )\n( 101, 7 )\n").unwrap();
    assert_eq!(tr.type_name(), "trades.v1.Trade");
    assert_eq!(tr.columns(), &["px", "qty"]);
    assert!(tr.directives().is_empty());
}

#[test]
fn no_table_returns_error() {
    let err = reader("@type foo.Msg\nname = \"x\"\n").err().expect("err");
    assert!(err.to_string().contains("no @table"));
}

#[test]
fn empty_input_returns_error() {
    let err = reader("").err().expect("err");
    assert!(err.to_string().contains("no @table"));
}

#[test]
fn leading_directives_preserved() {
    let src = "@header pkg.Hdr { id = \"h\" }\n\
               @frob alpha\n\
               @table trades.v1.Trade ( px, qty )\n\
               ( 1, 2 )\n";
    let tr = reader(src).unwrap();
    assert_eq!(tr.directives().len(), 2);
    assert_eq!(tr.directives()[0].name, "header");
    assert_eq!(tr.directives()[1].name, "frob");
}

#[test]
fn header_oversize_rejected() {
    let mut big = String::from("@frob ");
    while big.len() < 70 * 1024 {
        big.push_str("x ");
    }
    big.push_str("\n@table x.Row ( a )\n");
    let err = reader(&big).err().expect("err");
    assert!(err.to_string().contains("header exceeds"));
}

// ---- iteration -----------------------------------------------------------

#[test]
fn iterator_yields_rows_in_order() {
    let mut tr = reader("@table x.Row ( a, b )\n( 1, 2 )\n( 3, 4 )\n( 5, 6 )\n").unwrap();
    let mut count = 0;
    for row in &mut tr {
        let row = row.unwrap();
        assert_eq!(row.cells.len(), 2);
        count += 1;
    }
    assert_eq!(count, 3);
    assert!(tr.done());
}

#[test]
fn zero_rows_reports_done_immediately() {
    let mut tr = reader("@table x.Row ( a )\n").unwrap();
    assert!(tr.next_row().is_none());
    assert!(tr.done());
}

#[test]
fn cell_shapes_match_three_state_grammar() {
    let mut tr = reader("@table x.Row ( a, b, c, d, e )\n( 42, \"hi\", true, null, )\n").unwrap();
    let row = tr.next_row().unwrap().unwrap();
    assert!(matches!(row.cells[0], Some(AstValue::Int(_))));
    assert!(matches!(&row.cells[1], Some(AstValue::String(s)) if s.value == "hi"));
    assert!(matches!(row.cells[2], Some(AstValue::Bool(_))));
    assert!(matches!(row.cells[3], Some(AstValue::Null(_))));
    assert!(row.cells[4].is_none()); // absent (empty cell at end)
}

#[test]
fn arity_mismatch_surfaces_as_error() {
    let mut tr = reader("@table x.Row ( a, b )\n( 1, 2, 3 )\n( 4, 5 )\n").unwrap();
    let err = tr.next_row().unwrap().unwrap_err();
    assert!(err.to_string().contains("3 cells, expected 2"));
}

#[test]
fn parens_inside_strings_not_row_boundary() {
    let mut tr = reader("@table x.Row ( a )\n( \"hi ) there\" )\n( \"next\" )\n").unwrap();
    let r1 = tr.next_row().unwrap().unwrap();
    match &r1.cells[0] {
        Some(AstValue::String(s)) => assert_eq!(s.value, "hi ) there"),
        other => panic!("expected String, got {:?}", other),
    }
    let r2 = tr.next_row().unwrap().unwrap();
    match &r2.cells[0] {
        Some(AstValue::String(s)) => assert_eq!(s.value, "next"),
        other => panic!("expected String, got {:?}", other),
    }
    assert!(tr.next_row().is_none());
}

#[test]
fn comments_between_rows_ignored() {
    let src = "@table x.Row ( a )\n\
               # leading\n\
               ( 1 )\n\
               // mid\n\
               ( 2 )\n\
               /* block\n\
                  comment */\n\
               ( 3 )\n";
    let tr = reader(src).unwrap();
    assert_eq!(tr.count(), 3);
}

#[test]
fn streaming_pull_across_chunks() {
    // Reader source that yields one byte at a time, forcing many
    // pull() round-trips inside read_header and next_row.
    struct OneByteReader(std::io::Cursor<Vec<u8>>);
    impl std::io::Read for OneByteReader {
        fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
            if buf.is_empty() {
                return Ok(0);
            }
            let mut one = [0u8; 1];
            let n = self.0.read(&mut one)?;
            if n == 0 {
                return Ok(0);
            }
            buf[0] = one[0];
            Ok(1)
        }
    }
    let src = "@table x.Row ( a )\n( 1 )\n( 2 )\n( 3 )\n";
    let tr = TableReader::new(OneByteReader(Cursor::new(src.as_bytes().to_vec()))).unwrap();
    assert_eq!(tr.count(), 3);
}

// ---- tail() chaining -----------------------------------------------------

#[test]
fn tail_chains_to_second_table() {
    let src = "@table a.Row ( x )\n\
               ( 1 )\n\
               ( 2 )\n\
               @table b.Row ( y )\n\
               ( \"p\" )\n\
               ( \"q\" )\n";
    let mut tr1 = reader(src).unwrap();
    assert_eq!(tr1.type_name(), "a.Row");
    // Drain.
    while let Some(r) = tr1.next_row() {
        r.unwrap();
    }
    let tail = tr1.tail();
    let tr2 = TableReader::new(tail).unwrap();
    assert_eq!(tr2.type_name(), "b.Row");
    assert_eq!(tr2.count(), 2);
}

// ---- bind_row + scan -----------------------------------------------------

#[test]
fn bind_row_sets_fields_by_column() {
    let mut tr =
        reader("@table test.v1.AllTypes ( string_field, int32_field )\n( \"alpha\", 42 )\n")
            .unwrap();
    let columns = tr.columns().to_vec();
    let row = tr.next_row().unwrap().unwrap();
    let opts = UnmarshalOptions {
        skip_validate: true,
        ..Default::default()
    };
    let msg = bind_row(&all_types(), &columns, &row, opts).unwrap();
    let sf = msg.descriptor().get_field_by_name("string_field").unwrap();
    match msg.get_field(&sf).into_owned() {
        Value::String(s) => assert_eq!(s, "alpha"),
        v => panic!("expected String, got {:?}", v),
    }
    let i32f = msg.descriptor().get_field_by_name("int32_field").unwrap();
    match msg.get_field(&i32f).into_owned() {
        Value::I32(v) => assert_eq!(v, 42),
        v => panic!("expected I32, got {:?}", v),
    }
}

#[test]
fn scan_one_is_equivalent_to_next_plus_bind() {
    let mut tr =
        reader("@table test.v1.AllTypes ( string_field )\n( \"row1\" )\n( \"row2\" )\n").unwrap();
    let opts = UnmarshalOptions {
        skip_validate: true,
        ..Default::default()
    };
    let mut seen = Vec::new();
    while let Some(msg) = tr.scan_one(&all_types(), opts).unwrap() {
        let sf = msg.descriptor().get_field_by_name("string_field").unwrap();
        if let Value::String(s) = msg.get_field(&sf).into_owned() {
            seen.push(s);
        }
    }
    assert_eq!(seen, vec!["row1".to_string(), "row2".to_string()]);
}

#[test]
fn bind_row_absent_cell_leaves_field_default() {
    let mut tr =
        reader("@table test.v1.AllTypes ( string_field, int32_field )\n( , 7 )\n").unwrap();
    let columns = tr.columns().to_vec();
    let row = tr.next_row().unwrap().unwrap();
    let opts = UnmarshalOptions {
        skip_validate: true,
        ..Default::default()
    };
    let msg = bind_row(&all_types(), &columns, &row, opts).unwrap();
    let sf = msg.descriptor().get_field_by_name("string_field").unwrap();
    match msg.get_field(&sf).into_owned() {
        Value::String(s) => assert_eq!(s, ""),
        v => panic!("expected default String, got {:?}", v),
    }
    let i32f = msg.descriptor().get_field_by_name("int32_field").unwrap();
    match msg.get_field(&i32f).into_owned() {
        Value::I32(v) => assert_eq!(v, 7),
        v => panic!("got {:?}", v),
    }
}

#[test]
fn bind_row_mismatched_columns_errors() {
    let mut tr = reader("@table test.v1.AllTypes ( string_field )\n( \"x\" )\n").unwrap();
    let row = tr.next_row().unwrap().unwrap();
    let bad_columns = vec!["string_field".to_string(), "extra".to_string()];
    let opts = UnmarshalOptions {
        skip_validate: true,
        ..Default::default()
    };
    let err = bind_row(&all_types(), &bad_columns, &row, opts).unwrap_err();
    assert!(err.to_string().contains("2 columns vs 1 cells"));
}

#[test]
fn bind_row_unknown_column_errors() {
    let mut tr = reader("@table test.v1.AllTypes ( not_a_field )\n( \"x\" )\n").unwrap();
    let opts = UnmarshalOptions {
        skip_validate: true,
        ..Default::default()
    };
    let err = tr.scan_one(&all_types(), opts).unwrap_err();
    // The synthetic body's "not_a_field = ..." goes through unmarshal,
    // which rejects unknown fields by default.
    assert!(err.to_string().to_lowercase().contains("unknown"));
}

#[test]
fn bind_row_string_escape_round_trip() {
    let mut tr =
        reader("@table test.v1.AllTypes ( string_field )\n( \"she said \\\"hi\\\"\" )\n").unwrap();
    let columns = tr.columns().to_vec();
    let row = tr.next_row().unwrap().unwrap();
    let opts = UnmarshalOptions {
        skip_validate: true,
        ..Default::default()
    };
    let msg = bind_row(&all_types(), &columns, &row, opts).unwrap();
    let sf = msg.descriptor().get_field_by_name("string_field").unwrap();
    if let Value::String(s) = msg.get_field(&sf).into_owned() {
        assert_eq!(s, "she said \"hi\"");
    } else {
        panic!("expected String");
    }
}

#[test]
fn bind_row_bytes_cell_round_trip() {
    let mut tr = reader("@table test.v1.AllTypes ( bytes_field )\n( b\"YWJj\" )\n").unwrap(); // "abc"
    let columns = tr.columns().to_vec();
    let row = tr.next_row().unwrap().unwrap();
    let opts = UnmarshalOptions {
        skip_validate: true,
        ..Default::default()
    };
    let msg = bind_row(&all_types(), &columns, &row, opts).unwrap();
    let bf = msg.descriptor().get_field_by_name("bytes_field").unwrap();
    if let Value::Bytes(b) = msg.get_field(&bf).into_owned() {
        assert_eq!(b.as_ref(), b"abc");
    } else {
        panic!("expected Bytes");
    }
}
