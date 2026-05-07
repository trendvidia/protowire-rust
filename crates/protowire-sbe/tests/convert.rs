// SPDX-License-Identifier: MIT
// Copyright (c) 2026 TrendVidia, LLC.
//! SBE D2/D3/D4 + name-conversion tests. Mirrors the TS port's
//! `src/sbe/convert.test.ts`.

use prost_reflect::DescriptorPool;
use protowire_sbe::{
    camel_to_screaming_snake, camel_to_snake, parse_xml_schema, proto_to_xml,
    screaming_snake_to_pascal, singular_pascal, snake_to_camel, strip_enum_prefix, xml_to_proto,
};

const SBE_FDS: &[u8] = include_bytes!("../testdata/sbe-test.binpb");

const TEST_XML: &str = r#"<?xml version="1.0" encoding="UTF-8"?>
<sbe:messageSchema xmlns:sbe="http://fixprotocol.io/2016/sbe"
                   package="test.v1"
                   id="1"
                   version="0"
                   byteOrder="littleEndian">
    <types>
        <composite name="messageHeader">
            <type name="blockLength" primitiveType="uint16"/>
            <type name="templateId" primitiveType="uint16"/>
            <type name="schemaId" primitiveType="uint16"/>
            <type name="version" primitiveType="uint16"/>
        </composite>
        <composite name="groupSizeEncoding">
            <type name="blockLength" primitiveType="uint16"/>
            <type name="numInGroup" primitiveType="uint16"/>
        </composite>
        <enum name="Side" encodingType="uint8">
            <validValue name="Buy">0</validValue>
            <validValue name="Sell">1</validValue>
        </enum>
        <type name="str8" primitiveType="char" length="8"/>
        <composite name="Inner">
            <type name="x" primitiveType="int64"/>
            <type name="y" primitiveType="int64"/>
        </composite>
    </types>
    <sbe:message name="Order" id="1">
        <field name="orderId" id="1" type="uint64"/>
        <field name="symbol" id="2" type="str8"/>
        <field name="price" id="3" type="int64"/>
        <field name="quantity" id="4" type="uint32"/>
        <field name="side" id="5" type="Side"/>
        <field name="active" id="6" type="uint8"/>
        <field name="weight" id="7" type="double"/>
        <field name="score" id="8" type="float"/>
        <group name="fills" id="9">
            <field name="fillPrice" id="1" type="int64"/>
            <field name="fillQty" id="2" type="uint32"/>
            <field name="fillId" id="3" type="uint64"/>
        </group>
    </sbe:message>
    <sbe:message name="Simple" id="2">
        <field name="id" id="1" type="uint32"/>
        <field name="value" id="2" type="int32"/>
    </sbe:message>
    <sbe:message name="WithComposite" id="3">
        <field name="id" id="1" type="uint64"/>
        <field name="inner" id="2" type="Inner"/>
        <field name="code" id="3" type="int32"/>
    </sbe:message>
    <sbe:message name="WithNarrow" id="4">
        <field name="status" id="1" type="uint8"/>
        <field name="port" id="2" type="uint16"/>
        <field name="delta" id="3" type="int16"/>
    </sbe:message>
</sbe:messageSchema>"#;

// ---------------- parseXMLSchema ----------------

#[test]
fn parses_package_id_version_types_and_messages() {
    let schema = parse_xml_schema(TEST_XML).expect("parse");
    assert_eq!(schema.package, "test.v1");
    assert_eq!(schema.id, 1);
    assert_eq!(schema.version, 0);
    assert_eq!(schema.types.enums.len(), 1);
    assert_eq!(schema.types.enums[0].name, "Side");
    let vals: Vec<(String, String)> = schema.types.enums[0]
        .valid_values
        .iter()
        .map(|v| (v.name.clone(), v.value.clone()))
        .collect();
    assert_eq!(
        vals,
        vec![
            ("Buy".to_string(), "0".to_string()),
            ("Sell".to_string(), "1".to_string()),
        ]
    );
    assert_eq!(schema.messages.len(), 4);

    let order = schema
        .messages
        .iter()
        .find(|m| m.name == "Order")
        .expect("Order message");
    assert_eq!(order.id, 1);
    assert_eq!(order.fields.len(), 8);
    assert_eq!(order.groups.len(), 1);
    assert_eq!(order.groups[0].name, "fills");
}

#[test]
fn parses_xml_without_namespace_prefix() {
    let xml = TEST_XML
        .replace("sbe:message", "message")
        .replace("sbe:messageSchema", "messageSchema");
    let schema = parse_xml_schema(&xml).expect("parse");
    assert_eq!(schema.package, "test.v1");
    assert_eq!(schema.messages.len(), 4);
}

// ---------------- xmlToProto ----------------

#[test]
fn xml_to_proto_emits_expected_proto_fragments() {
    let proto = xml_to_proto(TEST_XML).expect("convert");
    assert!(proto.contains("option (sbe.schema_id) = 1;"), "{proto}");
    assert!(proto.contains("option (sbe.version) = 0;"), "{proto}");
    assert!(proto.contains("option (sbe.template_id) = 1;"), "{proto}");
    assert!(
        proto.contains("string symbol = 2 [(sbe.length) = 8];"),
        "{proto}"
    );
    assert!(proto.contains("Side side = 5;"), "{proto}");
    assert!(proto.contains("Inner inner = 2;"), "{proto}");
    assert!(proto.contains("repeated Fill fills = 9;"), "{proto}");
    assert!(proto.contains(r#"(sbe.encoding) = "uint8""#), "{proto}");
}

#[test]
fn xml_to_proto_converts_camel_case_to_snake_case() {
    let proto = xml_to_proto(TEST_XML).expect("convert");
    assert!(proto.contains("uint64 order_id = 1;"), "{proto}");
    assert!(proto.contains("int64 fill_price = 1;"), "{proto}");
    assert!(proto.contains("uint32 fill_qty = 2;"), "{proto}");
}

#[test]
fn xml_to_proto_singularizes_group_names_to_pascal() {
    let proto = xml_to_proto(TEST_XML).expect("convert");
    assert!(proto.contains("message Fill {"), "{proto}");
    assert!(proto.contains("repeated Fill fills = 9;"), "{proto}");
}

#[test]
fn xml_to_proto_re_prefixes_enum_value_names() {
    let proto = xml_to_proto(TEST_XML).expect("convert");
    assert!(proto.contains("SIDE_BUY = 0;"), "{proto}");
    assert!(proto.contains("SIDE_SELL = 1;"), "{proto}");
}

// ---------------- protoToXml ----------------

fn sbe_test_file() -> prost_reflect::FileDescriptor {
    let pool = DescriptorPool::decode(SBE_FDS).expect("decode sbe-test.binpb");
    pool.get_file_by_name("sbe-test.proto")
        .expect("sbe-test.proto in pool")
}

#[test]
fn proto_to_xml_emits_schema_header_and_message_sections() {
    let xml = proto_to_xml(&sbe_test_file()).expect("convert");
    assert!(xml.contains(r#"package="test.v1""#), "{xml}");
    assert!(xml.contains(r#"id="1""#), "{xml}");
    assert!(
        xml.contains(r#"<sbe:message name="Order" id="1">"#),
        "{xml}"
    );
    assert!(
        xml.contains(r#"<sbe:message name="Simple" id="2">"#),
        "{xml}"
    );
    assert!(xml.contains(r#"<enum name="Side""#), "{xml}");
    assert!(xml.contains(r#"<composite name="Inner">"#), "{xml}");
    assert!(xml.contains(r#"<group name="fills""#), "{xml}");
}

#[test]
fn proto_to_xml_strips_proto_enum_prefix_from_valid_values() {
    let xml = proto_to_xml(&sbe_test_file()).expect("convert");
    assert!(
        xml.contains(r#"<validValue name="Buy">0</validValue>"#),
        "{xml}"
    );
    assert!(
        xml.contains(r#"<validValue name="Sell">1</validValue>"#),
        "{xml}"
    );
}

#[test]
fn proto_to_xml_output_round_trips_through_parse_xml_schema() {
    let xml = proto_to_xml(&sbe_test_file()).expect("convert");
    let schema = parse_xml_schema(&xml).expect("parse");
    assert_eq!(schema.package, "test.v1");
    assert_eq!(schema.id, 1);
    assert_eq!(schema.messages.len(), 4);
    let order = schema
        .messages
        .iter()
        .find(|m| m.name == "Order")
        .expect("Order");
    assert_eq!(order.id, 1);
    assert_eq!(order.groups.len(), 1);
    assert_eq!(order.groups[0].name, "fills");
}

#[test]
fn proto_to_xml_round_trips_through_xml_to_proto() {
    let xml = proto_to_xml(&sbe_test_file()).expect("convert");
    let proto_src = xml_to_proto(&xml).expect("xml_to_proto");
    assert!(
        proto_src.contains("option (sbe.schema_id) = 1;"),
        "{proto_src}"
    );
    assert!(
        proto_src.contains("option (sbe.template_id) = 1;"),
        "{proto_src}"
    );
    assert!(
        proto_src.contains("option (sbe.template_id) = 2;"),
        "{proto_src}"
    );
    assert!(
        proto_src.contains("option (sbe.template_id) = 3;"),
        "{proto_src}"
    );
    assert!(
        proto_src.contains("option (sbe.template_id) = 4;"),
        "{proto_src}"
    );
}

// ---------------- name conversions ----------------

#[test]
fn camel_to_snake_handles_examples_from_go_suite() {
    assert_eq!(camel_to_snake("orderId"), "order_id");
    assert_eq!(camel_to_snake("fillPrice"), "fill_price");
    assert_eq!(camel_to_snake("id"), "id");
    assert_eq!(camel_to_snake("x"), "x");
    assert_eq!(camel_to_snake("orderID"), "order_id");
}

#[test]
fn snake_to_camel_handles_examples_from_go_suite() {
    assert_eq!(snake_to_camel("order_id"), "orderId");
    assert_eq!(snake_to_camel("fill_price"), "fillPrice");
    assert_eq!(snake_to_camel("id"), "id");
}

#[test]
fn camel_to_screaming_snake_adds_underscores_at_lower_to_upper_boundaries() {
    assert_eq!(camel_to_screaming_snake("Side"), "SIDE");
    assert_eq!(camel_to_screaming_snake("OrderType"), "ORDER_TYPE");
}

#[test]
fn screaming_snake_to_pascal_collapses_underscores() {
    assert_eq!(screaming_snake_to_pascal("BUY"), "Buy");
    assert_eq!(screaming_snake_to_pascal("ORDER_TYPE"), "OrderType");
}

#[test]
fn strip_enum_prefix_peels_screaming_snake_prefix() {
    assert_eq!(strip_enum_prefix("SIDE_BUY", "Side"), "Buy");
    assert_eq!(strip_enum_prefix("SIDE_SELL", "Side"), "Sell");
    assert_eq!(strip_enum_prefix("OTHER", "Side"), "Other");
}

#[test]
fn singular_pascal_handles_common_plural_endings() {
    assert_eq!(singular_pascal("fills"), "Fill");
    assert_eq!(singular_pascal("orders"), "Order");
    assert_eq!(singular_pascal("entries"), "Entry");
    assert_eq!(singular_pascal("class"), "Class");
}
