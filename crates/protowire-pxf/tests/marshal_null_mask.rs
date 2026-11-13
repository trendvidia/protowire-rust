// SPDX-License-Identifier: MIT
// Copyright (c) 2026 TrendVidia, LLC.
//! A path in the message's `_null` mask is written `null` only on a field
//! that can hold one. A scalar without presence, a repeated or map field,
//! and a required field are written as if the mask did not name them,
//! since `null` on them is a decode error: protowire-go's #151, held to the
//! Marshal assertion of its `TestDatasetWrite_NullMask`.

use prost_reflect::{DescriptorPool, DynamicMessage, MessageDescriptor, Value};
use protowire_pxf::{marshal, unmarshal_full, MarshalOptions, UnmarshalOptions};

const FDS: &[u8] = include_bytes!("../testdata/dataset-write-null-test.binpb");

fn desc() -> MessageDescriptor {
    DescriptorPool::decode(FDS)
        .expect("fixture")
        .get_message_by_name("dswnull.v1.N")
        .expect("dswnull.v1.N")
}

/// Go's message: `w`, `dw` and `o` null, `rw = 4`, then `w` and `n` set
/// over it, and `n` and `rw` added to the mask.
fn masked() -> DynamicMessage {
    let md = desc();
    let (mut m, _) = unmarshal_full(
        "w = null\ndw = null\nrw = 4\no = null\n",
        &md,
        UnmarshalOptions::default(),
    )
    .expect("decodes");
    let w = md.get_field_by_name("w").unwrap();
    let Some(prost_reflect::Kind::Message(sv)) = Some(w.kind()) else {
        unreachable!()
    };
    let mut set = DynamicMessage::new(sv);
    set.set_field_by_name("value", Value::String("set anyway".into()));
    m.set_field(&w, Value::Message(set));
    m.set_field_by_name("n", Value::I32(3));
    let Value::Message(mut mask) = m.get_field_by_name("_null").unwrap().into_owned() else {
        panic!("the mask is a message")
    };
    let Value::List(mut paths) = mask.get_field_by_name("paths").unwrap().into_owned() else {
        panic!("paths is a list")
    };
    let mut recorded: Vec<&str> = paths.iter().filter_map(Value::as_str).collect();
    recorded.sort();
    assert_eq!(
        recorded,
        ["dw", "o", "w"],
        "the mask UnmarshalFull recorded"
    );
    paths.extend(["n", "rw"].map(|p| Value::String(p.into())));
    mask.set_field_by_name("paths", Value::List(paths));
    m.set_field_by_name("_null", Value::Message(mask));
    m
}

#[test]
fn a_masked_field_that_cannot_hold_null_is_written_as_its_value() {
    let text = marshal(&masked(), &desc(), MarshalOptions::default()).expect("marshals");
    assert_eq!(text, "w = null\ndw = null\nrw = 4\nn = 3\no = null\n");
}

/// Unset, the mask does not make a required field writable: `null` is no
/// text for it.
#[test]
fn a_masked_unset_required_field_is_still_refused() {
    let md = desc();
    let mut m = masked();
    m.clear_field_by_name("rw");
    let err = marshal(&m, &md, MarshalOptions::default()).unwrap_err();
    assert_eq!(
        err.to_string(),
        "pxf: dswnull.v1.N.rw is unset, and the field is (pxf.required)"
    );
}
