// SPDX-License-Identifier: MIT
// Copyright (c) 2026 TrendVidia, LLC.
//! Slice D1 decoder tests: scalars, enums, nested messages, repeated lists,
//! oneof conflict detection, unknown-field handling, and the empty / leading
//! `@type` shapes. Mirrors the corresponding `describe()` blocks in the TS
//! port's `pxf/decode.test.ts`.
//!
//! Maps and well-known types defined on `test.v1.AllTypes` are part of the
//! shared `test.proto` schema but exercised in later D-slices.

use bytes::Bytes;
use prost_reflect::{DescriptorPool, MessageDescriptor, ReflectMessage, Value};
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

fn decode_with(input: &str, opts: UnmarshalOptions) -> prost_reflect::DynamicMessage {
    unmarshal(input, &all_types(), opts).expect("decode ok")
}

fn expect_err(input: &str) -> PxfError {
    match unmarshal(input, &all_types(), UnmarshalOptions::default()) {
        Ok(_) => panic!("expected decode error for input: {input}"),
        Err(e) => e,
    }
}

fn field_value<'a>(
    msg: &'a prost_reflect::DynamicMessage,
    name: &str,
) -> std::borrow::Cow<'a, Value> {
    msg.get_field_by_name(name)
        .unwrap_or_else(|| panic!("field {name} not found"))
}

// ---------------- scalars ----------------

#[test]
fn scalars_string_field() {
    let m = decode(r#"string_field = "hello world""#);
    match field_value(&m, "string_field").into_owned() {
        Value::String(s) => assert_eq!(s, "hello world"),
        v => panic!("expected string, got {v:?}"),
    }
}

#[test]
fn scalars_int32_field_via_int_token() {
    let m = decode("int32_field = -42");
    match field_value(&m, "int32_field").into_owned() {
        Value::I32(n) => assert_eq!(n, -42),
        v => panic!("expected i32, got {v:?}"),
    }
}

#[test]
fn scalars_int64_field() {
    let m = decode("int64_field = 9007199254740993");
    match field_value(&m, "int64_field").into_owned() {
        Value::I64(n) => assert_eq!(n, 9_007_199_254_740_993),
        v => panic!("expected i64, got {v:?}"),
    }
}

#[test]
fn scalars_uint32() {
    let m = decode("uint32_field = 4294967295");
    match field_value(&m, "uint32_field").into_owned() {
        Value::U32(n) => assert_eq!(n, u32::MAX),
        v => panic!("expected u32, got {v:?}"),
    }
}

#[test]
fn scalars_uint64() {
    let m = decode("uint64_field = 18446744073709551615");
    match field_value(&m, "uint64_field").into_owned() {
        Value::U64(n) => assert_eq!(n, u64::MAX),
        v => panic!("expected u64, got {v:?}"),
    }
}

#[test]
fn scalars_float_field() {
    let m = decode("float_field = 1.5");
    match field_value(&m, "float_field").into_owned() {
        Value::F32(f) => assert_eq!(f, 1.5_f32),
        v => panic!("expected f32, got {v:?}"),
    }
}

#[test]
fn scalars_double_field() {
    let m = decode("double_field = 1.23456789012345");
    match field_value(&m, "double_field").into_owned() {
        Value::F64(f) => assert!((f - 1.234_567_890_123_45_f64).abs() < 1e-12),
        v => panic!("expected f64, got {v:?}"),
    }
}

#[test]
fn scalars_bool_field() {
    let m = decode("bool_field = true");
    match field_value(&m, "bool_field").into_owned() {
        Value::Bool(b) => assert!(b),
        v => panic!("expected bool, got {v:?}"),
    }
}

#[test]
fn scalars_bytes_field_decodes_base64() {
    let m = decode(r#"bytes_field = b"3q2+7w==""#);
    match field_value(&m, "bytes_field").into_owned() {
        Value::Bytes(b) => assert_eq!(b, Bytes::from_static(&[0xde, 0xad, 0xbe, 0xef])),
        v => panic!("expected bytes, got {v:?}"),
    }
}

#[test]
fn scalars_rejects_out_of_range_int32() {
    let err = expect_err("int32_field = 99999999999");
    assert!(err.msg.contains("invalid int32"), "msg: {}", err.msg);
}

#[test]
fn scalars_rejects_type_mismatch() {
    let err = expect_err(r#"int32_field = "nope""#);
    assert!(err.msg.contains("expected integer"), "msg: {}", err.msg);
}

// ---------------- enums ----------------

#[test]
fn enums_by_name() {
    let m = decode("enum_field = STATUS_ACTIVE");
    match field_value(&m, "enum_field").into_owned() {
        Value::EnumNumber(n) => assert_eq!(n, 1),
        v => panic!("expected enum, got {v:?}"),
    }
}

#[test]
fn enums_by_number() {
    let m = decode("enum_field = 2");
    match field_value(&m, "enum_field").into_owned() {
        Value::EnumNumber(n) => assert_eq!(n, 2),
        v => panic!("expected enum, got {v:?}"),
    }
}

#[test]
fn enums_rejects_unknown_name() {
    let err = expect_err("enum_field = STATUS_BOGUS");
    assert!(err.msg.contains("unknown enum value"), "msg: {}", err.msg);
}

// ---------------- nested messages ----------------

fn nested_field<'a>(msg: &'a prost_reflect::DynamicMessage) -> &'a prost_reflect::DynamicMessage {
    let fd = msg
        .descriptor()
        .get_field_by_name("nested_field")
        .expect("nested_field fd missing");
    match msg.get_field(&fd) {
        std::borrow::Cow::Borrowed(Value::Message(m)) => m,
        _ => panic!("nested_field is not a borrowed message"),
    }
}

#[test]
fn nested_block_syntax() {
    let m = decode(
        r#"nested_field {
             name = "alice"
             value = 42
           }"#,
    );
    let sub = nested_field(&m);
    let n_fd = nested().get_field_by_name("name").unwrap();
    let v_fd = nested().get_field_by_name("value").unwrap();
    assert!(
        matches!(sub.get_field(&n_fd).into_owned(), Value::String(s) if s == "alice"),
    );
    assert!(matches!(sub.get_field(&v_fd).into_owned(), Value::I32(42)));
}

#[test]
fn nested_assignment_plus_block() {
    let m = decode(r#"nested_field = { name = "bob" value = 7 }"#);
    let sub = nested_field(&m);
    let n_fd = nested().get_field_by_name("name").unwrap();
    let v_fd = nested().get_field_by_name("value").unwrap();
    assert!(
        matches!(sub.get_field(&n_fd).into_owned(), Value::String(s) if s == "bob"),
    );
    assert!(matches!(sub.get_field(&v_fd).into_owned(), Value::I32(7)));
}

#[test]
fn nested_rejects_block_on_scalar_field() {
    let err = expect_err("int32_field { x = 1 }");
    assert!(err.msg.contains("not a message type"), "msg: {}", err.msg);
}

// ---------------- repeated ----------------

fn list_of(msg: &prost_reflect::DynamicMessage, name: &str) -> Vec<Value> {
    match field_value(msg, name).into_owned() {
        Value::List(items) => items,
        v => panic!("expected list, got {v:?}"),
    }
}

#[test]
fn repeated_string() {
    let m = decode(r#"repeated_string = ["a", "b", "c"]"#);
    let items = list_of(&m, "repeated_string");
    let strs: Vec<String> = items
        .into_iter()
        .map(|v| match v {
            Value::String(s) => s,
            other => panic!("expected string element, got {other:?}"),
        })
        .collect();
    assert_eq!(strs, vec!["a", "b", "c"]);
}

#[test]
fn repeated_string_with_trailing_comma() {
    let m = decode(r#"repeated_string = ["a", "b",]"#);
    let strs: Vec<String> = list_of(&m, "repeated_string")
        .into_iter()
        .map(|v| match v {
            Value::String(s) => s,
            other => panic!("expected string element, got {other:?}"),
        })
        .collect();
    assert_eq!(strs, vec!["a", "b"]);
}

#[test]
fn repeated_nested() {
    let m = decode(
        r#"repeated_nested = [
             { name = "x" value = 1 },
             { name = "y" value = 2 }
           ]"#,
    );
    let items = list_of(&m, "repeated_nested");
    assert_eq!(items.len(), 2);
    let n_fd = nested().get_field_by_name("name").unwrap();
    let v_fd = nested().get_field_by_name("value").unwrap();
    let first = match &items[0] {
        Value::Message(m) => m,
        v => panic!("expected message element, got {v:?}"),
    };
    assert!(
        matches!(first.get_field(&n_fd).into_owned(), Value::String(s) if s == "x"),
    );
    let second = match &items[1] {
        Value::Message(m) => m,
        v => panic!("expected message element, got {v:?}"),
    };
    assert!(matches!(
        second.get_field(&v_fd).into_owned(),
        Value::I32(2)
    ));
}

#[test]
fn repeated_rejects_null_element() {
    let err = expect_err(r#"repeated_string = ["a", null]"#);
    assert!(err.msg.contains("null is not allowed"), "msg: {}", err.msg);
}

#[test]
fn repeated_rejects_block_syntax() {
    let err = expect_err("repeated_string { x = 1 }");
    assert!(err.msg.contains("list syntax"), "msg: {}", err.msg);
}

// ---------------- oneof ----------------

#[test]
fn oneof_single_member_sets_cleanly() {
    let m = decode(r#"text_choice = "x""#);
    match field_value(&m, "text_choice").into_owned() {
        Value::String(s) => assert_eq!(s, "x"),
        v => panic!("expected string, got {v:?}"),
    }
}

#[test]
fn oneof_conflicting_members_error() {
    let err = expect_err("text_choice = \"x\"\nnumber_choice = 1");
    assert!(
        err.msg.contains("oneof") && err.msg.contains("conflicts"),
        "msg: {}",
        err.msg
    );
}

// ---------------- unknown fields ----------------

#[test]
fn unknown_field_errors_by_default() {
    let err = expect_err("bogus_field = 1");
    assert!(
        err.msg.contains("unknown field \"bogus_field\""),
        "msg: {}",
        err.msg
    );
}

#[test]
fn unknown_field_discard_skips_scalar() {
    let m = decode_with(
        "bogus_field = 1\nstring_field = \"ok\"",
        UnmarshalOptions {
            discard_unknown: true, ..Default::default()
        },
    );
    match field_value(&m, "string_field").into_owned() {
        Value::String(s) => assert_eq!(s, "ok"),
        v => panic!("expected string, got {v:?}"),
    }
}

#[test]
fn unknown_field_discard_skips_block() {
    let m = decode_with(
        "bogus_block { a = 1 b { c = 2 } }\nstring_field = \"ok\"",
        UnmarshalOptions {
            discard_unknown: true, ..Default::default()
        },
    );
    match field_value(&m, "string_field").into_owned() {
        Value::String(s) => assert_eq!(s, "ok"),
        v => panic!("expected string, got {v:?}"),
    }
}

#[test]
fn unknown_field_discard_skips_list() {
    let m = decode_with(
        "bogus_list = [1, 2, 3]\nstring_field = \"ok\"",
        UnmarshalOptions {
            discard_unknown: true, ..Default::default()
        },
    );
    match field_value(&m, "string_field").into_owned() {
        Value::String(s) => assert_eq!(s, "ok"),
        v => panic!("expected string, got {v:?}"),
    }
}

// ---------------- empty / @type ----------------

#[test]
fn empty_input_yields_default_message() {
    let m = decode("");
    match field_value(&m, "string_field").into_owned() {
        Value::String(s) => assert_eq!(s, ""),
        v => panic!("expected empty string, got {v:?}"),
    }
}

#[test]
fn leading_at_type_is_consumed() {
    let m = decode("@type test.v1.AllTypes\nstring_field = \"yes\"");
    match field_value(&m, "string_field").into_owned() {
        Value::String(s) => assert_eq!(s, "yes"),
        v => panic!("expected string, got {v:?}"),
    }
}
