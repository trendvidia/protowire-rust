// SPDX-License-Identifier: MIT
// Copyright (c) 2026 TrendVidia, LLC.
//! Slice D4 decoder tests: presence tracking via `Presence`,
//! `(pxf.required)` validation, `(pxf.default)` application, and the
//! `_null` `FieldMask` mirror channel. Mirrors the
//! `pxf.unmarshalFull — *` blocks in the TS port's `pxf/decode.test.ts`.

use prost::Message as _;
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
    let all_types = test_pool().get_message_by_name("test.v1.AllTypes").unwrap();
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
    let all_types = test_pool().get_message_by_name("test.v1.AllTypes").unwrap();
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

// ---------------- (pxf.default) on oneof members (#24) ----------------
//
// Setting any member of a oneof clears the others, so the per-field
// reading of "absent" does not hold inside one: a member is absent
// precisely when a sibling was chosen. Draft -01 §annotation-extensions
// ("Oneof Members") is the rule these pin.

/// The #24 repro: a document that chooses one arm keeps it. Before the fix
/// this returned a="" b="bbb" case=b — the written value cleared, not
/// shadowed.
#[test]
fn oneof_default_does_not_clobber_chosen_arm() {
    let desc = d4_msg("d4_test.v1.OneofDefault");
    let (m, p) = full("a = \"written\"", &desc);
    assert_eq!(field_value(&m, "a"), Value::String("written".into()));
    let b_fd = desc.get_field_by_name("b").unwrap();
    assert!(!m.has_field(&b_fd), "sibling default must not be applied");
    assert!(p.is_absent("b"));
    // The default outside the oneof still applies.
    assert_eq!(field_value(&m, "outside"), Value::String("out".into()));
    // Byte for byte: only field 1 reaches the wire.
    assert_eq!(
        m.encode_to_vec(),
        b"\x0a\x07written\x1a\x03out".to_vec(),
        "pb output must carry the arm the document chose"
    );
}

#[test]
fn oneof_default_applies_when_no_member_is_present() {
    let desc = d4_msg("d4_test.v1.OneofDefault");
    let (m, _) = full("outside = \"x\"", &desc);
    assert_eq!(field_value(&m, "b"), Value::String("bbb".into()));
    let a_fd = desc.get_field_by_name("a").unwrap();
    assert!(!m.has_field(&a_fd));
}

/// A member bound to `null` counts as present for the oneof test,
/// consistent with the rule that null suppresses a default.
#[test]
fn oneof_default_is_suppressed_by_null_sibling() {
    let desc = d4_msg("d4_test.v1.OneofNullableSibling");
    let (m, p) = full("w = null", &desc);
    assert!(p.is_null("w"));
    let b_fd = desc.get_field_by_name("b").unwrap();
    assert!(
        !m.has_field(&b_fd),
        "a null sibling is present; the default must not be applied"
    );
}

/// A proto3 `optional` field sits in a synthetic single-member oneof that
/// nothing can clear, so its default must keep applying.
#[test]
fn synthetic_oneof_default_still_applies() {
    let desc = d4_msg("d4_test.v1.SyntheticOneof");
    let (m, _) = full("", &desc);
    assert_eq!(field_value(&m, "opt"), Value::String("syn".into()));
}

// ---------------- (pxf.default) placement on repeated / map (#23, #25) ----------------
//
// A (pxf.default) carries one PXF literal, so it can denote a singular
// field only (draft -01 §annotation-extensions, "Default Placement"). The
// v1.11 bind-time check (#25) rejects the schema before any document is
// read; the runtime guard (#23) stays load-bearing for callers that set
// `skip_validate`, since a single literal on a repeated field MUST NOT be
// applied as a one-element list under any revision of the spec.

const PLACEMENT_FDS: &[u8] = include_bytes!("../testdata/default-placement-test.binpb");

fn placement_msg(name: &str) -> MessageDescriptor {
    DescriptorPool::decode(PLACEMENT_FDS)
        .expect("decode default-placement-test.binpb")
        .get_message_by_name(name)
        .unwrap_or_else(|| panic!("missing {name}"))
}

fn skip_validate() -> UnmarshalOptions<'static> {
    UnmarshalOptions {
        skip_validate: true,
        ..Default::default()
    }
}

#[test]
fn repeated_default_is_rejected_at_bind_time_and_by_the_runtime_guard() {
    let desc = placement_msg("default_placement_test.v1.RepeatedDefault");
    let err = unmarshal_full("", &desc, UnmarshalOptions::default()).expect_err("must reject");
    assert!(err.msg.starts_with("PXF schema violations:"), "{}", err.msg);
    assert!(
        err.msg.contains("field \"default_placement_test.v1.RepeatedDefault.tags\": invalid (pxf.default) = \"ignored\": (pxf.default) is not valid on repeated fields"),
        "{}",
        err.msg
    );
    // Bypassing the bind-time check, the placement is still never applied
    // as a one-element list.
    let err = unmarshal_full("", &desc, skip_validate()).expect_err("must reject");
    assert_eq!(
        err.msg,
        "default values not supported for repeated field \"tags\""
    );
}

#[test]
fn map_default_is_rejected_naming_the_placement() {
    let desc = placement_msg("default_placement_test.v1.MapDefault");
    let err = unmarshal_full("", &desc, UnmarshalOptions::default()).expect_err("must reject");
    assert!(
        err.msg.contains("field \"default_placement_test.v1.MapDefault.labels\": invalid (pxf.default) = \"ignored\": (pxf.default) is not valid on map fields"),
        "{}",
        err.msg
    );
    // The runtime guard names the placement, not the synthetic
    // `LabelsEntry` message type.
    let err = unmarshal_full("", &desc, skip_validate()).expect_err("must reject");
    assert_eq!(
        err.msg,
        "default values not supported for map field \"labels\""
    );
}

/// The bind-time check is independent of what the document contains: a
/// document that supplies the field is rejected too. Only with
/// `skip_validate` does it decode, and then the guard runs only for
/// absent fields.
#[test]
fn repeated_default_schema_is_rejected_even_when_field_is_supplied() {
    let desc = placement_msg("default_placement_test.v1.RepeatedDefault");
    unmarshal_full("tags = [\"a\"]", &desc, UnmarshalOptions::default())
        .expect_err("rejected independently of the document");
    let (m, _) = unmarshal_full("tags = [\"a\"]", &desc, skip_validate())
        .expect("supplied field decodes under skip_validate");
    assert_eq!(
        field_value(&m, "tags"),
        Value::List(vec![Value::String("a".into())])
    );
}
