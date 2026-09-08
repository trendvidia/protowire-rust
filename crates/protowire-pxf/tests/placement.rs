// SPDX-License-Identifier: MIT
// Copyright (c) 2026 TrendVidia, LLC.
//! Bind-time placement checks of protowire v1.11 (protowire-rust#25):
//! `(pxf.key)` placement (draft -01 §3.13.1 "Schema Placement"),
//! `(pxf.default)` placement ("Default Placement"), and the two oneof
//! rules ("Oneof Members"). One fixture file per violating shape, since a
//! violating file poisons every message declared in it; the accepted
//! placements live together in a file that must validate clean. Mirrors
//! protowire-go's `default_placement_test.go` and
//! `oneof_annotation_test.go`.

use std::io::Cursor;

use prost_reflect::{DescriptorPool, DynamicMessage, MessageDescriptor, Value};
use protowire_pxf::{
    bind_row, unmarshal, unmarshal_full, validate_descriptor, DatasetReader, UnmarshalOptions,
    Violation, ViolationKind,
};

fn msg(fds: &'static [u8], name: &str) -> MessageDescriptor {
    DescriptorPool::decode(fds)
        .expect("decode descriptor set")
        .get_message_by_name(name)
        .unwrap_or_else(|| panic!("missing {name}"))
}

const DEFAULTS: &[u8] = include_bytes!("../testdata/default-placement-test.binpb");
const GROUP: &[u8] = include_bytes!("../testdata/group-placement-test.binpb");
const ACCEPTED: &[u8] = include_bytes!("../testdata/placement-accepted-test.binpb");
const KEYS: &[u8] = include_bytes!("../testdata/key-placement-test.binpb");
const TWO_DEFAULTS: &[u8] = include_bytes!("../testdata/oneof-two-defaults-test.binpb");
const REQUIRED: &[u8] = include_bytes!("../testdata/oneof-required-test.binpb");

fn only(vs: &[Violation], element: &str) -> Violation {
    let hits: Vec<&Violation> = vs.iter().filter(|v| v.element == element).collect();
    assert_eq!(hits.len(), 1, "{element}: {vs:?}");
    hits[0].clone()
}

fn skip_validate() -> UnmarshalOptions<'static> {
    UnmarshalOptions {
        skip_validate: true,
        ..Default::default()
    }
}

// ---------------- (pxf.key) placement, draft -01 §3.13.1 ----------------

#[test]
fn key_placement_violations() {
    let vs = validate_descriptor(&msg(KEYS, "key_placement_test.v1.Item"));
    assert_eq!(vs.len(), 5, "{vs:?}");
    for (element, key, detail) in [
        (
            "key_placement_test.v1.KeyOnScalar.tags",
            "name",
            "(pxf.key) is valid only on repeated message-typed fields",
        ),
        (
            "key_placement_test.v1.KeyOnSingularMessage.item",
            "name",
            "(pxf.key) is valid only on repeated message-typed fields",
        ),
        (
            "key_placement_test.v1.KeyNamesMissingField.items",
            "id",
            "element message key_placement_test.v1.Item has no field \"id\"",
        ),
        (
            "key_placement_test.v1.KeyNamesNonStringField.items",
            "n",
            "key field key_placement_test.v1.Item.n must be a singular string field",
        ),
        (
            "key_placement_test.v1.KeyNamesRepeatedField.items",
            "names",
            "key field key_placement_test.v1.Item.names must be a singular string field",
        ),
    ] {
        let v = only(&vs, element);
        assert_eq!(v.kind, ViolationKind::KeyOption, "{element}");
        assert_eq!(v.name, key, "{element}");
        assert_eq!(v.detail, detail, "{element}");
        assert_eq!(v.file, "key-placement-test.proto");
    }
    let v = only(&vs, "key_placement_test.v1.KeyNamesMissingField.items");
    assert_eq!(
        v.to_string(),
        "key-placement-test.proto: field \"key_placement_test.v1.KeyNamesMissingField.items\": invalid (pxf.key) = \"id\": element message key_placement_test.v1.Item has no field \"id\" (draft -01 §3.13)"
    );
}

// ---------------- (pxf.default) placement, "Default Placement" ----------------

#[test]
fn default_placement_violations_one_per_shape() {
    let vs = validate_descriptor(&msg(DEFAULTS, "default_placement_test.v1.Sibling"));
    assert_eq!(vs.len(), 5, "{vs:?}");
    for (element, literal, detail) in [
        ("default_placement_test.v1.ListMessage.ds", "5s", "(pxf.default) is not valid on repeated fields: one literal cannot denote a list"),
        ("default_placement_test.v1.MapDefault.labels", "ignored", "(pxf.default) is not valid on map fields: one literal cannot denote a map"),
        ("default_placement_test.v1.MapMessage.m", "5s", "(pxf.default) is not valid on map fields: one literal cannot denote a map"),
        ("default_placement_test.v1.MessageNonWkt.p", "ignored", "(pxf.default) is not valid on message type default_placement_test.v1.Plain: no PXF literal denotes it"),
        ("default_placement_test.v1.RepeatedDefault.tags", "ignored", "(pxf.default) is not valid on repeated fields: one literal cannot denote a list"),
    ] {
        let v = only(&vs, element);
        assert_eq!(v.kind, ViolationKind::DefaultOption, "{element}");
        assert_eq!(v.name, literal, "{element}");
        assert_eq!(v.detail, detail, "{element}");
    }
    // Sorted by element within the file.
    let elements: Vec<&str> = vs.iter().map(|v| v.element.as_str()).collect();
    let mut sorted = elements.clone();
    sorted.sort();
    assert_eq!(elements, sorted);
    assert_eq!(
        only(&vs, "default_placement_test.v1.RepeatedDefault.tags").to_string(),
        "default-placement-test.proto: field \"default_placement_test.v1.RepeatedDefault.tags\": invalid (pxf.default) = \"ignored\": (pxf.default) is not valid on repeated fields: one literal cannot denote a list (draft -01 §annotation-extensions)"
    );
}

#[test]
fn default_on_group_field() {
    let vs = validate_descriptor(&msg(GROUP, "group_placement_test.v1.WithGroup"));
    assert_eq!(vs.len(), 1, "{vs:?}");
    assert_eq!(vs[0].kind, ViolationKind::DefaultOption);
    assert_eq!(vs[0].element, "group_placement_test.v1.WithGroup.sub");
    assert_eq!(vs[0].detail, "(pxf.default) is not valid on group fields");
}

/// Every placement the section accepts validates clean: every scalar
/// kind, an enum, exactly the message types a literal can denote, a
/// proto3 optional, one default per oneof, a required field outside any
/// oneof, and a well-placed (pxf.key). And the defaults apply.
#[test]
fn accepted_placements_are_clean_and_apply() {
    let desc = msg(
        ACCEPTED,
        "placement_accepted_test.v1.EveryAcceptedPlacement",
    );
    assert!(validate_descriptor(&desc).is_empty());
    let keyed = msg(ACCEPTED, "placement_accepted_test.v1.KeyedOk");
    assert!(validate_descriptor(&keyed).is_empty());

    let (m, _) = unmarshal_full("req = \"x\"", &desc, UnmarshalOptions::default()).expect("binds");
    let get = |name: &str| m.get_field_by_name(name).unwrap().into_owned();
    assert_eq!(get("s"), Value::String("hello".into()));
    assert_eq!(get("i32"), Value::I32(-1));
    assert_eq!(get("r"), Value::EnumNumber(1));
    assert_eq!(get("c"), Value::String("ccc".into()));
    assert_eq!(get("opt"), Value::String("syn".into()));
    let Value::Message(bi) = get("bi") else {
        panic!("bi")
    };
    assert_eq!(
        bi.get_field_by_name("abs").unwrap().into_owned(),
        Value::Bytes(vec![42u8].into())
    );
    let Value::Message(dur) = get("dur") else {
        panic!("dur")
    };
    assert_eq!(
        dur.get_field_by_name("seconds").unwrap().into_owned(),
        Value::I64(5)
    );
    let Value::Message(w) = get("w_string") else {
        panic!("w_string")
    };
    assert_eq!(
        w.get_field_by_name("value").unwrap().into_owned(),
        Value::String("hi".into())
    );
}

/// A `(pxf.default)` on a `pxf.BigFloat` field binds and applies
/// (protowire-rust#39): `2.718` at 256 bits, the reference's bytes.
#[test]
fn big_float_default_placement_binds_and_applies() {
    let desc = msg(
        ACCEPTED,
        "placement_accepted_test.v1.EveryAcceptedPlacement",
    );
    assert!(validate_descriptor(&desc).is_empty());
    let (m, _) = unmarshal_full("req = \"x\"", &desc, UnmarshalOptions::default()).expect("binds");
    let Value::Message(bf) = m.get_field_by_name("bf").unwrap().into_owned() else {
        panic!("bf")
    };
    let Value::Bytes(mant) = bf.get_field_by_name("mantissa").unwrap().into_owned() else {
        panic!("mantissa")
    };
    assert_eq!(&mant[..4], &[0xad, 0xf3, 0xb6, 0x45]);
    assert_eq!(
        bf.get_field_by_name("exponent").unwrap().into_owned(),
        Value::I32(-254)
    );
}

/// A violating file poisons every message declared in it: the conformant
/// `Sibling` does not bind either. Intended — a widely-imported file is
/// where a dead annotation does the most damage.
#[test]
fn bad_placement_poisons_the_whole_file() {
    let sibling = msg(DEFAULTS, "default_placement_test.v1.Sibling");
    let err = unmarshal("ok = \"x\"", &sibling, UnmarshalOptions::default())
        .expect_err("sibling inherits the file's violations");
    assert!(err.msg.starts_with("PXF schema violations:"), "{}", err.msg);
    assert!(err.msg.contains("RepeatedDefault.tags"), "{}", err.msg);
}

// ---------------- Oneof rules, "Oneof Members" ----------------

/// At most one member of a oneof may carry (pxf.default); every offending
/// member is reported, with the same detail, and a oneof with exactly one
/// is not.
#[test]
fn two_defaults_on_one_oneof_are_both_reported() {
    let vs = validate_descriptor(&msg(TWO_DEFAULTS, "oneof_two_defaults_test.v1.TwoDefaults"));
    assert_eq!(vs.len(), 2, "{vs:?}");
    let detail = "at most one member of oneof \"choice\" may carry a default; 2 do (a, b)";
    let a = only(&vs, "oneof_two_defaults_test.v1.TwoDefaults.a");
    assert_eq!(
        (a.kind, a.name.as_str(), a.detail.as_str()),
        (ViolationKind::DefaultOption, "aaa", detail)
    );
    let b = only(&vs, "oneof_two_defaults_test.v1.TwoDefaults.b");
    assert_eq!(
        (b.kind, b.name.as_str(), b.detail.as_str()),
        (ViolationKind::DefaultOption, "bbb", detail)
    );
    assert!(
        vs.iter().all(|v| !v.element.ends_with(".x")),
        "one default in `second` is fine"
    );
}

#[test]
fn required_on_oneof_member_is_rejected() {
    let vs = validate_descriptor(&msg(REQUIRED, "oneof_required_test.v1.RequiredMember"));
    assert_eq!(vs.len(), 1, "{vs:?}");
    let v = &vs[0];
    assert_eq!(v.kind, ViolationKind::RequiredOption);
    assert_eq!(v.element, "oneof_required_test.v1.RequiredMember.a");
    assert_eq!(v.name, "choice");
    assert_eq!(
        v.to_string(),
        "oneof-required-test.proto: field \"oneof_required_test.v1.RequiredMember.a\": invalid (pxf.required): (pxf.required) is not valid on a member of oneof \"choice\": read per field it demands that one arm always be chosen, which makes every other arm undecodable (draft -01 §annotation-extensions)"
    );
    // The document choosing the other arm used to be rejected at decode
    // time for the absence of `a`; now the schema does not bind at all.
    let err = unmarshal_full(
        "b = \"x\"",
        &msg(REQUIRED, "oneof_required_test.v1.RequiredMember"),
        UnmarshalOptions::default(),
    )
    .expect_err("schema must not bind");
    assert!(err.msg.contains("invalid (pxf.required)"), "{}", err.msg);
}

// ---------------- Enforcement at every entry point ----------------

#[test]
fn placement_is_rejected_by_every_entry_point_and_bypassed_by_skip_validate() {
    let desc = msg(DEFAULTS, "default_placement_test.v1.RepeatedDefault");
    let want = "invalid (pxf.default) = \"ignored\": (pxf.default) is not valid on repeated fields";

    let err = unmarshal("", &desc, UnmarshalOptions::default()).expect_err("unmarshal");
    assert!(err.msg.contains(want), "unmarshal: {}", err.msg);
    let err = unmarshal_full("", &desc, UnmarshalOptions::default()).expect_err("unmarshal_full");
    assert!(err.msg.contains(want), "unmarshal_full: {}", err.msg);
    // Rejected independently of what the document contains: even when
    // the annotated field is present.
    let err = unmarshal("tags = [\"a\"]", &desc, UnmarshalOptions::default()).expect_err("present");
    assert!(err.msg.contains(want), "present: {}", err.msg);

    // The dataset paths bind through unmarshal and reject the same way
    // (the schema check runs before any cell is bound, so the scalar cell
    // never reaches the type check).
    let doc = "@dataset default_placement_test.v1.RepeatedDefault (tags)\n(\"a\")\n";
    let mut r = DatasetReader::new(Cursor::new(doc)).expect("header");
    let err = r
        .scan_one(&desc, UnmarshalOptions::default())
        .expect_err("scan_one");
    assert!(err.msg.contains(want), "scan_one: {}", err.msg);
    let mut r = DatasetReader::new(Cursor::new(doc)).expect("header");
    let row = r.next_row().unwrap().unwrap();
    let err = bind_row(
        &desc,
        &["tags".to_string()],
        &row,
        UnmarshalOptions::default(),
    )
    .expect_err("bind_row");
    assert!(err.msg.contains(want), "bind_row: {}", err.msg);

    // skip_validate bypasses the bind-time check; the runtime guard
    // (#23) then refuses the placement where it would be applied, and a
    // plain unmarshal — which never applies defaults — decodes.
    let m: DynamicMessage =
        unmarshal("tags = [\"a\"]", &desc, skip_validate()).expect("skip_validate");
    assert_eq!(
        m.get_field_by_name("tags").unwrap().into_owned(),
        Value::List(vec![Value::String("a".into())])
    );
    let err = unmarshal_full("", &desc, skip_validate()).expect_err("runtime guard");
    assert_eq!(
        err.msg,
        "default values not supported for repeated field \"tags\""
    );
}
