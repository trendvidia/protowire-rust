//! Slice C tests for the SBE [`View`] / [`GroupView`] zero-copy reader.
//! Mirrors the TS port's `src/sbe/view.test.ts`.

use prost_reflect::{DescriptorPool, DynamicMessage, MessageDescriptor, ReflectMessage, Value};
use protowire_sbe::{marshal, Codec};

const SBE_FDS: &[u8] = include_bytes!("../testdata/sbe-test.binpb");

fn pool() -> DescriptorPool {
    DescriptorPool::decode(SBE_FDS).expect("decode sbe-test.binpb")
}

fn codec() -> Codec {
    let p = pool();
    let file = p
        .get_file_by_name("sbe-test.proto")
        .expect("sbe-test.proto in pool");
    Codec::from_files(&[file]).expect("build codec")
}

fn desc_of(name: &str) -> MessageDescriptor {
    pool()
        .get_message_by_name(name)
        .unwrap_or_else(|| panic!("missing {name}"))
}

fn empty(desc: &MessageDescriptor) -> DynamicMessage {
    DynamicMessage::new(desc.clone())
}

fn set(msg: &mut DynamicMessage, name: &str, value: Value) {
    let fd = msg.descriptor().get_field_by_name(name).expect("field");
    msg.set_field(&fd, value);
}

fn build_order() -> Vec<u8> {
    let codec = codec();
    let desc = desc_of("test.v1.Order");
    let fill_desc = desc_of("test.v1.Order.Fill");
    let mut msg = empty(&desc);
    set(&mut msg, "order_id", Value::U64(1001));
    set(&mut msg, "symbol", Value::String("AAPL".into()));
    set(&mut msg, "price", Value::I64(19150));
    set(&mut msg, "quantity", Value::U32(100));
    set(&mut msg, "side", Value::EnumNumber(1));
    set(&mut msg, "active", Value::Bool(true));
    set(&mut msg, "weight", Value::F64(0.85));
    set(&mut msg, "score", Value::F32(2.5));

    let mut fills: Vec<Value> = Vec::new();
    for &(price, qty, id) in &[(100i64, 10u32, 7u64), (200, 20, 8)] {
        let mut f = empty(&fill_desc);
        set(&mut f, "fill_price", Value::I64(price));
        set(&mut f, "fill_qty", Value::U32(qty));
        set(&mut f, "fill_id", Value::U64(id));
        fills.push(Value::Message(f));
    }
    set(&mut msg, "fills", Value::List(fills));
    marshal(&codec, &msg).expect("marshal")
}

// ---------------- scalars ----------------

#[test]
fn view_reads_scalars_and_string_by_name() {
    let codec = codec();
    let desc = desc_of("test.v1.Order");
    let mut msg = empty(&desc);
    set(&mut msg, "order_id", Value::U64(1001));
    set(&mut msg, "symbol", Value::String("AAPL".into()));
    set(&mut msg, "price", Value::I64(19150));
    set(&mut msg, "quantity", Value::U32(100));
    set(&mut msg, "side", Value::EnumNumber(1));
    set(&mut msg, "active", Value::Bool(true));
    set(&mut msg, "weight", Value::F64(0.85));
    set(&mut msg, "score", Value::F32(2.5));

    let data = marshal(&codec, &msg).expect("marshal");
    let view = codec.view(&data).expect("view");

    assert_eq!(view.uint_field("order_id").unwrap(), 1001);
    assert_eq!(view.string_field("symbol").unwrap(), "AAPL");
    assert_eq!(view.int_field("price").unwrap(), 19150);
    assert_eq!(view.uint_field("quantity").unwrap(), 100);
    assert_eq!(view.enum_field("side").unwrap(), 1);
    assert!(view.bool_field("active").unwrap());
    assert!((view.float_field("weight").unwrap() - 0.85).abs() < 1e-10);
    assert!((view.float_field("score").unwrap() - 2.5).abs() < 1e-6);
}

// ---------------- groups ----------------

#[test]
fn view_reads_group_entries_with_their_own_field_accessors() {
    let codec = codec();
    let data = build_order();
    let view = codec.view(&data).expect("view");
    let fills = view.group("fills").expect("fills group");
    assert_eq!(fills.len(), 2);

    let e0 = fills.entry(0).expect("entry 0");
    assert_eq!(e0.int_field("fill_price").unwrap(), 100);
    assert_eq!(e0.uint_field("fill_qty").unwrap(), 10);
    assert_eq!(e0.uint_field("fill_id").unwrap(), 7);

    let e1 = fills.entry(1).expect("entry 1");
    assert_eq!(e1.int_field("fill_price").unwrap(), 200);
    assert_eq!(e1.uint_field("fill_qty").unwrap(), 20);
    assert_eq!(e1.uint_field("fill_id").unwrap(), 8);
}

#[test]
fn view_reports_zero_entries_for_empty_group() {
    let codec = codec();
    let desc = desc_of("test.v1.Order");
    let mut msg = empty(&desc);
    set(&mut msg, "order_id", Value::U64(1));
    let data = marshal(&codec, &msg).expect("marshal");
    let view = codec.view(&data).expect("view");
    assert_eq!(view.group("fills").expect("fills").len(), 0);
}

#[test]
fn view_entry_index_out_of_range_errors() {
    let codec = codec();
    let desc = desc_of("test.v1.Order");
    let mut msg = empty(&desc);
    set(&mut msg, "order_id", Value::U64(1));
    let data = marshal(&codec, &msg).expect("marshal");
    let view = codec.view(&data).expect("view");
    let err = view
        .group("fills")
        .expect("fills")
        .entry(0)
        .expect_err("out of range");
    assert!(err.msg.contains("out of range"), "{}", err.msg);
}

// ---------------- composites ----------------

#[test]
fn view_descends_into_composite_fields_via_composite() {
    let codec = codec();
    let desc = desc_of("test.v1.WithComposite");
    let inner_desc = desc_of("test.v1.Inner");
    let mut msg = empty(&desc);
    set(&mut msg, "id", Value::U64(99));
    let mut inner = empty(&inner_desc);
    set(&mut inner, "x", Value::I64(100));
    set(&mut inner, "y", Value::I64(-200));
    set(&mut msg, "inner", Value::Message(inner));
    set(&mut msg, "code", Value::I32(42));

    let data = marshal(&codec, &msg).expect("marshal");
    let view = codec.view(&data).expect("view");
    assert_eq!(view.uint_field("id").unwrap(), 99);
    assert_eq!(view.int_field("code").unwrap(), 42);

    let inner_view = view.composite("inner").expect("inner composite");
    assert_eq!(inner_view.int_field("x").unwrap(), 100);
    assert_eq!(inner_view.int_field("y").unwrap(), -200);
}

// ---------------- errors ----------------

#[test]
fn view_rejects_unknown_template_id() {
    let codec = codec();
    let data = vec![0u8; 8]; // header zeroed → templateId 0
    let err = codec.view(&data).expect_err("unknown template ID");
    assert!(err.msg.contains("unknown template ID"), "{}", err.msg);
}

#[test]
fn view_rejects_buffer_shorter_than_header() {
    let codec = codec();
    let err = codec.view(&[0u8; 4]).expect_err("too short");
    assert!(err.msg.contains("too short for header"), "{}", err.msg);
}

#[test]
fn view_unknown_field_name_errors() {
    let codec = codec();
    let data = build_order();
    let view = codec.view(&data).expect("view");
    let err = view.int_field("nope").expect_err("unknown field");
    assert!(err.msg.contains("unknown field"), "{}", err.msg);
}
