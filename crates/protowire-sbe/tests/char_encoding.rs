// SPDX-License-Identifier: MIT
// Copyright (c) 2026 TrendVidia, LLC.
//! Draft -01 § Character Encoding (#94, trendvidia/protowire#494): the SBE
//! XML importer maps a `char` type to a proto `string` only when its
//! `characterEncoding` is UTF-8 compatible: absent (FIX SBE's default is
//! US-ASCII), a US-ASCII name, or a UTF-8 name. Anything else is `bytes`
//! with the same `(sbe.length)`, so its text is carried rather than refused
//! at run time. Mirrors protowire-go's `TestXMLToProtoCharacterEncoding`
//! (protowire-go#248).

use prost_reflect::{DescriptorPool, DynamicMessage, Value};
use protowire_sbe::{marshal, unmarshal, xml_to_proto, Codec};

const XML: &str = include_str!("../testdata/char-encoding.xml");
/// `xml_to_proto(XML)`, kept so it can be compiled (`gen-testdata.sh`).
const PROTO: &str = include_str!("../testdata/char-encoding.proto");
const FDS: &[u8] = include_bytes!("../testdata/char-encoding.binpb");

#[test]
fn a_char_in_a_non_utf8_encoding_is_bytes() {
    let proto = xml_to_proto(XML).expect("convert");
    for line in [
        "string plain = 1 [(sbe.length) = 4];",
        "string ascii = 2 [(sbe.length) = 4];",
        "string us_ascii = 3 [(sbe.length) = 4];",
        "string utf8 = 4 [(sbe.length) = 4];",
        "  // SBE characterEncoding \"ISO-8859-1\" is not UTF-8 compatible, so this field is bytes.\n  bytes latin1 = 5 [(sbe.length) = 4];",
        "  // SBE characterEncoding \"windows-1252\" is not UTF-8 compatible, so this field is bytes.\n  bytes code = 1 [(sbe.length) = 2];",
        "string tag = 2 [(sbe.length) = 2];",
    ] {
        assert!(proto.contains(line), "missing {line:?} in\n{proto}");
    }
    // The compiled fixture is this output.
    assert_eq!(
        proto, PROTO,
        "regenerate testdata/char-encoding.proto and .binpb"
    );
}

#[test]
fn the_bytes_field_carries_latin1() {
    let pool = DescriptorPool::decode(FDS).expect("decode char-encoding.binpb");
    let file = pool
        .files()
        .find(|f| f.package_name() == "enc.v1")
        .expect("enc.v1 file");
    let codec = Codec::from_files(&[file]).expect("codec");
    let desc = pool.get_message_by_name("enc.v1.Text").unwrap();

    // Latin-1 "café": E9 is not UTF-8, and the bytes field carries it.
    let latin1 = vec![b'c', b'a', b'f', 0xE9];
    let mut msg = DynamicMessage::new(desc.clone());
    msg.set_field_by_name("latin1", Value::Bytes(latin1.clone().into()));
    let data = marshal(&codec, &msg).expect("marshal");
    let mut got = DynamicMessage::new(desc);
    unmarshal(&codec, &mut got, &data).expect("unmarshal");
    assert_eq!(
        got.get_field_by_name("latin1")
            .unwrap()
            .as_bytes()
            .unwrap()
            .as_ref(),
        &latin1[..]
    );
}
