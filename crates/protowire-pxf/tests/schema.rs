// SPDX-License-Identifier: MIT
// Copyright (c) 2026 TrendVidia, LLC.
//! Tests for the PXF schema reserved-name validator (draft §3.13) and
//! the unmarshal-time gate.
//!
//! Note on scope: `validate_descriptor(&desc)` walks the FILE the
//! descriptor lives in (matching Go's `ParentFile()` behavior), so a
//! single call returns every violation declared in `schema-test.proto`.
//! Tests filter by element FQN prefix to isolate specific cases.

use prost_reflect::{DescriptorPool, MessageDescriptor};
use protowire_pxf::{
    unmarshal, unmarshal_full, validate_descriptor, UnmarshalOptions, Violation, ViolationKind,
};

const FDS: &[u8] = include_bytes!("../testdata/schema-test.binpb");

fn pool() -> DescriptorPool {
    DescriptorPool::decode(FDS).expect("decode schema-test.binpb")
}

fn get(name: &str) -> MessageDescriptor {
    pool()
        .get_message_by_name(name)
        .unwrap_or_else(|| panic!("missing {}", name))
}

fn all_violations() -> Vec<Violation> {
    validate_descriptor(&get("schema.test.v1.Conformant"))
}

fn under(prefix: &str) -> Vec<Violation> {
    all_violations()
        .into_iter()
        .filter(|v| v.element == prefix || v.element.starts_with(&format!("{}.", prefix)))
        .collect()
}

#[test]
fn field_named_null_caught() {
    let vs = under("schema.test.v1.FieldNull");
    assert_eq!(vs.len(), 1);
    let v = &vs[0];
    assert_eq!(v.name, "null");
    assert_eq!(v.kind, ViolationKind::Field);
    assert_eq!(v.element, "schema.test.v1.FieldNull.null");
    assert!(v.to_string().contains("PXF-reserved name \"null\""));
}

#[test]
fn oneof_named_true_caught() {
    let vs = under("schema.test.v1.OneofTrue");
    assert_eq!(vs.len(), 1);
    assert_eq!(vs[0].kind, ViolationKind::Oneof);
    assert_eq!(vs[0].element, "schema.test.v1.OneofTrue.true");
}

#[test]
fn file_level_enum_value_named_false_caught() {
    // proto3 places file-level enum values at the FILE package scope:
    // `enum SideFalse { false = 1; }` → "schema.test.v1.false".
    let vs: Vec<_> = all_violations()
        .into_iter()
        .filter(|v| v.element == "schema.test.v1.false")
        .collect();
    assert_eq!(vs.len(), 1);
    assert_eq!(vs[0].kind, ViolationKind::EnumValue);
}

#[test]
fn nested_enum_value_caught() {
    let vs = under("schema.test.v1.OuterWithNestedEnum");
    assert_eq!(vs.len(), 1);
    assert_eq!(vs[0].kind, ViolationKind::EnumValue);
    // Nested enum values live at the enclosing message's scope.
    assert_eq!(vs[0].element, "schema.test.v1.OuterWithNestedEnum.null");
}

#[test]
fn nested_message_field_caught() {
    let vs: Vec<_> = under("schema.test.v1.OuterWithNestedMsg")
        .into_iter()
        .filter(|v| v.kind == ViolationKind::Field)
        .collect();
    assert_eq!(vs.len(), 1);
    assert_eq!(
        vs[0].element,
        "schema.test.v1.OuterWithNestedMsg.Inner.true"
    );
}

#[test]
fn case_sensitive_check() {
    // NULL / True don't lex as PXF keywords, so they don't trip the validator.
    assert!(under("schema.test.v1.CaseInsensitiveOK").is_empty());
}

#[test]
fn multi_violation_sort_by_element_fqn() {
    let vs = under("schema.test.v1.MultiViolations");
    let elements: Vec<&str> = vs.iter().map(|v| v.element.as_str()).collect();
    assert_eq!(
        elements,
        vec![
            "schema.test.v1.MultiViolations.false",
            "schema.test.v1.MultiViolations.null",
        ]
    );
}

#[test]
fn synthetic_oneof_from_proto3_optional_is_filtered() {
    // `optional int64 null = 1;` produces both a field named `null`
    // and a synthetic oneof — but is_synthetic() filters the latter,
    // so we expect exactly ONE violation (the field).
    let vs = under("schema.test.v1.SyntheticOneof");
    assert_eq!(vs.len(), 1);
    assert_eq!(vs[0].kind, ViolationKind::Field);
}

#[test]
fn file_path_in_violation_matches_proto_name() {
    let vs = under("schema.test.v1.FieldNull");
    assert_eq!(vs[0].file, "schema-test.proto");
}

// ---- unmarshal-time gate ----

#[test]
fn unmarshal_rejects_non_conformant_schema() {
    let err = unmarshal(
        "a = 1\n",
        &get("schema.test.v1.FieldNull"),
        UnmarshalOptions::default(),
    )
    .unwrap_err();
    assert!(err
        .to_string()
        .contains("PXF schema reserved-name violations"));
}

#[test]
fn unmarshal_full_also_gated() {
    let err = unmarshal_full(
        "a = 1\n",
        &get("schema.test.v1.FieldNull"),
        UnmarshalOptions::default(),
    )
    .unwrap_err();
    assert!(err
        .to_string()
        .contains("PXF schema reserved-name violations"));
}

#[test]
fn skip_validate_bypasses_check() {
    // Body doesn't reference the reserved-name field, so decode
    // succeeds once the gate is skipped.
    let opts = UnmarshalOptions {
        skip_validate: true,
        ..Default::default()
    };
    let msg = unmarshal("a = 1\n", &get("schema.test.v1.FieldNull"), opts).expect("decode ok");
    // Smoke-check we got something back.
    let _ = msg;
}

#[test]
fn conformant_schema_decodes_normally() {
    // Conformant shares the file with non-conformant messages; the
    // validator-as-gate would block unmarshal calls on ANY message in
    // the file. Use skip_validate so we can still exercise decode
    // semantics — the validator's own coverage is in the tests above.
    let opts = UnmarshalOptions {
        skip_validate: true,
        ..Default::default()
    };
    let msg = unmarshal(
        "price = 100\nqty = 5\n",
        &get("schema.test.v1.Conformant"),
        opts,
    )
    .expect("decode ok");
    let _ = msg;
}
