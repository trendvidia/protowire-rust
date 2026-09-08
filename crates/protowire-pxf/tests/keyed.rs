// SPDX-License-Identifier: MIT
// Copyright (c) 2026 TrendVidia, LLC.
//! Keyed repeated fields (draft -01 §3.13; protowire#116,
//! protowire-rust#20). The fixture corpus under `testdata/keyed/` is
//! vendored verbatim from the spec repository and shared by every port;
//! its README states each document's verdict. Mirrors the reference's
//! `keyed_test.go`.

use std::path::{Path, PathBuf};

use prost::Message as _;
use prost_reflect::{DescriptorPool, DynamicMessage, MessageDescriptor, Value};
use protowire_pxf::{
    canonicalize_keyed, format, ident_safe_entry_name, key_field, marshal, parse, unmarshal,
    unmarshal_full, Entry, MarshalOptions, UnmarshalOptions,
};

const KEYED_FDS: &[u8] = include_bytes!("../testdata/keyed/keyed.binpb");
const TEST_FDS: &[u8] = include_bytes!("../testdata/test.binpb");

fn pool() -> DescriptorPool {
    DescriptorPool::decode(KEYED_FDS).expect("decode keyed.binpb")
}

fn fixture_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("testdata/keyed")
}

fn read(name: &str) -> String {
    std::fs::read_to_string(fixture_dir().join(name)).unwrap_or_else(|e| panic!("{name}: {e}"))
}

/// The fixture's message, resolved from its `@type` directive.
fn desc_of(doc: &str) -> MessageDescriptor {
    let parsed = parse(doc).expect("fixture parses");
    let name = parsed.type_url.rsplit('.').next().unwrap().to_string();
    pool()
        .get_message_by_name(&format!("keyed.v1.{name}"))
        .unwrap_or_else(|| panic!("keyed.v1.{name}"))
}

/// Strip comment lines and collapse the blank runs they leave behind,
/// yielding the byte-exact document the encoder is expected to reproduce
/// (the fixtures' comments are documentation, not content).
fn clean(data: &str) -> String {
    let kept: Vec<&str> = data
        .lines()
        .filter(|l| !l.trim_start().starts_with('#'))
        .collect();
    let mut s = kept.join("\n");
    while s.contains("\n\n\n") {
        s = s.replace("\n\n\n", "\n\n");
    }
    format!("{}\n", s.trim_end_matches('\n'))
}

fn decode(doc: &str, desc: &MessageDescriptor) -> DynamicMessage {
    unmarshal(doc, desc, UnmarshalOptions::default()).unwrap_or_else(|e| panic!("{e}\n{doc}"))
}

fn marshal_with_type(msg: &DynamicMessage, desc: &MessageDescriptor, type_url: &str) -> String {
    marshal(
        msg,
        desc,
        MarshalOptions {
            type_url: Some(type_url),
            ..Default::default()
        },
    )
}

fn children(msg: &DynamicMessage) -> Vec<DynamicMessage> {
    match msg.get_field_by_name("children").unwrap().into_owned() {
        Value::List(items) => items
            .into_iter()
            .map(|v| match v {
                Value::Message(m) => m,
                other => panic!("{other:?}"),
            })
            .collect(),
        other => panic!("{other:?}"),
    }
}

fn string_field(m: &DynamicMessage, name: &str) -> String {
    match m.get_field_by_name(name).unwrap().into_owned() {
        Value::String(s) => s,
        other => panic!("{name}: {other:?}"),
    }
}

// ---------------- Accept fixtures ----------------

#[test]
fn fixtures_round_trip_through_decode_and_encode() {
    for name in ["roundtrip-keyed.pxf", "roundtrip-quoted.pxf"] {
        let data = read(name);
        let desc = desc_of(&data);
        let type_url = parse(&data).unwrap().type_url;
        let msg = decode(&data, &desc);
        let out = marshal_with_type(&msg, &desc, &type_url);
        assert_eq!(
            out,
            clean(&data),
            "{name}: decode → encode must reproduce the body"
        );
        unmarshal_full(&data, &desc, UnmarshalOptions::default()).expect("full decode agrees");
    }
}

#[test]
fn anonymous_form_is_equivalent_and_canonicalizes_to_keyed() {
    let anon = read("anonymous-equivalence.pxf");
    let keyed = read("roundtrip-keyed.pxf");
    let desc = desc_of(&anon);
    let anon_msg = decode(&anon, &desc);
    let keyed_msg = decode(&keyed, &desc);
    assert_eq!(
        anon_msg.encode_to_vec(),
        keyed_msg.encode_to_vec(),
        "anonymous form must decode to the same message"
    );
    // Encoding the anonymous-form decode canonicalizes to the keyed form.
    let type_url = parse(&anon).unwrap().type_url;
    assert_eq!(
        marshal_with_type(&anon_msg, &desc, &type_url),
        clean(&keyed)
    );
    // fmt canonicalizes the document itself to the keyed form.
    let mut doc = parse(&anon).unwrap();
    canonicalize_keyed(&mut doc, &desc);
    let reparsed = parse(&format(&doc)).expect("canonical output parses");
    assert_eq!(reparsed.entries.len(), 3);
    let Entry::Block(blk) = &reparsed.entries[2] else {
        panic!(
            "children must canonicalize to a keyed block, got {:?}",
            reparsed.entries[2]
        );
    };
    assert_eq!(blk.name, "children");
    assert_eq!(blk.entries.len(), 2);
}

#[test]
fn redundant_agreeing_key_is_legal_and_dropped_by_fmt() {
    let data = read("redundant-key-ok.pxf");
    let desc = desc_of(&data);
    let msg = decode(&data, &desc);
    let kids = children(&msg);
    assert_eq!(kids.len(), 1);
    assert_eq!(string_field(&kids[0], "id"), "greeting");
    // The encoder writes the key once, as the entry name.
    let out = marshal(&msg, &desc, MarshalOptions::default());
    assert_eq!(out.matches("id = ").count(), 1, "{out}");
    // fmt drops the redundant assignment.
    let mut doc = parse(&data).unwrap();
    canonicalize_keyed(&mut doc, &desc);
    let formatted = format(&doc);
    assert_eq!(formatted.matches("id = ").count(), 1, "{formatted}");
}

#[test]
fn anonymous_duplicate_keys_stay_anonymous() {
    let data = read("anonymous-duplicate-ok.pxf");
    let desc = desc_of(&data);
    let msg = decode(&data, &desc);
    let kids = children(&msg);
    assert_eq!(kids.len(), 2);
    assert!(kids.iter().all(|k| string_field(k, "id") == "dup"));
    let out = marshal(&msg, &desc, MarshalOptions::default());
    assert!(
        out.contains("children = ["),
        "duplicate keys must stay anonymous:\n{out}"
    );
    let mut doc = parse(&data).unwrap();
    canonicalize_keyed(&mut doc, &desc);
    assert!(
        format(&doc).contains("children = ["),
        "fmt must keep the anonymous form"
    );
}

// ---------------- Reject fixtures ----------------

#[test]
fn reject_fixtures_are_rejected_with_the_reference_wording() {
    for (name, want) in [
        ("err-duplicate-key.pxf", "duplicate key \"greeting\" in keyed field \"children\""),
        ("err-duplicate-key-spelling.pxf", "duplicate key \"greeting\" in keyed field \"children\""),
        ("err-key-conflict.pxf", "key field \"id\" = \"farewell\" conflicts with entry name \"greeting\" in keyed field \"children\""),
        ("err-empty-key.pxf", "empty entry name in keyed field \"children\": the empty string is not a valid key"),
        ("err-empty-key-anonymous.pxf", "explicit empty-string assignment to key field \"id\" of keyed field \"children\": the empty string is not a valid key"),
        ("err-quoted-name-unkeyed.pxf", "quoted entry name \"a\" is only valid inside a keyed repeated field's block (draft -01 §3.13)"),
    ] {
        let data = read(name);
        // The grammar accepts every one of these; the schema layer rejects.
        parse(&data).unwrap_or_else(|e| panic!("{name} must parse: {e}"));
        let desc = desc_of(&data);
        let err = unmarshal(&data, &desc, UnmarshalOptions::default())
            .expect_err(&format!("{name} must be rejected"));
        assert_eq!(err.msg, want, "{name}");
        let err = unmarshal_full(&data, &desc, UnmarshalOptions::default())
            .expect_err(&format!("{name} must be rejected by the full decode too"));
        assert_eq!(err.msg, want, "{name} (full)");
    }
}

// ---------------- fmt canonicalization pairs ----------------

#[test]
fn fmt_pairs_canonicalize_and_are_fixed_points() {
    for pair in ["fmt-unquote", "fmt-anonymous-to-keyed"] {
        let input = read(&format!("{pair}.pxf"));
        let expected = read(&format!("{pair}.expected.pxf"));
        let desc = desc_of(&input);
        let mut doc = parse(&input).unwrap();
        canonicalize_keyed(&mut doc, &desc);
        assert_eq!(format(&doc), expected, "{pair}");
        let mut fp = parse(&expected).unwrap();
        canonicalize_keyed(&mut fp, &desc);
        assert_eq!(
            format(&fp),
            expected,
            "{pair}: the expected file is a fixed point"
        );
    }
}

// ---------------- Beyond the fixtures ----------------

#[test]
fn descriptor_helpers() {
    let node = pool().get_message_by_name("keyed.v1.Node").unwrap();
    let kf = key_field(&node.get_field_by_name("children").unwrap()).expect("children is keyed");
    assert_eq!(kf.name(), "id");
    assert!(key_field(&node.get_field_by_name("id").unwrap()).is_none());
    let doc = pool().get_message_by_name("keyed.v1.Doc").unwrap();
    assert!(key_field(&doc.get_field_by_name("items").unwrap()).is_none());

    for (name, ok) in [
        ("greeting", true),
        ("user.name", true),
        ("_x1", true),
        ("us-east-1", false),
        ("1abc", false),
        ("", false),
        ("true", false),
        ("null", false),
    ] {
        assert_eq!(ident_safe_entry_name(name), ok, "{name:?}");
    }
}

/// `children = { ... }` spells the keyed block form; a named entry must
/// carry a block value; fmt normalizes the spelling to `children { ... }`.
#[test]
fn assignment_spelling_of_the_keyed_block() {
    let desc = pool().get_message_by_name("keyed.v1.Node").unwrap();
    let doc = "id = \"root\"\nchildren = {\n  greeting { type = \"Label\" }\n  counter_row = { type = \"HBox\" }\n}\n";
    let msg = decode(doc, &desc);
    let kids = children(&msg);
    assert_eq!(kids.len(), 2);
    assert_eq!(string_field(&kids[0], "id"), "greeting");
    assert_eq!(string_field(&kids[1], "id"), "counter_row");

    let err = unmarshal(
        "children { greeting = \"x\" }",
        &desc,
        UnmarshalOptions::default(),
    )
    .expect_err("a named entry needs a block value");
    assert!(err.msg.contains("block value"), "{}", err.msg);

    let mut parsed = parse(doc).unwrap();
    canonicalize_keyed(&mut parsed, &desc);
    let formatted = format(&parsed);
    assert!(formatted.contains("children {\n"), "{formatted}");
    assert!(formatted.contains("  greeting {\n"), "{formatted}");
    assert!(formatted.contains("  counter_row {\n"), "{formatted}");
}

/// A document may bind a keyed field more than once, in either form;
/// elements concatenate in document order — and so do plain repeated
/// fields, which used to be replaced by a second binding.
#[test]
fn bindings_concatenate_in_document_order() {
    let desc = pool().get_message_by_name("keyed.v1.Node").unwrap();
    let doc = "children {\n  a { type = \"Label\" }\n}\nchildren = [ { id = \"b\"\n type = \"HBox\" } ]\nchildren {\n  c { type = \"VBox\" }\n}\n";
    let kids = children(&decode(doc, &desc));
    let ids: Vec<String> = kids.iter().map(|k| string_field(k, "id")).collect();
    assert_eq!(ids, ["a", "b", "c"]);

    let all_types = DescriptorPool::decode(TEST_FDS)
        .unwrap()
        .get_message_by_name("test.v1.AllTypes")
        .unwrap();
    let m = decode(
        "repeated_string = [\"x\"]\nrepeated_string = [\"y\", \"z\"]",
        &all_types,
    );
    assert_eq!(
        m.get_field_by_name("repeated_string").unwrap().into_owned(),
        Value::List(vec![
            Value::String("x".into()),
            Value::String("y".into()),
            Value::String("z".into())
        ])
    );
}

#[test]
fn nested_keyed_blocks_encode_without_repeating_keys() {
    let desc = pool().get_message_by_name("keyed.v1.Node").unwrap();
    let doc = "id = \"root\"\nchildren {\n  outer {\n    children {\n      inner { type = \"Leaf\" }\n    }\n  }\n}\n";
    let msg = decode(doc, &desc);
    let outer = &children(&msg)[0];
    assert_eq!(string_field(outer, "id"), "outer");
    let inner = &children(outer)[0];
    assert_eq!(string_field(inner, "id"), "inner");
    let out = marshal(&msg, &desc, MarshalOptions::default());
    assert!(!out.contains("id = \"outer\""), "{out}");
    assert!(!out.contains("id = \"inner\""), "{out}");
    assert!(out.contains("inner {"), "{out}");
}

/// The keyed form is emitted only when every key is present, non-empty and
/// distinct; otherwise the anonymous list form, which can represent those.
#[test]
fn encoder_falls_back_to_anonymous_form_when_ineligible() {
    let desc = pool().get_message_by_name("keyed.v1.Node").unwrap();
    let node_desc = desc.clone();
    let mk = |id: &str| {
        let mut m = DynamicMessage::new(node_desc.clone());
        if !id.is_empty() {
            m.set_field_by_name("id", Value::String(id.into()));
        }
        m.set_field_by_name("type", Value::String("T".into()));
        Value::Message(m)
    };
    let mut root = DynamicMessage::new(desc.clone());
    root.set_field_by_name("children", Value::List(vec![mk("a"), mk("")]));
    let out = marshal(&root, &desc, MarshalOptions::default());
    assert!(out.contains("children = ["), "absent key:\n{out}");
    root.set_field_by_name("children", Value::List(vec![mk("a"), mk("a")]));
    let out = marshal(&root, &desc, MarshalOptions::default());
    assert!(out.contains("children = ["), "duplicate key:\n{out}");
    root.set_field_by_name("children", Value::List(vec![mk("a"), mk("b")]));
    let out = marshal(&root, &desc, MarshalOptions::default());
    assert_eq!(
        out,
        "children {\n  a {\n    type = \"T\"\n  }\n  b {\n    type = \"T\"\n  }\n}\n"
    );
}

/// The grammar side: a quoted entry name parses everywhere (with `{` or
/// `= { }`), the AST records the quoting, and the schema-less formatter
/// reproduces it byte for byte.
#[test]
fn quoted_entry_name_grammar_and_fidelity() {
    let input = "regions {\n  \"us-east-1\" {\n    replicas = 3\n  }\n}\n";
    let doc = parse(input).unwrap();
    let Entry::Block(regions) = &doc.entries[0] else {
        panic!()
    };
    let Entry::Block(entry) = &regions.entries[0] else {
        panic!()
    };
    assert_eq!(entry.name, "us-east-1");
    assert!(entry.name_quoted);
    assert_eq!(format(&doc), input);

    let doc = parse("regions {\n  \"us-east-1\" = { replicas = 3 }\n}\n").unwrap();
    let Entry::Block(regions) = &doc.entries[0] else {
        panic!()
    };
    let Entry::Assignment(a) = &regions.entries[0] else {
        panic!()
    };
    assert_eq!(a.key, "us-east-1");
    assert!(a.key_quoted);

    // Integer and bool keys still take only the ':' tail.
    let err = parse("m {\n  1 = \"x\"\n}").unwrap_err();
    assert!(
        err.msg.contains("requires an identifier or string key"),
        "{}",
        err.msg
    );
}
