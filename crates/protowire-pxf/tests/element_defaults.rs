// SPDX-License-Identifier: MIT
// Copyright (c) 2026 TrendVidia, LLC.
//! `(pxf.default)` and `(pxf.required)` apply in every message body a document
//! writes (protowire-go#161). `presence_oracle.rs` holds these shapes against
//! the reference; this file covers what it can't: the dataset paths, and that
//! `unmarshal` applies nothing.

use prost_reflect::{DescriptorPool, DynamicMessage, MessageDescriptor, Value};
use protowire_pxf::{
    bind_row, parse, unmarshal, unmarshal_full, DatasetReader, PoolResolver, UnmarshalOptions,
};

const FDS: &[u8] = include_bytes!("../testdata/element-defaults-test.binpb");

fn pool() -> DescriptorPool {
    DescriptorPool::decode(FDS).expect("decode element-defaults-test.binpb")
}

fn desc(name: &str) -> MessageDescriptor {
    pool()
        .get_message_by_name(&format!("elemdefaults.v1.{name}"))
        .unwrap()
}

fn first(m: &DynamicMessage, field: &str) -> DynamicMessage {
    match m.get_field_by_name(field).unwrap().as_ref() {
        Value::List(l) => match &l[0] {
            Value::Message(e) => e.clone(),
            other => panic!("{other:?}"),
        },
        other => panic!("{other:?}"),
    }
}

fn label(m: &DynamicMessage) -> String {
    m.get_field_by_name("label")
        .unwrap()
        .as_str()
        .unwrap()
        .to_string()
}

#[test]
fn unmarshal_applies_nothing() {
    let m = unmarshal(
        "items = [{ deep {} }]",
        &desc("P"),
        UnmarshalOptions::default(),
    )
    .expect("no pass, so no required check");
    assert_eq!(label(&first(&m, "items")), "");
}

#[test]
fn a_rerun_of_a_map_error_names_the_same_key() {
    for _ in 0..32 {
        let err = unmarshal_full(
            r#"by_key = { "b": { deep {} } "a": { deep {} } "c": { deep {} } }"#,
            &desc("P"),
            UnmarshalOptions::default(),
        )
        .expect_err("required")
        .to_string();
        assert!(err.contains(r#"by_key[\"a\"].deep.n"#), "{err}");
    }
}

#[test]
fn a_dataset_cell_is_an_element_like_any_other() {
    let d = desc("R");
    let doc = parse("@dataset elemdefaults.v1.R (items)\n([{ pick_i = 1 }])\n").expect("parses");
    let ds = &doc.datasets[0];
    let m = bind_row(&d, &ds.columns, &ds.rows[0], UnmarshalOptions::default()).expect("binds");
    assert_eq!(label(&first(&m, "items")), "item-dflt");

    let doc = parse("@dataset elemdefaults.v1.R (items)\n([{ deep {} }])\n").expect("parses");
    let ds = &doc.datasets[0];
    let err =
        bind_row(&d, &ds.columns, &ds.rows[0], UnmarshalOptions::default()).expect_err("required");
    assert!(err.msg.contains("items[0].deep.n"), "{err}");
}

#[test]
fn the_dataset_reader_reaches_any_cells() {
    let d = desc("S");
    let pool = pool();
    let resolver = PoolResolver(&pool);
    let opts = UnmarshalOptions {
        type_resolver: Some(&resolver),
        ..Default::default()
    };
    let table = "@dataset elemdefaults.v1.S (anys)\n([{ @type = \"elemdefaults.v1.Leaf\" }])\n";
    let mut reader = DatasetReader::new(table.as_bytes()).expect("reader");
    let m = reader.scan_one(&d, opts).expect("one row").expect("binds");
    let any = first(&m, "anys");
    let bytes = match any.get_field_by_name("value").unwrap().into_owned() {
        Value::Bytes(b) => b,
        other => panic!("{other:?}"),
    };
    let leaf = DynamicMessage::decode(desc("Leaf"), bytes).expect("packed");
    assert_eq!(
        leaf.get_field_by_name("tag").unwrap().as_str(),
        Some("leaf-dflt")
    );
}
