// SPDX-License-Identifier: MIT
// Copyright (c) 2026 TrendVidia, LLC.
//! Slice D4 decoder tests: presence tracking via `Presence`,
//! `(pxf.required)` validation, `(pxf.default)` application, and the
//! `_null` `FieldMask` mirror channel. Mirrors the
//! `pxf.unmarshalFull — *` blocks in the TS port's `pxf/decode.test.ts`.

use prost_reflect::{DescriptorPool, DynamicMessage, MessageDescriptor, ReflectMessage, Value};
use protowire_pxf::{unmarshal_full, UnmarshalOptions};

const D4_FDS: &[u8] = include_bytes!("../testdata/d4-test.binpb");
const TEST_FDS: &[u8] = include_bytes!("../testdata/test.binpb");

fn d4_pool() -> DescriptorPool {
    DescriptorPool::decode(D4_FDS).expect("decode d4-test.binpb")
}

fn test_pool() -> DescriptorPool {
    DescriptorPool::decode(TEST_FDS).expect("decode test.binpb")
}

fn d4_msg(name: &str) -> MessageDescriptor {
    d4_pool()
        .get_message_by_name(name)
        .unwrap_or_else(|| panic!("missing {name}"))
}

fn full(input: &str, desc: &MessageDescriptor) -> (DynamicMessage, protowire_pxf::Presence) {
    unmarshal_full(input, desc, UnmarshalOptions::default()).expect("unmarshal_full")
}

fn field_value(msg: &DynamicMessage, name: &str) -> Value {
    let fd = msg.descriptor().get_field_by_name(name).unwrap();
    msg.get_field(&fd).into_owned()
}

fn message_field<'a>(msg: &'a DynamicMessage, name: &str) -> &'a DynamicMessage {
    let fd = msg.descriptor().get_field_by_name(name).unwrap();
    match msg.get_field(&fd) {
        std::borrow::Cow::Borrowed(Value::Message(m)) => m,
        _ => panic!("{name} is not a borrowed message"),
    }
}

// ---------------- Presence tracking ----------------

#[test]
fn presence_marks_set_null_and_absent() {
    let all_types = test_pool()
        .get_message_by_name("test.v1.AllTypes")
        .unwrap();
    let (_, p) = unmarshal_full(
        "string_field = \"hi\"\nnullable_int = null",
        &all_types,
        UnmarshalOptions::default(),
    )
    .expect("ok");
    assert!(p.is_set("string_field"));
    assert!(!p.is_null("string_field"));
    assert!(p.is_null("nullable_int"));
    assert!(p.is_absent("int32_field"));
}

#[test]
fn presence_tracks_dotted_paths_into_nested_messages() {
    let all_types = test_pool()
        .get_message_by_name("test.v1.AllTypes")
        .unwrap();
    let (_, p) = unmarshal_full(
        r#"nested_field { name = "alice" }"#,
        &all_types,
        UnmarshalOptions::default(),
    )
    .expect("ok");
    assert!(p.is_set("nested_field"));
    assert!(p.is_set("nested_field.name"));
    assert!(p.is_absent("nested_field.value"));
}

// ---------------- pxf.required ----------------

#[test]
fn required_errors_when_field_is_absent() {
    let with_required = d4_msg("d4_test.v1.WithRequired");
    let err = unmarshal_full("value = 1", &with_required, UnmarshalOptions::default())
        .expect_err("required should error");
    assert!(
        err.msg.contains("required field \"name\" is absent"),
        "msg: {}",
        err.msg
    );
}

#[test]
fn required_passes_when_field_is_set() {
    let with_required = d4_msg("d4_test.v1.WithRequired");
    let (m, _) = full("name = \"ok\"", &with_required);
    assert!(matches!(field_value(&m, "name"), Value::String(s) if s == "ok"));
}

#[test]
fn required_treats_null_as_present() {
    let with_required = d4_msg("d4_test.v1.WithRequired");
    let (_, p) = full("name = null", &with_required);
    assert!(p.is_null("name"));
    assert!(!p.is_absent("name"));
}

// ---------------- pxf.default ----------------

#[test]
fn default_applies_string_when_absent() {
    let with_default = d4_msg("d4_test.v1.WithDefault");
    let (m, _) = full("count = 9", &with_default);
    assert!(matches!(field_value(&m, "name"), Value::String(s) if s == "anonymous"));
    assert!(matches!(field_value(&m, "count"), Value::I32(9)));
}

#[test]
fn default_applies_int_when_absent() {
    let with_default = d4_msg("d4_test.v1.WithDefault");
    let (m, _) = full("name = \"x\"", &with_default);
    assert!(matches!(field_value(&m, "count"), Value::I32(5)));
}

#[test]
fn default_applies_bool_when_absent() {
    let with_default = d4_msg("d4_test.v1.WithDefault");
    let (m, _) = full("", &with_default);
    assert!(matches!(field_value(&m, "active"), Value::Bool(true)));
}

#[test]
fn default_does_not_apply_when_field_is_null() {
    let with_default = d4_msg("d4_test.v1.WithDefault");
    let (m, p) = full("name = null", &with_default);
    assert!(p.is_null("name"));
    assert!(matches!(field_value(&m, "name"), Value::String(s) if s.is_empty()));
}

#[test]
fn default_does_not_apply_when_set_explicitly() {
    let with_default = d4_msg("d4_test.v1.WithDefault");
    let (m, _) = full("name = \"explicit\"", &with_default);
    assert!(matches!(field_value(&m, "name"), Value::String(s) if s == "explicit"));
}

#[test]
fn default_recurses_into_nested_messages() {
    let outer = d4_msg("d4_test.v1.Outer");
    let (m, _) = full("inner { num = 7 }", &outer);
    let inner = message_field(&m, "inner");
    assert!(matches!(field_value(inner, "label"), Value::String(s) if s == "fallback"));
    assert!(matches!(field_value(inner, "num"), Value::I32(7)));
}

// ---------------- _null FieldMask ----------------

fn null_mask_paths(msg: &DynamicMessage) -> Vec<String> {
    let fd = msg.descriptor().get_field_by_name("_null").unwrap();
    let mask = match msg.get_field(&fd) {
        std::borrow::Cow::Borrowed(Value::Message(m)) => m.clone(),
        std::borrow::Cow::Owned(Value::Message(m)) => m,
        _ => return Vec::new(),
    };
    let paths_fd = mask.descriptor().get_field_by_name("paths").unwrap();
    match mask.get_field(&paths_fd).into_owned() {
        Value::List(items) => items
            .into_iter()
            .filter_map(|v| match v {
                Value::String(s) => Some(s),
                _ => None,
            })
            .collect(),
        _ => Vec::new(),
    }
}

#[test]
fn null_mask_appends_paths_of_null_set_fields() {
    let with_null_mask = d4_msg("d4_test.v1.WithNullMask");
    let (m, p) = full("name = \"alice\"\nvalue = null", &with_null_mask);
    assert!(p.is_null("value"));
    assert_eq!(null_mask_paths(&m), vec!["value".to_string()]);
}

#[test]
fn null_mask_untouched_when_no_field_is_null() {
    let with_null_mask = d4_msg("d4_test.v1.WithNullMask");
    let (m, _) = full("name = \"ok\"\nvalue = 1", &with_null_mask);
    assert_eq!(null_mask_paths(&m), Vec::<String>::new());
}
