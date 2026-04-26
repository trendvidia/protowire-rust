//! Slice E encoder tests for the schema-bound PXF marshaller.
//! Mirrors the corresponding `describe()` blocks in the TS port's
//! `pxf/encode.test.ts`.

use prost::Message as _;
use prost_reflect::{DescriptorPool, DynamicMessage, MessageDescriptor, Value};
use protowire_pxf::{
    marshal, unmarshal, unmarshal_full, MarshalOptions, PoolResolver, Presence,
    UnmarshalOptions,
};

const TEST_FDS: &[u8] = include_bytes!("../testdata/test.binpb");
const ANY_FDS: &[u8] = include_bytes!("../testdata/any-test.binpb");
const D4_FDS: &[u8] = include_bytes!("../testdata/d4-test.binpb");

fn pool(bytes: &[u8]) -> DescriptorPool {
    DescriptorPool::decode(bytes).expect("decode FDS")
}

fn msg(p: &DescriptorPool, name: &str) -> MessageDescriptor {
    p.get_message_by_name(name)
        .unwrap_or_else(|| panic!("missing {name}"))
}

fn all_types() -> MessageDescriptor {
    msg(&pool(TEST_FDS), "test.v1.AllTypes")
}

fn decode_to_all_types(input: &str) -> DynamicMessage {
    unmarshal(input, &all_types(), UnmarshalOptions::default()).expect("decode")
}

fn encode_default(m: &DynamicMessage, desc: &MessageDescriptor) -> String {
    marshal(m, desc, MarshalOptions::default())
}

// ---------------- scalars ----------------

#[test]
fn scalars_emit_set_string_skip_zero_proto3() {
    let m = decode_to_all_types(r#"string_field = "hello""#);
    assert_eq!(encode_default(&m, &all_types()), "string_field = \"hello\"\n");
}

#[test]
fn scalars_emit_zero_when_emit_defaults() {
    let m = decode_to_all_types("");
    let out = marshal(
        &m,
        &all_types(),
        MarshalOptions {
            emit_defaults: true,
            ..Default::default()
        },
    );
    assert!(out.contains("string_field = \"\""), "{out}");
    assert!(out.contains("int32_field = 0"), "{out}");
    assert!(out.contains("bool_field = false"), "{out}");
}

#[test]
fn scalars_escape_control_chars_and_quotes() {
    let m = decode_to_all_types(r#"string_field = "a\nb\"c""#);
    assert_eq!(
        encode_default(&m, &all_types()),
        "string_field = \"a\\nb\\\"c\"\n"
    );
}

#[test]
fn scalars_encode_bytes_as_base64() {
    let m = decode_to_all_types(r#"bytes_field = b"3q2+7w==""#);
    assert_eq!(
        encode_default(&m, &all_types()),
        "bytes_field = b\"3q2+7w==\"\n"
    );
}

// ---------------- enums and oneofs ----------------

#[test]
fn enum_emits_by_name_when_known() {
    let m = decode_to_all_types("enum_field = STATUS_ACTIVE");
    assert_eq!(
        encode_default(&m, &all_types()),
        "enum_field = STATUS_ACTIVE\n"
    );
}

#[test]
fn oneof_emits_selected_member_only() {
    let m = decode_to_all_types(r#"text_choice = "x""#);
    let out = encode_default(&m, &all_types());
    assert!(out.contains(r#"text_choice = "x""#), "{out}");
    assert!(!out.contains("number_choice"), "{out}");
}

// ---------------- messages and repeated ----------------

#[test]
fn nested_message_block_syntax() {
    let m = decode_to_all_types(r#"nested_field { name = "alice" value = 7 }"#);
    assert_eq!(
        encode_default(&m, &all_types()),
        "nested_field {\n  name = \"alice\"\n  value = 7\n}\n"
    );
}

#[test]
fn repeated_scalar_list() {
    let m = decode_to_all_types(r#"repeated_string = ["a", "b", "c"]"#);
    assert_eq!(
        encode_default(&m, &all_types()),
        "repeated_string = [\n  \"a\",\n  \"b\",\n  \"c\"\n]\n"
    );
}

#[test]
fn repeated_message_list_with_block_elements() {
    let m = decode_to_all_types(
        r#"repeated_nested = [
             { name = "x" value = 1 },
             { name = "y" value = 2 }
           ]"#,
    );
    assert_eq!(
        encode_default(&m, &all_types()),
        concat!(
            "repeated_nested = [\n",
            "  {\n    name = \"x\"\n    value = 1\n  },\n",
            "  {\n    name = \"y\"\n    value = 2\n  }\n",
            "]\n"
        )
    );
}

// ---------------- maps ----------------

#[test]
fn map_string_string_sorted_by_key() {
    let m = decode_to_all_types(r#"string_map = { foo: "1" bar: "2" zed: "3" }"#);
    assert_eq!(
        encode_default(&m, &all_types()),
        "string_map = {\n  bar: \"2\"\n  foo: \"1\"\n  zed: \"3\"\n}\n"
    );
}

#[test]
fn map_int32_string_sorted_by_stringified_key() {
    let m = decode_to_all_types(r#"int_map = { 10: "ten" 2: "two" }"#);
    assert_eq!(
        encode_default(&m, &all_types()),
        "int_map = {\n  10: \"ten\"\n  2: \"two\"\n}\n"
    );
}

#[test]
fn map_quotes_non_identifier_string_keys() {
    let m = decode_to_all_types(r#"string_map = { "with space": "v" }"#);
    assert_eq!(
        encode_default(&m, &all_types()),
        "string_map = {\n  \"with space\": \"v\"\n}\n"
    );
}

#[test]
fn map_message_valued_entries_as_blocks() {
    let m = decode_to_all_types(r#"nested_map = { a: { name = "alice" value = 1 } }"#);
    assert_eq!(
        encode_default(&m, &all_types()),
        "nested_map = {\n  a: {\n    name = \"alice\"\n    value = 1\n  }\n}\n"
    );
}

// ---------------- well-known types ----------------

#[test]
fn timestamp_without_fractional_seconds() {
    let m = decode_to_all_types("ts_field = 2024-01-01T12:00:00Z");
    assert_eq!(
        encode_default(&m, &all_types()),
        "ts_field = 2024-01-01T12:00:00Z\n"
    );
}

#[test]
fn timestamp_with_nanoseconds_trims_trailing_zeros() {
    let m = decode_to_all_types("ts_field = 1970-01-01T00:00:00.123456789Z");
    assert_eq!(
        encode_default(&m, &all_types()),
        "ts_field = 1970-01-01T00:00:00.123456789Z\n"
    );
}

#[test]
fn duration_hour_plus_minute_composition() {
    let m = decode_to_all_types("dur_field = 1h30m");
    assert_eq!(encode_default(&m, &all_types()), "dur_field = 1h30m0s\n");
}

#[test]
fn duration_sub_millisecond_uses_micro_unit() {
    let m = decode_to_all_types("dur_field = 500us");
    assert_eq!(encode_default(&m, &all_types()), "dur_field = 500µs\n");
}

#[test]
fn duration_explicit_zero_prints_zero_seconds() {
    let m = decode_to_all_types("dur_field = 0s");
    assert_eq!(encode_default(&m, &all_types()), "dur_field = 0s\n");
}

#[test]
fn wrapper_string_emits_as_bare_scalar() {
    let m = decode_to_all_types(r#"nullable_string = "hi""#);
    assert_eq!(
        encode_default(&m, &all_types()),
        "nullable_string = \"hi\"\n"
    );
}

#[test]
fn wrapper_int32_emits_as_bare_integer() {
    let m = decode_to_all_types("nullable_int = 7");
    assert_eq!(encode_default(&m, &all_types()), "nullable_int = 7\n");
}

// ---------------- Any sugar ----------------

#[test]
fn any_emits_at_type_plus_inline_when_resolver_finds_url() {
    let p = pool(ANY_FDS);
    let resolver = PoolResolver(&p);
    let container = msg(&p, "any_test.v1.Container");
    let detail = msg(&p, "any_test.v1.Detail");

    let mut detail_msg = DynamicMessage::new(detail.clone());
    let code_fd = detail.get_field_by_name("code").unwrap();
    let reason_fd = detail.get_field_by_name("reason").unwrap();
    detail_msg.set_field(&code_fd, Value::I32(42));
    detail_msg.set_field(&reason_fd, Value::String("boom".into()));
    let packed = detail_msg.encode_to_vec();

    let mut c = DynamicMessage::new(container.clone());
    let name_fd = container.get_field_by_name("name").unwrap();
    let payload_fd = container.get_field_by_name("payload").unwrap();
    c.set_field(&name_fd, Value::String("test".into()));
    let any_desc = match payload_fd.kind() {
        prost_reflect::Kind::Message(m) => m,
        _ => panic!("payload not message"),
    };
    let mut any_msg = DynamicMessage::new(any_desc.clone());
    let type_url_fd = any_desc.get_field_by_name("type_url").unwrap();
    let value_fd = any_desc.get_field_by_name("value").unwrap();
    any_msg.set_field(&type_url_fd, Value::String("any_test.v1.Detail".into()));
    any_msg.set_field(&value_fd, Value::Bytes(packed.into()));
    c.set_field(&payload_fd, Value::Message(any_msg));

    let out = marshal(
        &c,
        &container,
        MarshalOptions {
            type_resolver: Some(&resolver),
            ..Default::default()
        },
    );
    assert!(out.contains("payload {"), "{out}");
    assert!(out.contains(r#"@type = "any_test.v1.Detail""#), "{out}");
    assert!(out.contains("code = 42"), "{out}");
    assert!(out.contains(r#"reason = "boom""#), "{out}");
}

#[test]
fn any_falls_back_to_plain_block_when_no_resolver() {
    let p = pool(ANY_FDS);
    let container = msg(&p, "any_test.v1.Container");
    let payload_fd = container.get_field_by_name("payload").unwrap();
    let any_desc = match payload_fd.kind() {
        prost_reflect::Kind::Message(m) => m,
        _ => panic!("payload not message"),
    };
    let mut any_msg = DynamicMessage::new(any_desc.clone());
    let type_url_fd = any_desc.get_field_by_name("type_url").unwrap();
    let value_fd = any_desc.get_field_by_name("value").unwrap();
    any_msg.set_field(&type_url_fd, Value::String("any_test.v1.Detail".into()));
    any_msg.set_field(&value_fd, Value::Bytes(vec![1, 2, 3].into()));

    let mut c = DynamicMessage::new(container.clone());
    c.set_field(&payload_fd, Value::Message(any_msg));

    let out = marshal(&c, &container, MarshalOptions::default());
    assert!(out.contains(r#"type_url = "any_test.v1.Detail""#), "{out}");
    assert!(out.contains("value = b\""), "{out}");
}

// ---------------- null emission ----------------

#[test]
fn null_reads_paths_from_in_message_null_mask() {
    let with_null_mask = msg(&pool(D4_FDS), "d4_test.v1.WithNullMask");
    let (m, _) = unmarshal_full(
        "name = \"n\"\nvalue = null",
        &with_null_mask,
        UnmarshalOptions::default(),
    )
    .expect("ok");
    let out = marshal(&m, &with_null_mask, MarshalOptions::default());
    assert!(out.contains("value = null"), "{out}");
    assert!(!out.contains("_null"), "{out}");
}

#[test]
fn null_uses_marshal_options_null_fields_when_no_mask() {
    let m = decode_to_all_types(r#"string_field = "x""#);
    let mut presence = Presence::new();
    presence.mark_null("nullable_string");
    let out = marshal(
        &m,
        &all_types(),
        MarshalOptions {
            null_fields: Some(&presence),
            ..Default::default()
        },
    );
    assert!(out.contains("nullable_string = null"), "{out}");
}

// ---------------- typeURL prefix ----------------

#[test]
fn type_url_prepends_at_type_plus_blank_line() {
    let m = decode_to_all_types("int32_field = 1");
    let out = marshal(
        &m,
        &all_types(),
        MarshalOptions {
            type_url: Some("test.v1.AllTypes"),
            ..Default::default()
        },
    );
    assert!(
        out.starts_with("@type test.v1.AllTypes\n\n"),
        "out: {out:?}"
    );
}

// ---------------- round-trip ----------------

#[test]
fn round_trip_decode_encode_decode_preserves_message() {
    let input = "string_field = \"hello\"\n\
                 int32_field = -7\n\
                 int64_field = 9007199254740993\n\
                 uint32_field = 100\n\
                 float_field = 1.5\n\
                 bool_field = true\n\
                 bytes_field = b\"3q2+7w==\"\n\
                 enum_field = STATUS_ACTIVE\n\
                 nested_field { name = \"n\" value = 3 }\n\
                 repeated_string = [\"a\", \"b\"]\n\
                 string_map = { foo: \"1\" }\n\
                 ts_field = 2024-01-01T12:00:00Z\n\
                 dur_field = 5s\n\
                 nullable_string = \"hi\"\n\
                 text_choice = \"pick\"\n";
    let desc = all_types();
    let m1 = unmarshal(input, &desc, UnmarshalOptions::default()).expect("decode 1");
    let text = marshal(&m1, &desc, MarshalOptions::default());
    let m2 = unmarshal(&text, &desc, UnmarshalOptions::default()).expect("decode 2");
    assert_eq!(m1.encode_to_vec(), m2.encode_to_vec(), "wire mismatch\n{text}");
}
