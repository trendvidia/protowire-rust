// SPDX-License-Identifier: MIT
// Copyright (c) 2026 TrendVidia, LLC.
//! Slice D3 decoder tests: `google.protobuf.Any` sugar via `TypeResolver`.
//! Mirrors the `pxf.unmarshal — google.protobuf.Any` block in the TS port's
//! `pxf/decode.test.ts`.

use bytes::Bytes;
use prost_reflect::{DescriptorPool, DynamicMessage, MessageDescriptor, ReflectMessage, Value};
use protowire_pxf::{unmarshal, PoolResolver, PxfError, UnmarshalOptions};

const ANY_FDS: &[u8] = include_bytes!("../testdata/any-test.binpb");

fn pool() -> DescriptorPool {
    DescriptorPool::decode(ANY_FDS).expect("decode any-test.binpb")
}

fn container() -> MessageDescriptor {
    pool()
        .get_message_by_name("any_test.v1.Container")
        .expect("missing any_test.v1.Container")
}

fn detail() -> MessageDescriptor {
    pool()
        .get_message_by_name("any_test.v1.Detail")
        .expect("missing any_test.v1.Detail")
}

fn payload<'a>(msg: &'a DynamicMessage) -> &'a DynamicMessage {
    let fd = msg.descriptor().get_field_by_name("payload").unwrap();
    match msg.get_field(&fd) {
        std::borrow::Cow::Borrowed(Value::Message(m)) => m,
        _ => panic!("payload is not a borrowed message"),
    }
}

fn type_url_value(any_msg: &DynamicMessage) -> (String, Bytes) {
    let t_fd = any_msg.descriptor().get_field_by_name("type_url").unwrap();
    let v_fd = any_msg.descriptor().get_field_by_name("value").unwrap();
    let url = match any_msg.get_field(&t_fd).into_owned() {
        Value::String(s) => s,
        v => panic!("type_url not string: {v:?}"),
    };
    let bytes = match any_msg.get_field(&v_fd).into_owned() {
        Value::Bytes(b) => b,
        v => panic!("value not bytes: {v:?}"),
    };
    (url, bytes)
}

fn decode_detail(bytes: &Bytes) -> DynamicMessage {
    DynamicMessage::decode(detail(), &bytes[..]).expect("decode Detail")
}

fn detail_field(d: &DynamicMessage, name: &str) -> Value {
    let fd = d.descriptor().get_field_by_name(name).unwrap();
    d.get_field(&fd).into_owned()
}

#[test]
fn any_decodes_block_syntax_via_type_lookup() {
    let p = pool();
    let resolver = PoolResolver(&p);
    let m = unmarshal(
        r#"name = "test"
           payload {
             @type = "any_test.v1.Detail"
             code = 42
             reason = "not found"
           }"#,
        &container(),
        UnmarshalOptions {
            type_resolver: Some(&resolver),
            ..Default::default()
        },
    )
    .expect("decode ok");
    let any_msg = payload(&m);
    let (url, bytes) = type_url_value(any_msg);
    assert_eq!(url, "any_test.v1.Detail");
    let inner = decode_detail(&bytes);
    assert!(matches!(detail_field(&inner, "code"), Value::I32(42)));
    assert!(matches!(detail_field(&inner, "reason"), Value::String(s) if s == "not found"));
}

#[test]
fn any_decodes_assignment_syntax_via_type_lookup() {
    let p = pool();
    let resolver = PoolResolver(&p);
    let m = unmarshal(
        r#"payload = { @type = "any_test.v1.Detail" code = 7 }"#,
        &container(),
        UnmarshalOptions {
            type_resolver: Some(&resolver),
            ..Default::default()
        },
    )
    .expect("decode ok");
    let any_msg = payload(&m);
    let (url, bytes) = type_url_value(any_msg);
    assert_eq!(url, "any_test.v1.Detail");
    let inner = decode_detail(&bytes);
    assert!(matches!(detail_field(&inner, "code"), Value::I32(7)));
}

#[test]
fn any_strips_type_googleapis_com_prefix_when_looking_up() {
    let p = pool();
    let resolver = PoolResolver(&p);
    let m = unmarshal(
        r#"payload { @type = "type.googleapis.com/any_test.v1.Detail" code = 9 }"#,
        &container(),
        UnmarshalOptions {
            type_resolver: Some(&resolver),
            ..Default::default()
        },
    )
    .expect("decode ok");
    let any_msg = payload(&m);
    let (url, bytes) = type_url_value(any_msg);
    assert_eq!(url, "type.googleapis.com/any_test.v1.Detail");
    let inner = decode_detail(&bytes);
    assert!(matches!(detail_field(&inner, "code"), Value::I32(9)));
}

#[test]
fn any_errors_on_unresolvable_type_when_resolver_set() {
    let p = pool();
    let resolver = PoolResolver(&p);
    let err: PxfError = unmarshal(
        r#"payload { @type = "any_test.v1.Missing" code = 1 }"#,
        &container(),
        UnmarshalOptions {
            type_resolver: Some(&resolver),
            ..Default::default()
        },
    )
    .expect_err("expected unresolvable Any error");
    assert!(
        err.msg.contains("cannot resolve Any type"),
        "msg: {}",
        err.msg
    );
}

#[test]
fn any_without_resolver_decodes_as_plain_message() {
    let m = unmarshal(
        r#"payload { type_url = "any_test.v1.Detail" value = b"" }"#,
        &container(),
        UnmarshalOptions::default(),
    )
    .expect("decode ok");
    let any_msg = payload(&m);
    let (url, bytes) = type_url_value(any_msg);
    assert_eq!(url, "any_test.v1.Detail");
    assert!(bytes.is_empty());
}
