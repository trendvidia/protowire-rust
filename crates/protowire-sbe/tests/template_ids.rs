// SPDX-License-Identifier: MIT
// Copyright (c) 2026 TrendVidia, LLC.
//! A template id is unique within a schema, not across schemas (#121, draft
//! -01 § SBE Annotations). Ports protowire-go's
//! `encoding/sbe/template_id_test.go`: the codec refuses two messages of one
//! schema with one id, `view` finds a template by schema id and template id
//! together, and `unmarshal` refuses a header naming another schema.
//!
//! The fixtures are `testdata/template-ids-*.proto`, one schema per file, in
//! the descriptor set `template-ids.binpb`.

use prost_reflect::{
    DescriptorPool, DynamicMessage, FileDescriptor, MessageDescriptor, ReflectMessage, Value,
};
use protowire_sbe::{marshal, unmarshal, Codec};

const FDS: &[u8] = include_bytes!("../testdata/template-ids.binpb");

fn pool() -> DescriptorPool {
    DescriptorPool::decode(FDS).expect("decode template-ids.binpb")
}

fn file(pool: &DescriptorPool, suffix: &str) -> FileDescriptor {
    let name = format!("template-ids-{suffix}.proto");
    pool.get_file_by_name(&name)
        .unwrap_or_else(|| panic!("{name} in pool"))
}

fn files(suffixes: &[&str]) -> Vec<FileDescriptor> {
    let p = pool();
    suffixes.iter().map(|s| file(&p, s)).collect()
}

fn message(name: &str) -> MessageDescriptor {
    pool()
        .get_message_by_name(name)
        .unwrap_or_else(|| panic!("missing {name}"))
}

fn with(name: &str, field: &str, value: Value) -> DynamicMessage {
    let mut msg = DynamicMessage::new(message(name));
    let fd = msg.descriptor().get_field_by_name(field).expect("field");
    msg.set_field(&fd, value);
    msg
}

fn le16(b: &[u8]) -> u16 {
    u16::from_le_bytes([b[0], b[1]])
}

/// Go's TestTemplateIDUniqueInSchema. Before the fix the codec bound both
/// messages and the second replaced the first in the id lookup.
#[test]
fn template_id_unique_in_schema() {
    let cases: &[(&str, &[&str], Option<&str>)] = &[
        (
            "two messages in one file",
            &["dup-file"],
            Some("sbe: messages ids.dupfile.A and ids.dupfile.B share (sbe.template_id) = 7 in schema 1; it must be unique within a schema"),
        ),
        (
            "two files of one schema",
            &["dup-e", "dup-f"],
            Some("sbe: messages ids.dupe.A and ids.dupf.B share (sbe.template_id) = 7 in schema 1; it must be unique within a schema"),
        ),
        (
            "a nested message",
            &["dup-nested"],
            Some("sbe: messages ids.dupnested.A and ids.dupnested.A.N share (sbe.template_id) = 7 in schema 1; it must be unique within a schema"),
        ),
        ("distinct ids", &["distinct"], None),
    ];
    let mut checked = 0;
    for (name, suffixes, reject) in cases {
        let got = Codec::from_files(&files(suffixes));
        match reject {
            Some(want) => {
                let err = got.expect_err(name);
                assert_eq!(err.to_string(), *want, "{name}");
            }
            None => {
                got.unwrap_or_else(|e| panic!("{name}: {e}"));
            }
        }
        checked += 1;
    }
    assert_eq!(checked, 4);
}

/// Go's TestTemplateIDSameFileTwice: a file passed twice registers the same
/// messages again, which is not a duplicate.
#[test]
fn template_id_same_file_twice() {
    let fa = files(&["a"]);
    let codec = Codec::from_files(&[fa[0].clone(), fa[0].clone()]).expect("same file twice");
    let a = with("ids.a.A", "x", Value::U32(42));
    let data = marshal(&codec, &a).expect("marshal");
    let v = codec.view(&data).expect("view");
    assert_eq!(v.uint_field("x").unwrap(), 42);
}

/// Go's TestTemplateIDAcrossSchemas: two schemas may each use template 7,
/// and `view` picks the template from the header's schema id and template
/// id together, whichever file was registered last.
#[test]
fn template_id_across_schemas() {
    let fds = files(&["a", "b"]);
    let a = with("ids.a.A", "x", Value::U32(0xA));
    let b = with("ids.b.B", "y", Value::U64(0xB));

    let mut orders = 0;
    for order in [
        vec![fds[0].clone(), fds[1].clone()],
        vec![fds[1].clone(), fds[0].clone()],
    ] {
        let codec = Codec::from_files(&order).expect("two schemas may each use template 7");
        let data_a = marshal(&codec, &a).expect("marshal A");
        let data_b = marshal(&codec, &b).expect("marshal B");
        assert_eq!(le16(&data_a[4..]), 1, "A's header names schema 1");
        assert_eq!(le16(&data_b[4..]), 2, "B's header names schema 2");

        let v = codec.view(&data_a).expect("view A");
        assert_eq!(v.uint_field("x").unwrap(), 0xA);
        let v = codec.view(&data_b).expect("view B");
        assert_eq!(v.uint_field("y").unwrap(), 0xB);
        orders += 1;
    }
    assert_eq!(orders, 2);

    // Template 9 exists in schema 1 only. A header naming it in schema 2 is
    // refused, where an id-only lookup would have found it.
    let codec = Codec::from_files(&fds).expect("codec");
    let mut data9 =
        marshal(&codec, &DynamicMessage::new(message("ids.a.A.Only1"))).expect("marshal Only1");
    codec.view(&data9).expect("view Only1 in schema 1");
    data9[4] = 2;
    let err = codec.view(&data9).expect_err("template 9 in schema 2");
    assert_eq!(err.to_string(), "sbe: unknown template ID 9 in schema 2");
}

/// Go's TestUnmarshalChecksSchemaID: `unmarshal` refuses bytes whose header
/// names another schema than the target message's, even when the template id
/// matches. A's block is no shorter than B's, so before the check A's bytes
/// decoded as B with no error, B.y holding the low half of A.x.
#[test]
fn unmarshal_checks_schema_id() {
    let codec = Codec::from_files(&files(&["c", "d"])).expect("codec");
    let a = with("ids.c.A", "x", Value::U64(0xA_0000_000A));
    let mut data_a = marshal(&codec, &a).expect("marshal A");

    let want = "sbe: schema ID mismatch: got 1, want 2 for template 7";
    let mut b = DynamicMessage::new(message("ids.d.B"));
    let err = unmarshal(&codec, &mut b, &data_a).expect_err("A's bytes as B");
    assert_eq!(err.to_string(), want);

    // Its own message still reads it, and so does B once the header names
    // schema 2: the check is the header's schema id, not the layout.
    let mut got = DynamicMessage::new(message("ids.c.A"));
    unmarshal(&codec, &mut got, &data_a).expect("A's bytes as A");
    assert_eq!(
        got.get_field_by_name("x").unwrap().as_u64(),
        Some(0xA_0000_000A)
    );
    data_a[4] = 2;
    let mut got = DynamicMessage::new(message("ids.d.B"));
    unmarshal(&codec, &mut got, &data_a).expect("schema 2 header as B");
    assert_eq!(got.get_field_by_name("y").unwrap().as_u32(), Some(0xA));

    // A header with schema id 0, as an emitter that does not set it writes,
    // is refused for a schema that declares another id.
    data_a[4] = 0;
    let mut got = DynamicMessage::new(message("ids.c.A"));
    let err = unmarshal(&codec, &mut got, &data_a).expect_err("schema 0");
    assert_eq!(
        err.to_string(),
        "sbe: schema ID mismatch: got 0, want 1 for template 7"
    );
}
