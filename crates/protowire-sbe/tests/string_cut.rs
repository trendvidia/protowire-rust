// SPDX-License-Identifier: MIT
// Copyright (c) 2026 TrendVidia, LLC.
//! Draft -01 § SBE Annotations (trendvidia/protowire#495): an overlong
//! `(sbe.length)` string is cut at the last whole UTF-8 character at or
//! before the limit and NUL-padded, so it still decodes; a `bytes` field is
//! cut at the limit. Mirrors protowire-go's
//! `TestOverlongStringCutOnCharacterBoundary`.

use prost_reflect::{DescriptorPool, DynamicMessage, MessageDescriptor, Value};
use protowire_sbe::{marshal, unmarshal, Codec};

const SBE_FDS: &[u8] = include_bytes!("../testdata/sbe-test.binpb");

fn pool() -> DescriptorPool {
    DescriptorPool::decode(SBE_FDS).expect("decode sbe-test.binpb")
}

fn codec() -> Codec {
    let file = pool().get_file_by_name("sbe-test.proto").unwrap();
    Codec::from_files(&[file]).expect("build codec")
}

fn desc_of(name: &str) -> MessageDescriptor {
    pool().get_message_by_name(name).unwrap()
}

fn with(desc: &MessageDescriptor, name: &str, value: Value) -> DynamicMessage {
    let mut msg = DynamicMessage::new(desc.clone());
    msg.set_field_by_name(name, value);
    msg
}

fn marshal_cut(codec: &Codec, text: &str) -> Vec<u8> {
    let s = || Value::String(text.to_string());
    let mut msg = with(&desc_of("test.v1.Cut"), "text", s());
    msg.set_field_by_name("raw", Value::Bytes(text.as_bytes().to_vec().into()));
    msg.set_field_by_name(
        "code",
        Value::Message(with(&desc_of("test.v1.CutCode"), "text", s())),
    );
    msg.set_field_by_name(
        "rows",
        Value::List(vec![Value::Message(with(
            &desc_of("test.v1.CutRow"),
            "tag",
            s(),
        ))]),
    );
    marshal(codec, &msg).expect("marshal")
}

fn text_of(msg: &DynamicMessage, name: &str) -> String {
    match msg.get_field_by_name(name).unwrap().into_owned() {
        Value::String(s) => s,
        other => panic!("{name}: {other:?}"),
    }
}

// Header 8; block: text 0-8, raw 8-16, code.text 16-24; group header 4;
// the one row's tag 28-36.
const BLOCK: usize = 8;
const SITES: [(&str, usize); 3] = [
    ("root field", BLOCK),
    ("composite field", BLOCK + 16),
    ("group field", BLOCK + 28),
];

#[test]
fn overlong_string_cut_on_character_boundary() {
    let codec = codec();
    let cases = [
        ("2-byte character across the limit", "ABCDEFGé", "ABCDEFG"),
        ("3-byte character across the limit", "ABCDEF€", "ABCDEF"),
        ("3-byte character, one byte over", "ABCDE€x", "ABCDE€"),
        ("4-byte character across the limit", "ABCDE😀", "ABCDE"),
        ("limit between two 4-byte characters", "😀😀😀", "😀😀"),
        ("cut already on a boundary", "ABCDEFGHI", "ABCDEFGH"),
        ("fits exactly", "ABCDEFé", "ABCDEFé"),
    ];
    for (name, input, text) in cases {
        let frame = marshal_cut(&codec, input);
        let mut want = text.as_bytes().to_vec();
        want.resize(8, 0);
        for (site, at) in SITES {
            assert_eq!(&frame[at..at + 8], &want[..], "{name}: {site}");
        }
        assert_eq!(
            &frame[BLOCK + 8..BLOCK + 16],
            &input.as_bytes()[..8],
            "{name}: bytes field"
        );

        let mut got = DynamicMessage::new(desc_of("test.v1.Cut"));
        unmarshal(&codec, &mut got, &frame).unwrap_or_else(|e| panic!("{name}: {e}"));
        assert_eq!(text_of(&got, "text"), text, "{name}");
    }

    // The case the issue names, byte for byte.
    let frame = marshal_cut(&codec, "ABCDEFGé");
    assert_eq!(
        &frame[BLOCK..BLOCK + 8],
        &[0x41, 0x42, 0x43, 0x44, 0x45, 0x46, 0x47, 0x00]
    );
    assert_eq!(
        &frame[BLOCK + 8..BLOCK + 16],
        &[0x41, 0x42, 0x43, 0x44, 0x45, 0x46, 0x47, 0xC3]
    );
}

#[test]
fn limit_shorter_than_first_character_leaves_field_empty() {
    let codec = codec();
    let tiny = desc_of("test.v1.CutTiny");
    let frame = marshal(&codec, &with(&tiny, "text", Value::String("😀".into()))).unwrap();
    assert_eq!(&frame[BLOCK..BLOCK + 3], &[0, 0, 0]);
    let mut got = DynamicMessage::new(tiny);
    unmarshal(&codec, &mut got, &frame).expect("unmarshal");
    assert_eq!(text_of(&got, "text"), "");
}
