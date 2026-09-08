// SPDX-License-Identifier: MIT
// Copyright (c) 2026 TrendVidia, LLC.
//! HARDENING.md § Mandatory limits and § SBE validation for the SBE codec
//! (protowire-rust#33): the input to one decode or view is capped at
//! `MaxMessageSize` before the header is read; a group's `numInGroup` at
//! `MaxRepeatedCount` before any entry is allocated; a wire block shorter
//! than the template's is rejected (step 2); a zero block length with a
//! non-zero count is rejected (step 4); and the group walk is bounds-checked
//! in 64-bit arithmetic (step 3). Mirrors protowire-go's `TestSizeLimits`.

use prost_reflect::{DescriptorPool, DynamicMessage, MessageDescriptor, ReflectMessage, Value};
use protowire_sbe::{marshal, unmarshal, Codec, Limits, MAX_MESSAGE_SIZE};

const SBE_FDS: &[u8] = include_bytes!("../testdata/sbe-test.binpb");

fn pool() -> DescriptorPool {
    DescriptorPool::decode(SBE_FDS).expect("decode sbe-test.binpb")
}

fn codec_with(limits: Limits) -> Codec {
    let p = pool();
    let file = p
        .get_file_by_name("sbe-test.proto")
        .expect("sbe-test.proto");
    Codec::from_files_with_limits(&[file], limits).expect("build codec")
}

fn desc_of(name: &str) -> MessageDescriptor {
    pool()
        .get_message_by_name(name)
        .unwrap_or_else(|| panic!("missing {name}"))
}

fn set(msg: &mut DynamicMessage, name: &str, value: Value) {
    let fd = msg.descriptor().get_field_by_name(name).unwrap();
    msg.set_field(&fd, value);
}

/// An Order with five fills: header(8) + root block(42) + group header(4)
/// + 5 × 20.
fn five_fills() -> Vec<u8> {
    let codec = codec_with(Limits::default());
    let desc = desc_of("test.v1.Order");
    let fill_desc = desc_of("test.v1.Order.Fill");
    let mut msg = DynamicMessage::new(desc);
    set(&mut msg, "order_id", Value::U64(7));
    set(&mut msg, "symbol", Value::String("AAPL".into()));
    let fills: Vec<Value> = (0..5u32)
        .map(|i| {
            let mut f = DynamicMessage::new(fill_desc.clone());
            set(&mut f, "fill_qty", Value::U32(i));
            Value::Message(f)
        })
        .collect();
    set(&mut msg, "fills", Value::List(fills));
    marshal(&codec, &msg).expect("marshal")
}

fn decode(codec: &Codec, data: &[u8]) -> Result<DynamicMessage, protowire_sbe::SbeError> {
    let mut out = DynamicMessage::new(desc_of("test.v1.Order"));
    unmarshal(codec, &mut out, data).map(|_| out)
}

fn fills_len(msg: &DynamicMessage) -> usize {
    match msg.get_field_by_name("fills").map(|v| v.into_owned()) {
        Some(Value::List(items)) => items.len(),
        _ => 0,
    }
}

#[test]
fn max_message_size_on_unmarshal_and_view() {
    let data = five_fills();
    let default = codec_with(Limits::default());
    assert_eq!(fills_len(&decode(&default, &data).expect("default")), 5);

    let small = codec_with(Limits {
        max_message_size: 8,
        ..Limits::default()
    });
    let err = decode(&small, &data).unwrap_err();
    assert_eq!(
        err.msg,
        format!(
            "sbe: input of {} bytes exceeds MaxMessageSize=8",
            data.len()
        )
    );
    let err = small.view(&data).unwrap_err();
    assert!(err.msg.contains("MaxMessageSize=8"), "view: {}", err.msg);

    let exact = codec_with(Limits {
        max_message_size: data.len(),
        ..Limits::default()
    });
    decode(&exact, &data).expect("at the bound");
    exact.view(&data).expect("at the bound, view");

    // The default rejects 64 MiB + 1 before reading the header.
    let mut big = vec![0u8; MAX_MESSAGE_SIZE + 1];
    big[..data.len()].copy_from_slice(&data);
    let err = decode(&default, &big).unwrap_err();
    assert!(err.msg.contains("MaxMessageSize=67108864"), "{}", err.msg);
}

#[test]
fn max_repeated_count_before_any_entry_is_allocated() {
    let data = five_fills();
    let four = codec_with(Limits {
        max_repeated_count: 4,
        ..Limits::default()
    });
    let err = decode(&four, &data).unwrap_err();
    assert_eq!(
        err.msg,
        "sbe: group fills declares 5 entries, exceeds MaxRepeatedCount=4"
    );
    let v = four.view(&data).expect("the root view is fine");
    let err = v.group("fills").unwrap_err();
    assert!(
        err.msg.contains("MaxRepeatedCount=4"),
        "view walk: {}",
        err.msg
    );

    let five = codec_with(Limits {
        max_repeated_count: 5,
        ..Limits::default()
    });
    assert_eq!(fills_len(&decode(&five, &data).expect("at the bound")), 5);
    assert_eq!(five.view(&data).unwrap().group("fills").unwrap().len(), 5);
}

/// § SBE step 2: a wire block strictly smaller than the template's block
/// length is rejected, for the root block and for a group's entries; a
/// larger one is forward-compatible and accepted.
#[test]
fn wire_block_shorter_than_template_is_rejected() {
    let codec = codec_with(Limits::default());
    let mut data = five_fills();
    // Root block_length is the first u16 LE; the template's is 42.
    data[0] = 2;
    data[1] = 0;
    let err = decode(&codec, &data).unwrap_err();
    assert_eq!(
        err.msg,
        "sbe: wire block_length 2 is below template block_length 42"
    );
    let err = codec.view(&data).unwrap_err();
    assert!(
        err.msg.contains("below template block_length"),
        "view: {}",
        err.msg
    );

    // A group's entry block below the template's 20.
    let mut data = five_fills();
    let g = 8 + 42;
    data[g] = 4;
    data[g + 1] = 0;
    let err = decode(&codec, &data).unwrap_err();
    assert_eq!(
        err.msg,
        "sbe: group fills wire block_length 4 is below template block_length 20"
    );
}

/// § SBE step 4: a zero entry block length with a non-zero count invites
/// allocating `count` entries for zero further bytes; rejected outright,
/// on decode and on the view's group walk.
#[test]
fn zero_block_length_with_entries_is_rejected() {
    let codec = codec_with(Limits::default());
    let mut data = five_fills();
    let g = 8 + 42;
    data[g] = 0;
    data[g + 1] = 0;
    data[g + 2] = 0x10;
    data[g + 3] = 0x27; // count 10000
    let err = decode(&codec, &data).unwrap_err();
    assert_eq!(
        err.msg,
        "sbe: group fills declares 10000 entries of block_length 0"
    );
    let err = codec.view(&data).unwrap().group("fills").unwrap_err();
    assert!(err.msg.contains("block_length 0"), "view walk: {}", err.msg);
}

/// § SBE step 3: `count × block_length` is checked against the buffer in
/// 64-bit arithmetic before any entry is read — a header asserting
/// 0xFFFF entries of 0xFFFF bytes on a short buffer is a clean error on
/// decode and on the view's group walk, never an out-of-bounds read.
#[test]
fn group_count_overflow_is_a_clean_error() {
    let codec = codec_with(Limits::default());
    let mut data = five_fills();
    let g = 8 + 42;
    data[g..g + 4].copy_from_slice(&[0xff, 0xff, 0xff, 0xff]);
    let err = decode(&codec, &data).unwrap_err();
    assert!(
        err.msg.contains("data too short for group entries"),
        "{}",
        err.msg
    );
    let err = codec.view(&data).unwrap().group("fills").unwrap_err();
    assert!(
        err.msg.contains("data too short for group entries"),
        "view walk: {}",
        err.msg
    );
}
