// SPDX-License-Identifier: MIT
// Copyright (c) 2026 TrendVidia, LLC.
//! Draft -01 § Any Blocks (trendvidia/protowire#501): `marshal` writes an
//! Any as `@type` and a body only when reading that body gives back the
//! Any's value, byte for byte. Anything else stays as the Any's own fields,
//! `type_url` and `value`. A reader compares an Any by its bytes, so a body
//! that drops or reorders them changes the message. Mirrors protowire-go's
//! `encoding/pxf/any_body_readback_test.go`.

use std::cell::Cell;

use prost::Message as _;
use prost_reflect::{DescriptorPool, DynamicMessage, Kind, MessageDescriptor, Value};
use protowire_pxf::{
    marshal, unmarshal, unmarshal_full, MarshalOptions, PoolResolver, TypeResolver,
    UnmarshalOptions,
};

const FDS: &[u8] = include_bytes!("../testdata/any-readback-test.binpb");

fn pool() -> &'static DescriptorPool {
    Box::leak(Box::new(DescriptorPool::decode(FDS).expect("decode FDS")))
}

fn holder(p: &DescriptorPool) -> MessageDescriptor {
    p.get_message_by_name("anyback.v1.Holder").unwrap()
}

/// A Holder whose payload is `type_name` with `value` verbatim.
fn any_holder(d: &MessageDescriptor, type_name: &str, value: Vec<u8>) -> DynamicMessage {
    let fd = d.get_field_by_name("payload").unwrap();
    let Kind::Message(any_desc) = fd.kind() else {
        unreachable!()
    };
    let mut a = DynamicMessage::new(any_desc);
    a.set_field_by_name("type_url", Value::String(type_name.into()));
    a.set_field_by_name("value", Value::Bytes(value.into()));
    let mut m = DynamicMessage::new(d.clone());
    m.set_field(&fd, Value::Message(a));
    m
}

/// Marshal `m` and read it back with both decoders, each giving the same
/// bytes. Returns the document.
fn assert_reads_back(d: &MessageDescriptor, res: &dyn TypeResolver, m: &DynamicMessage) -> String {
    let out = marshal(
        m,
        d,
        MarshalOptions {
            type_resolver: Some(res),
            ..Default::default()
        },
    )
    .unwrap_or_else(|e| panic!("marshal: {e}"));
    let opts = UnmarshalOptions {
        type_resolver: Some(res),
        ..Default::default()
    };
    let want = m.encode_to_vec();
    let back = unmarshal(&out, d, opts).unwrap_or_else(|e| panic!("unmarshal: {e}\n{out}"));
    assert_eq!(
        back.encode_to_vec(),
        want,
        "unmarshal changed the message:\n{out}"
    );
    let (full, _) =
        unmarshal_full(&out, d, opts).unwrap_or_else(|e| panic!("unmarshal_full: {e}\n{out}"));
    assert_eq!(
        full.encode_to_vec(),
        want,
        "unmarshal_full changed the message:\n{out}"
    );
    out
}

fn varint(mut v: u64) -> Vec<u8> {
    let mut out = Vec::new();
    while v >= 0x80 {
        out.push((v as u8) | 0x80);
        v >>= 7;
    }
    out.push(v as u8);
    out
}

fn varint_field(num: u64, v: u64) -> Vec<u8> {
    [varint(num << 3), varint(v)].concat()
}

fn len_field(num: u64, body: &[u8]) -> Vec<u8> {
    [
        varint((num << 3) | 2),
        varint(body.len() as u64),
        body.to_vec(),
    ]
    .concat()
}

#[test]
fn written_only_when_it_reads_back() {
    let p = pool();
    let d = holder(p);
    let res = PoolResolver(p);

    let tag_t = len_field(1, b"t");
    let n1 = varint_field(2, 1);
    let unknown = varint_field(101, 1);
    let entry = |k: &str, v: u64| {
        len_field(
            4,
            &[len_field(1, k.as_bytes()), varint_field(2, v)].concat(),
        )
    };

    let cases: Vec<(&str, &str, Vec<u8>, bool)> = vec![
        // What a body can say keeps the body form.
        (
            "canonical",
            "anyback.v1.Leaf",
            [tag_t.clone(), n1.clone()].concat(),
            true,
        ),
        ("empty", "anyback.v1.Leaf", vec![], true),
        (
            "timestamp",
            "anyback.v1.Rich",
            len_field(1, &varint_field(1, 1_700_000_000)),
            true,
        ),
        (
            "wrapper",
            "anyback.v1.Rich",
            len_field(2, &varint_field(1, 7)),
            true,
        ),
        (
            "wrapper holding zero",
            "anyback.v1.Rich",
            len_field(2, &[]),
            true,
        ),
        (
            "sorted map",
            "anyback.v1.Rich",
            [entry("a", 1), entry("b", 2)].concat(),
            true,
        ),
        (
            "packed list",
            "anyback.v1.Rich",
            len_field(5, &[1, 2]),
            true,
        ),
        ("optional zero", "anyback.v1.Rich", varint_field(6, 0), true),
        (
            "default field set",
            "anyback.v1.Defaulted",
            len_field(1, b"x"),
            true,
        ),
        (
            "required field set",
            "anyback.v1.Required",
            varint_field(1, 3),
            true,
        ),
        // The issue's cases.
        (
            "unknown field",
            "anyback.v1.Leaf",
            [tag_t.clone(), unknown.clone()].concat(),
            false,
        ),
        (
            "fields out of order",
            "anyback.v1.Leaf",
            [n1.clone(), tag_t.clone()].concat(),
            false,
        ),
        // The same loss one level down.
        (
            "unknown in submessage",
            "anyback.v1.Rich",
            len_field(3, &[tag_t.clone(), unknown.clone()].concat()),
            false,
        ),
        (
            "unknown in list element",
            "anyback.v1.Rich",
            len_field(8, &[tag_t.clone(), unknown.clone()].concat()),
            false,
        ),
        (
            "unknown in wrapper",
            "anyback.v1.Rich",
            len_field(2, &[varint_field(1, 7), unknown.clone()].concat()),
            false,
        ),
        (
            "unknown in timestamp",
            "anyback.v1.Rich",
            len_field(1, &[varint_field(1, 5), unknown.clone()].concat()),
            false,
        ),
        // Bytes a decoder would not write back.
        (
            "explicit zero",
            "anyback.v1.Leaf",
            varint_field(2, 0),
            false,
        ),
        (
            "long varint",
            "anyback.v1.Leaf",
            vec![0x10, 0x81, 0x00],
            false,
        ),
        (
            "unpacked list",
            "anyback.v1.Rich",
            [varint_field(5, 1), varint_field(5, 2)].concat(),
            false,
        ),
        (
            "map out of order",
            "anyback.v1.Rich",
            [entry("b", 2), entry("a", 1)].concat(),
            false,
        ),
        (
            "repeated singular field",
            "anyback.v1.Leaf",
            [varint_field(2, 1), varint_field(2, 2)].concat(),
            false,
        ),
        // A literal cannot hold it.
        (
            "timestamp past year 9999",
            "anyback.v1.Rich",
            len_field(1, &varint_field(1, 1 << 40)),
            false,
        ),
        // An unset annotated field is written so that it reads back (#116,
        // protowire-go#259), so the body gives back the bytes.
        (
            "default unset",
            "anyback.v1.Defaulted",
            varint_field(2, 1),
            true,
        ),
        (
            "required unset",
            "anyback.v1.Required",
            len_field(2, b"s"),
            true,
        ),
        // Never unpacked at all.
        ("unresolved", "anyback.v1.Nope", tag_t.clone(), false),
        ("unparsable", "anyback.v1.Leaf", vec![0xff], false),
        (
            "map entry with a key of the wrong wire type",
            "anyback.v1.Rich",
            len_field(4, &[len_field(1, b"a"), varint_field(1, 5)].concat()),
            false,
        ),
    ];
    let mut wrong = Vec::new();
    for (name, type_name, value, body) in cases {
        let out = assert_reads_back(&d, &res, &any_holder(&d, type_name, value));
        if out.contains("@type") != body {
            wrong.push(format!("{name}: want body={body}\n{out}"));
        }
    }
    assert!(wrong.is_empty(), "{}", wrong.join("\n"));
}

/// `n` nested Anys, each holding a Holder, around leaf bytes.
fn holder_chain(d: &MessageDescriptor, n: usize) -> DynamicMessage {
    let mut m = any_holder(d, "anyback.v1.Leaf", len_field(1, b"t"));
    for _ in 1..n {
        m = any_holder(d, "anyback.v1.Holder", m.encode_to_vec());
    }
    m
}

/// A chain of Anys costs its own bytes, not MaxNestingDepth, on the wire:
/// without a limit a long one wrote a document no reader accepts.
#[test]
fn chain_stops_at_the_depth_limit() {
    let p = pool();
    let d = holder(p);
    let res = PoolResolver(p);
    // The 100th Any's body needs the 101st's raw block.
    for (n, bodies) in [(1, 1), (99, 99), (100, 100), (101, 99), (300, 99)] {
        let out = assert_reads_back(&d, &res, &holder_chain(&d, n));
        assert_eq!(out.matches("@type").count(), bodies, "{n} Anys");
    }
}

struct CountingResolver<'a> {
    inner: PoolResolver<'a>,
    calls: Cell<usize>,
}

impl TypeResolver for CountingResolver<'_> {
    fn find_message_by_url(&self, url: &str) -> Option<MessageDescriptor> {
        self.calls.set(self.calls.get() + 1);
        self.inner.find_message_by_url(url)
    }
}

/// The encoder stops unpacking at the limit, so a longer chain costs it no
/// more lookups.
#[test]
fn chain_past_the_limit_is_not_walked() {
    let p = pool();
    let d = holder(p);
    let lookups = |n: usize| {
        let r = CountingResolver {
            inner: PoolResolver(p),
            calls: Cell::new(0),
        };
        let opts = MarshalOptions {
            type_resolver: Some(&r),
            ..Default::default()
        };
        marshal(&holder_chain(&d, n), &d, opts).expect("marshal");
        r.calls.get()
    };
    assert_eq!(lookups(300), lookups(2000));
}

/// A body is measured where it lands: the same value that fits near the
/// root does not fit near the limit, and only the Any there stays raw.
#[test]
fn submessage_depth_counts() {
    let p = pool();
    let d = holder(p);
    let res = PoolResolver(p);
    let nest = |levels: usize| {
        let mut m = any_holder(&d, "anyback.v1.Rich", len_field(3, &len_field(1, b"t")));
        for _ in 0..levels {
            let mut parent = DynamicMessage::new(d.clone());
            parent.set_field_by_name("next", Value::Message(m));
            m = parent;
        }
        m
    };
    for (levels, body) in [(0, true), (98, true), (99, false)] {
        let out = assert_reads_back(&d, &res, &nest(levels));
        assert_eq!(out.contains("@type"), body, "{levels} levels");
    }
}

/// `emit_defaults` writes an unset optional field out, which sets it for a
/// reader, so a body that has one stays raw.
#[test]
fn emit_defaults() {
    let p = pool();
    let d = holder(p);
    let res = PoolResolver(p);
    let m = any_holder(&d, "anyback.v1.Rich", len_field(3, &[]));
    let out = marshal(
        &m,
        &d,
        MarshalOptions {
            type_resolver: Some(&res),
            emit_defaults: true,
            ..Default::default()
        },
    )
    .unwrap();
    let back = unmarshal(
        &out,
        &d,
        UnmarshalOptions {
            type_resolver: Some(&res),
            ..Default::default()
        },
    )
    .unwrap_or_else(|e| panic!("{e}\n{out}"));
    assert_eq!(back.encode_to_vec(), m.encode_to_vec(), "{out}");
}

/// The decoder packs an Any body's map in key order, so reading the same
/// document twice gives the same bytes, and they are the bytes a
/// deterministic writer gives.
#[test]
fn decoder_packs_a_map_in_key_order() {
    let p = pool();
    let d = holder(p);
    let res = PoolResolver(p);
    let keys = ["q", "a", "m", "z", "b", "k", "c", "x", "d", "e"];
    let mut body = String::from("payload {\n  @type = \"anyback.v1.Rich\"\n  m = {\n");
    for (i, k) in keys.iter().enumerate() {
        body.push_str(&format!("    {k}: {i}\n"));
    }
    body.push_str("  }\n}\n");
    let mut sorted: Vec<(&str, u64)> = keys
        .iter()
        .enumerate()
        .map(|(i, k)| (*k, i as u64))
        .collect();
    sorted.sort();
    let want: Vec<u8> = sorted
        .iter()
        .flat_map(|(k, v)| {
            len_field(
                4,
                &[len_field(1, k.as_bytes()), varint_field(2, *v)].concat(),
            )
        })
        .collect();
    for _ in 0..5 {
        let m = unmarshal(
            &body,
            &d,
            UnmarshalOptions {
                type_resolver: Some(&res),
                ..Default::default()
            },
        )
        .unwrap();
        let any = m.get_field_by_name("payload").unwrap().into_owned();
        let Value::Message(any) = any else {
            unreachable!()
        };
        let value = any.get_field_by_name("value").unwrap().into_owned();
        assert_eq!(value.as_bytes().unwrap().as_ref(), &want[..]);
    }
}
