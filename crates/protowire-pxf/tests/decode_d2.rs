// SPDX-License-Identifier: MIT
// Copyright (c) 2026 TrendVidia, LLC.
//! Slice D2 decoder tests: maps (string/int/message values), well-known
//! Timestamp + Duration sugar, and wrapper-type bare-scalar shorthand.
//! Mirrors the corresponding `describe()` blocks in the TS port's
//! `pxf/decode.test.ts`.

use prost_reflect::{DescriptorPool, MapKey, MessageDescriptor, ReflectMessage, Value};
use protowire_pxf::{unmarshal, PxfError, UnmarshalOptions};

const TEST_FDS: &[u8] = include_bytes!("../testdata/test.binpb");

fn pool() -> DescriptorPool {
    DescriptorPool::decode(TEST_FDS).expect("decode test.binpb")
}

fn all_types() -> MessageDescriptor {
    pool()
        .get_message_by_name("test.v1.AllTypes")
        .expect("missing test.v1.AllTypes")
}

fn nested() -> MessageDescriptor {
    pool()
        .get_message_by_name("test.v1.Nested")
        .expect("missing test.v1.Nested")
}

fn decode(input: &str) -> prost_reflect::DynamicMessage {
    unmarshal(input, &all_types(), UnmarshalOptions::default()).expect("decode ok")
}

fn expect_err(input: &str) -> PxfError {
    match unmarshal(input, &all_types(), UnmarshalOptions::default()) {
        Ok(_) => panic!("expected decode error for input: {input}"),
        Err(e) => e,
    }
}

fn map_of(msg: &prost_reflect::DynamicMessage, name: &str) -> std::collections::HashMap<MapKey, Value> {
    let fd = msg
        .descriptor()
        .get_field_by_name(name)
        .unwrap_or_else(|| panic!("field {name} missing"));
    match msg.get_field(&fd).into_owned() {
        Value::Map(m) => m,
        v => panic!("expected map, got {v:?}"),
    }
}

fn message_field<'a>(
    msg: &'a prost_reflect::DynamicMessage,
    name: &str,
) -> &'a prost_reflect::DynamicMessage {
    let fd = msg
        .descriptor()
        .get_field_by_name(name)
        .unwrap_or_else(|| panic!("field {name} missing"));
    match msg.get_field(&fd) {
        std::borrow::Cow::Borrowed(Value::Message(m)) => m,
        _ => panic!("{name} is not a borrowed message"),
    }
}

// ---------------- maps ----------------

#[test]
fn map_string_string() {
    let m = decode(r#"string_map = { foo: "bar" baz: "qux" }"#);
    let map = map_of(&m, "string_map");
    assert_eq!(map.len(), 2);
    let foo = map.get(&MapKey::String("foo".into())).expect("foo");
    let baz = map.get(&MapKey::String("baz".into())).expect("baz");
    assert!(matches!(foo, Value::String(s) if s == "bar"));
    assert!(matches!(baz, Value::String(s) if s == "qux"));
}

#[test]
fn map_int32_string_with_integer_keys() {
    let m = decode(r#"int_map = { 1: "one" 2: "two" }"#);
    let map = map_of(&m, "int_map");
    assert!(matches!(map.get(&MapKey::I32(1)), Some(Value::String(s)) if s == "one"));
    assert!(matches!(map.get(&MapKey::I32(2)), Some(Value::String(s)) if s == "two"));
}

#[test]
fn map_string_message_values() {
    let m = decode(
        r#"nested_map = {
             alpha: { name = "a" value = 1 }
             beta:  { name = "b" value = 2 }
           }"#,
    );
    let map = map_of(&m, "nested_map");
    let n_fd = nested().get_field_by_name("name").unwrap();
    let v_fd = nested().get_field_by_name("value").unwrap();

    let alpha = match map.get(&MapKey::String("alpha".into())).expect("alpha") {
        Value::Message(m) => m,
        v => panic!("expected message, got {v:?}"),
    };
    assert!(matches!(alpha.get_field(&n_fd).into_owned(), Value::String(s) if s == "a"));
    assert!(matches!(alpha.get_field(&v_fd).into_owned(), Value::I32(1)));

    let beta = match map.get(&MapKey::String("beta".into())).expect("beta") {
        Value::Message(m) => m,
        v => panic!("expected message, got {v:?}"),
    };
    assert!(matches!(beta.get_field(&v_fd).into_owned(), Value::I32(2)));
}

#[test]
fn map_string_keys_via_string_token() {
    let m = decode(r#"string_map = { "with space": "value" }"#);
    let map = map_of(&m, "string_map");
    assert!(matches!(
        map.get(&MapKey::String("with space".into())),
        Some(Value::String(s)) if s == "value"
    ));
}

#[test]
fn map_rejects_equals_inside() {
    let err = expect_err(r#"string_map = { foo = "bar" }"#);
    assert!(
        err.msg.contains("use ':' for map entries"),
        "msg: {}",
        err.msg
    );
}

#[test]
fn map_rejects_null_value() {
    let err = expect_err(r#"string_map = { foo: null }"#);
    assert!(
        err.msg.contains("null is not allowed as map value"),
        "msg: {}",
        err.msg
    );
}

#[test]
fn map_rejects_invalid_int32_key() {
    let err = expect_err(r#"int_map = { not_a_number: "x" }"#);
    assert!(
        err.msg.contains("invalid int32 map key"),
        "msg: {}",
        err.msg
    );
}

#[test]
fn map_empty_block() {
    let m = decode(r#"string_map = { }"#);
    let map = map_of(&m, "string_map");
    assert!(map.is_empty());
}

// ---------------- well-known Timestamp / Duration ----------------

fn seconds_nanos(target: &prost_reflect::DynamicMessage) -> (i64, i32) {
    let s_fd = target.descriptor().get_field_by_name("seconds").unwrap();
    let n_fd = target.descriptor().get_field_by_name("nanos").unwrap();
    let s = match target.get_field(&s_fd).into_owned() {
        Value::I64(s) => s,
        v => panic!("seconds not i64: {v:?}"),
    };
    let n = match target.get_field(&n_fd).into_owned() {
        Value::I32(n) => n,
        v => panic!("nanos not i32: {v:?}"),
    };
    (s, n)
}

#[test]
fn timestamp_second_precision() {
    let m = decode("ts_field = 2024-01-01T12:00:00Z");
    let ts = message_field(&m, "ts_field");
    assert_eq!(seconds_nanos(ts), (1_704_110_400, 0));
}

#[test]
fn timestamp_with_fractional_nanoseconds() {
    let m = decode("ts_field = 1970-01-01T00:00:00.123456789Z");
    let ts = message_field(&m, "ts_field");
    assert_eq!(seconds_nanos(ts), (0, 123_456_789));
}

#[test]
fn duration_hours_plus_minutes() {
    let m = decode("dur_field = 1h30m");
    let d = message_field(&m, "dur_field");
    assert_eq!(seconds_nanos(d), (5400, 0));
}

#[test]
fn duration_subsecond_via_ms() {
    let m = decode("dur_field = 1500ms");
    let d = message_field(&m, "dur_field");
    assert_eq!(seconds_nanos(d), (1, 500_000_000));
}

#[test]
fn duration_negative_carries_sign_on_both_fields() {
    let m = decode("dur_field = -2500ms");
    let d = message_field(&m, "dur_field");
    assert_eq!(seconds_nanos(d), (-2, -500_000_000));
}

#[test]
fn timestamp_block_syntax_still_works() {
    let m = decode("ts_field { seconds = 100 nanos = 250 }");
    let ts = message_field(&m, "ts_field");
    assert_eq!(seconds_nanos(ts), (100, 250));
}

// ---------------- wrapper types ----------------

#[test]
fn wrapper_string_takes_bare_string() {
    let m = decode(r#"nullable_string = "hello""#);
    let w = message_field(&m, "nullable_string");
    let v_fd = w.descriptor().get_field_by_name("value").unwrap();
    assert!(matches!(w.get_field(&v_fd).into_owned(), Value::String(s) if s == "hello"));
}

#[test]
fn wrapper_int32_takes_bare_integer() {
    let m = decode("nullable_int = 42");
    let w = message_field(&m, "nullable_int");
    let v_fd = w.descriptor().get_field_by_name("value").unwrap();
    assert!(matches!(w.get_field(&v_fd).into_owned(), Value::I32(42)));
}

#[test]
fn wrapper_bool_takes_bare_bool() {
    let m = decode("nullable_bool = true");
    let w = message_field(&m, "nullable_bool");
    let v_fd = w.descriptor().get_field_by_name("value").unwrap();
    assert!(matches!(w.get_field(&v_fd).into_owned(), Value::Bool(true)));
}

#[test]
fn wrapper_block_syntax_still_works() {
    let m = decode(r#"nullable_string { value = "explicit" }"#);
    let w = message_field(&m, "nullable_string");
    let v_fd = w.descriptor().get_field_by_name("value").unwrap();
    assert!(
        matches!(w.get_field(&v_fd).into_owned(), Value::String(s) if s == "explicit"),
    );
}
