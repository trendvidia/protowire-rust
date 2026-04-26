//! Slice B SBE codec round-trip tests. Mirrors the TS port's
//! `src/sbe/codec.test.ts` for marshal + unmarshal symmetry across
//! Simple, Order (with Fill group), WithComposite, WithNarrow.

use prost_reflect::{DescriptorPool, DynamicMessage, MessageDescriptor, ReflectMessage, Value};
use protowire_sbe::{marshal, unmarshal, Codec};

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
    let fd = msg
        .descriptor()
        .get_field_by_name(name)
        .unwrap_or_else(|| panic!("no field {name} on {}", msg.descriptor().full_name()));
    msg.set_field(&fd, value);
}

fn get(msg: &DynamicMessage, name: &str) -> Value {
    let fd = msg
        .descriptor()
        .get_field_by_name(name)
        .unwrap_or_else(|| panic!("no field {name} on {}", msg.descriptor().full_name()));
    msg.get_field(&fd).into_owned()
}

// ---------------- Simple ----------------

#[test]
fn simple_round_trips_through_16_bytes() {
    let codec = codec();
    let desc = desc_of("test.v1.Simple");
    let mut msg = empty(&desc);
    set(&mut msg, "id", Value::U32(42));
    set(&mut msg, "value", Value::I32(-100));

    let data = marshal(&codec, &msg).expect("marshal");
    assert_eq!(data.len(), 16); // header(8) + id(4) + value(4)

    let mut got = empty(&desc);
    unmarshal(&codec, &mut got, &data).expect("unmarshal");
    assert!(matches!(get(&got, "id"), Value::U32(42)));
    assert!(matches!(get(&got, "value"), Value::I32(-100)));
}

#[test]
fn zero_value_simple_round_trips() {
    let codec = codec();
    let desc = desc_of("test.v1.Simple");
    let msg = empty(&desc);
    let data = marshal(&codec, &msg).expect("marshal");
    assert_eq!(data.len(), 16);

    let mut got = empty(&desc);
    unmarshal(&codec, &mut got, &data).expect("unmarshal");
    assert!(matches!(get(&got, "id"), Value::U32(0)));
    assert!(matches!(get(&got, "value"), Value::I32(0)));
}

// ---------------- Order ----------------

#[test]
fn order_with_fills_round_trips_through_94_bytes() {
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
    for &(price, qty, id) in &[(19155i64, 25u32, 5001u64), (19160, 50, 5002)] {
        let mut f = empty(&fill_desc);
        set(&mut f, "fill_price", Value::I64(price));
        set(&mut f, "fill_qty", Value::U32(qty));
        set(&mut f, "fill_id", Value::U64(id));
        fills.push(Value::Message(f));
    }
    set(&mut msg, "fills", Value::List(fills));

    let data = marshal(&codec, &msg).expect("marshal");
    // header(8) + root(42) + group_header(4) + 2*fill(20) = 94
    assert_eq!(data.len(), 94);

    let mut got = empty(&desc);
    unmarshal(&codec, &mut got, &data).expect("unmarshal");
    assert!(matches!(get(&got, "order_id"), Value::U64(1001)));
    assert!(matches!(get(&got, "symbol"), Value::String(s) if s == "AAPL"));
    assert!(matches!(get(&got, "price"), Value::I64(19150)));
    assert!(matches!(get(&got, "quantity"), Value::U32(100)));
    assert!(matches!(get(&got, "side"), Value::EnumNumber(1)));
    assert!(matches!(get(&got, "active"), Value::Bool(true)));
    let weight = match get(&got, "weight") {
        Value::F64(f) => f,
        v => panic!("weight not f64: {v:?}"),
    };
    assert!((weight - 0.85).abs() < 1e-10);
    let score = match get(&got, "score") {
        Value::F32(f) => f,
        v => panic!("score not f32: {v:?}"),
    };
    assert!((score - 2.5_f32).abs() < 1e-6);

    let got_fills = match get(&got, "fills") {
        Value::List(items) => items,
        v => panic!("fills not list: {v:?}"),
    };
    assert_eq!(got_fills.len(), 2);
    let f0 = match &got_fills[0] {
        Value::Message(m) => m,
        v => panic!("fill[0] not message: {v:?}"),
    };
    assert!(matches!(get(f0, "fill_price"), Value::I64(19155)));
    assert!(matches!(get(f0, "fill_qty"), Value::U32(25)));
    assert!(matches!(get(f0, "fill_id"), Value::U64(5001)));
    let f1 = match &got_fills[1] {
        Value::Message(m) => m,
        v => panic!("fill[1] not message: {v:?}"),
    };
    assert!(matches!(get(f1, "fill_price"), Value::I64(19160)));
    assert!(matches!(get(f1, "fill_qty"), Value::U32(50)));
    assert!(matches!(get(f1, "fill_id"), Value::U64(5002)));
}

#[test]
fn empty_group_emits_group_header_only() {
    let codec = codec();
    let desc = desc_of("test.v1.Order");
    let mut msg = empty(&desc);
    set(&mut msg, "order_id", Value::U64(1));

    let data = marshal(&codec, &msg).expect("marshal");
    // header(8) + root(42) + group_header(4) = 54
    assert_eq!(data.len(), 54);

    let mut got = empty(&desc);
    unmarshal(&codec, &mut got, &data).expect("unmarshal");
    assert!(matches!(get(&got, "order_id"), Value::U64(1)));
    let fills = match get(&got, "fills") {
        Value::List(items) => items,
        v => panic!("fills not list: {v:?}"),
    };
    assert_eq!(fills.len(), 0);
}

#[test]
fn strings_longer_than_sbe_length_are_truncated_on_marshal() {
    let codec = codec();
    let desc = desc_of("test.v1.Order");
    let mut msg = empty(&desc);
    set(&mut msg, "symbol", Value::String("LONGERTHAN8".into()));

    let data = marshal(&codec, &msg).expect("marshal");
    let mut got = empty(&desc);
    unmarshal(&codec, &mut got, &data).expect("unmarshal");
    assert!(matches!(get(&got, "symbol"), Value::String(s) if s == "LONGERTH"));
}

#[test]
fn large_negative_int64_round_trips() {
    let codec = codec();
    let desc = desc_of("test.v1.Order");
    let mut msg = empty(&desc);
    set(&mut msg, "price", Value::I64(-99999));

    let data = marshal(&codec, &msg).expect("marshal");
    let mut got = empty(&desc);
    unmarshal(&codec, &mut got, &data).expect("unmarshal");
    assert!(matches!(get(&got, "price"), Value::I64(-99999)));
}

// ---------------- WithComposite ----------------

#[test]
fn with_composite_round_trips_through_36_bytes() {
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
    // header(8) + id(8) + inner(x+y=16) + code(4) = 36
    assert_eq!(data.len(), 36);

    let mut got = empty(&desc);
    unmarshal(&codec, &mut got, &data).expect("unmarshal");
    assert!(matches!(get(&got, "id"), Value::U64(99)));
    assert!(matches!(get(&got, "code"), Value::I32(42)));
    let got_inner = match get(&got, "inner") {
        Value::Message(m) => m,
        v => panic!("inner not message: {v:?}"),
    };
    assert!(matches!(get(&got_inner, "x"), Value::I64(100)));
    assert!(matches!(get(&got_inner, "y"), Value::I64(-200)));
}

// ---------------- WithNarrow ----------------

#[test]
fn sbe_encoding_overrides_change_wire_size() {
    let codec = codec();
    let desc = desc_of("test.v1.WithNarrow");
    let mut msg = empty(&desc);
    set(&mut msg, "status", Value::U32(200));
    set(&mut msg, "port", Value::U32(8080));
    set(&mut msg, "delta", Value::I32(-1234));

    let data = marshal(&codec, &msg).expect("marshal");
    // header(8) + status(1) + port(2) + delta(2) = 13
    assert_eq!(data.len(), 13);

    let mut got = empty(&desc);
    unmarshal(&codec, &mut got, &data).expect("unmarshal");
    assert!(matches!(get(&got, "status"), Value::U32(200)));
    assert!(matches!(get(&got, "port"), Value::U32(8080)));
    assert!(matches!(get(&got, "delta"), Value::I32(-1234)));
}

// ---------------- error case ----------------

#[test]
fn template_id_mismatch_errors_on_unmarshal() {
    let codec = codec();
    let order = desc_of("test.v1.Order");
    let simple = desc_of("test.v1.Simple");
    let data = marshal(&codec, &empty(&simple)).expect("marshal Simple");
    let mut got = empty(&order);
    let err = unmarshal(&codec, &mut got, &data)
        .expect_err("Order/Simple template-id mismatch should error");
    assert!(err.msg.contains("template ID mismatch"), "{}", err.msg);
}
