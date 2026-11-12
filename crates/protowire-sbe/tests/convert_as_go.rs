// SPDX-License-Identifier: MIT
// Copyright (c) 2026 TrendVidia, LLC.
//! The SBE XML converter, held to protowire-go's (#104). Every golden under
//! `testdata/convert/` is what protowire-go's converter wrote
//! (`scripts/gen-sbe-convert.sh`), and this port's must write the same bytes.
//!
//! Draft -01 § Fixed-Length Arrays: a `uint8` or `int8` array of length N
//! (N > 1) imports as `bytes` with `(sbe.length) = N`, an array of a wider
//! primitive is refused, and a `bytes` field exports as a `uint8` array, so a
//! schema survives XML → proto → XML → proto. Ports protowire-go's
//! `TestXMLRoundTripFixedArrays`, `TestXMLToProtoOctetArray`,
//! `TestXMLToProtoRefusesWideArray` and `TestXMLRoundTripBytesLengthOne`.

use prost_reflect::{DescriptorPool, DynamicMessage, FileDescriptor, Value};
use protowire_sbe::{marshal, proto_to_xml, unmarshal, xml_to_proto, Codec};

macro_rules! golden {
    ($name:literal) => {
        include_str!(concat!("../testdata/convert/", $name))
    };
}

const FDS: &[u8] = include_bytes!("../testdata/convert/convert.binpb");

fn file(name: &str) -> FileDescriptor {
    let pool = DescriptorPool::decode(FDS).expect("decode convert.binpb");
    pool.get_file_by_name(name)
        .unwrap_or_else(|| panic!("{name} in convert.binpb"))
}

/// Drops the comment lines XMLToProto writes above a field: they are all
/// the export does not carry back.
fn without_notes(proto: &str) -> String {
    proto
        .lines()
        .filter(|l| !l.trim_start().starts_with("//"))
        .map(|l| format!("{l}\n"))
        .collect()
}

/// Go's TestXMLToProtoOctetArray and TestXMLRoundTripFixedArrays: the
/// import, its export and the export's import are protowire-go's, byte for
/// byte, and the second import declares the fields the first one does.
#[test]
fn fixed_arrays_round_trip_as_go() {
    let proto = xml_to_proto(golden!("fixed-arrays.xml")).expect("xml_to_proto");
    assert_eq!(proto, golden!("fixed-arrays.proto"));
    for line in [
        "bytes uuid = 2 [(sbe.length) = 16];",
        "bytes signed = 3 [(sbe.length) = 3];",
        "bytes raw = 1 [(sbe.length) = 6];",
        "uint32 tail = 5;",
    ] {
        assert!(proto.contains(line), "missing {line:?}");
    }

    let xml = proto_to_xml(&file("convert/fixed-arrays.proto")).expect("proto_to_xml");
    assert_eq!(xml, golden!("fixed-arrays.export.xml"));
    for line in [
        r#"<type name="bytes16" primitiveType="uint8" length="16"/>"#,
        r#"<field name="name" id="1" type="bytes4"/>"#,
        r#"<type name="raw" primitiveType="uint8" length="6"/>"#,
    ] {
        assert!(xml.contains(line), "missing {line:?}");
    }

    let again = xml_to_proto(&xml).expect("xml_to_proto of the export");
    assert_eq!(again, golden!("fixed-arrays.again.txt"));
    // The same field types and options, so the same descriptors and the same
    // bytes on the wire; only the characterEncoding note is gone.
    assert_eq!(again, without_notes(&proto));
}

/// Go's TestXMLToProtoOctetArray: each array is its N bytes in the block,
/// and reads back.
#[test]
fn octet_arrays_are_their_bytes_on_the_wire() {
    let fd = file("convert/fixed-arrays.proto");
    let codec = Codec::from_files(std::slice::from_ref(&fd)).expect("codec");
    let desc = fd
        .parent_pool()
        .get_message_by_name("arr.v1.Record")
        .unwrap();
    let key_desc = fd.parent_pool().get_message_by_name("arr.v1.Key").unwrap();

    let uuid: Vec<u8> = (0..15).chain([0xFF]).collect();
    let mut msg = DynamicMessage::new(desc.clone());
    msg.set_field_by_name("name", Value::Bytes(vec![b'c', b'a', b'f', 0xE9].into()));
    msg.set_field_by_name("uuid", Value::Bytes(uuid.clone().into()));
    msg.set_field_by_name("signed", Value::Bytes(vec![0x80, 0x7F, 0x01].into()));
    let mut key = DynamicMessage::new(key_desc);
    key.set_field_by_name("raw", Value::Bytes(b"abcdef".to_vec().into()));
    key.set_field_by_name("seq", Value::U32(7));
    msg.set_field_by_name("key", Value::Message(key));
    msg.set_field_by_name("tail", Value::U32(0xA1B2C3D4));
    let data = marshal(&codec, &msg).expect("marshal");

    // Header(8) + name(4) + uuid(16) + signed(3) + key(6+4) + tail(4).
    assert_eq!(data.len(), 8 + 4 + 16 + 3 + 10 + 4);
    assert_eq!(&data[8..12], &[b'c', b'a', b'f', 0xE9]);
    assert_eq!(&data[12..28], &uuid[..]);
    assert_eq!(&data[28..31], &[0x80, 0x7F, 0x01]);
    assert_eq!(&data[31..37], b"abcdef");
    assert_eq!(&data[37..41], &7u32.to_le_bytes());
    assert_eq!(&data[41..], &[0xD4, 0xC3, 0xB2, 0xA1]);

    let mut got = DynamicMessage::new(desc);
    unmarshal(&codec, &mut got, &data).expect("unmarshal");
    assert_eq!(got, msg);
}

/// Go's TestXMLRoundTripBytesLengthOne: a bytes field of length 1 exports
/// as one uint8 and imports back as a uint32 narrowed to uint8.
#[test]
fn bytes_of_length_one_round_trip_as_go() {
    let xml = proto_to_xml(&file("convert/bytes-one.proto")).expect("proto_to_xml");
    assert_eq!(xml, golden!("bytes-one.export.xml"));
    assert!(xml.contains(r#"<type name="bytes1" primitiveType="uint8" length="1"/>"#));
    let again = xml_to_proto(&xml).expect("xml_to_proto");
    assert_eq!(again, golden!("bytes-one.again.txt"));
    assert!(again.contains(r#"uint32 b = 1 [(sbe.encoding) = "uint8"];"#));
}

/// Go's TestXMLToProtoRefusesWideArray: an array of a primitive wider than
/// a byte is refused with protowire-go's text, at a field and in a composite.
#[test]
fn wide_arrays_are_refused_as_go() {
    let cases = [
        (golden!("wide-field.xml"), golden!("wide-field.err")),
        (golden!("wide-composite.xml"), golden!("wide-composite.err")),
        (golden!("wide-uuid.xml"), golden!("wide-uuid.err")),
    ];
    for (xml, want) in cases {
        let err = xml_to_proto(xml).expect_err("a wide array");
        assert_eq!(format!("ERR {err}\n"), want);
        assert!(want.contains("which no proto field represents"));
    }
}

/// The export of this crate's other two fixtures is protowire-go's too.
#[test]
fn fixtures_export_as_go() {
    for (set, name, want) in [
        (
            &include_bytes!("../testdata/sbe-test.binpb")[..],
            "sbe-test.proto",
            golden!("sbe-test.export.xml"),
        ),
        (
            &include_bytes!("../testdata/char-encoding.binpb")[..],
            "char-encoding.proto",
            golden!("char-encoding.export.xml"),
        ),
    ] {
        let pool = DescriptorPool::decode(set).expect("decode");
        let fd = pool.get_file_by_name(name).expect(name);
        assert_eq!(proto_to_xml(&fd).expect(name), want, "{name}");
    }
}
