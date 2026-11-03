// SPDX-License-Identifier: MIT
// Copyright (c) 2026 TrendVidia, LLC.
//! Draft -01 § Recursion (trendvidia/protowire#502): `marshal` refuses a
//! message deeper than a decoder at the same limit accepts, so nothing it
//! writes is refused for depth when read back. Mirrors protowire-go's
//! `encoding/pxf/marshal_depth_test.go`.

use prost::Message as _;
use prost_reflect::{
    DescriptorPool, DynamicMessage, MapKey, MessageDescriptor, ReflectMessage, Value,
};
use protowire_pxf::{
    marshal, parse, unmarshal, Limits, MarshalOptions, PoolResolver, UnmarshalOptions,
    MAX_NESTING_DEPTH,
};

const FDS: &[u8] = include_bytes!("../testdata/marshal-depth-test.binpb");
const DEPTH_ERROR: &str = "nesting depth exceeds MaxNestingDepth=100";

fn pool() -> &'static DescriptorPool {
    Box::leak(Box::new(DescriptorPool::decode(FDS).expect("decode FDS")))
}

fn t_desc(p: &DescriptorPool) -> MessageDescriptor {
    p.get_message_by_name("mdepth.v1.T").unwrap()
}

fn resolver(p: &'static DescriptorPool) -> &'static PoolResolver<'static> {
    Box::leak(Box::new(PoolResolver(p)))
}

fn reader(max_nesting_depth: usize) -> UnmarshalOptions<'static> {
    UnmarshalOptions {
        limits: Limits {
            max_nesting_depth,
            ..Limits::default()
        },
        ..Default::default()
    }
}

/// Whether two messages hold the same value: the same bytes, as
/// `proto.Equal` compares them. `DynamicMessage`'s `==` also tells an unset
/// zero from a set one, and its `Debug` recurses through a self-referential
/// descriptor without end, so `assert_eq!` on one cannot report a mismatch.
fn same(a: &DynamicMessage, b: &DynamicMessage) -> bool {
    a.encode_to_vec() == b.encode_to_vec()
}

/// `m` wrapped in `n` singular `next` fields.
fn deep(desc: &MessageDescriptor, mut m: DynamicMessage, n: usize) -> DynamicMessage {
    for _ in 0..n {
        let mut p = DynamicMessage::new(desc.clone());
        p.set_field_by_name("next", Value::Message(m));
        m = p;
    }
    m
}

type Wrap = fn(&MessageDescriptor, DynamicMessage) -> DynamicMessage;
type Leaf = fn(&mut DynamicMessage);

fn key(s: &str) -> MapKey {
    MapKey::String(s.to_string())
}

const WRAPS: [(&str, Wrap); 4] = [
    ("singular", |d, c| deep(d, c, 1)),
    ("list", |d, c| {
        let mut p = DynamicMessage::new(d.clone());
        p.set_field_by_name("kids", Value::List(vec![Value::Message(c)]));
        p
    }),
    ("map", |d, c| {
        let mut p = DynamicMessage::new(d.clone());
        p.set_field_by_name(
            "by_key",
            Value::Map([(key("k"), Value::Message(c))].into_iter().collect()),
        );
        p
    }),
    ("keyed", |d, mut c| {
        c.set_field_by_name("id", Value::String("x".into()));
        let mut p = DynamicMessage::new(d.clone());
        p.set_field_by_name("keyed", Value::List(vec![Value::Message(c)]));
        p
    }),
];

const LEAVES: [(&str, Leaf); 5] = [
    ("empty", |_| {}),
    ("scalar", |m| m.set_field_by_name("v", Value::I32(1))),
    ("list", |m| {
        m.set_field_by_name("ns", Value::List(vec![Value::I32(1)]))
    }),
    ("scalar map", |m| {
        m.set_field_by_name(
            "scalars",
            Value::Map([(key("a"), Value::I32(1))].into_iter().collect()),
        )
    }),
    // A Duration past the literal's range is a block, not a literal.
    ("duration block", |m| {
        let fd = m.descriptor().get_field_by_name("long").unwrap();
        let prost_reflect::Kind::Message(dd) = fd.kind() else {
            unreachable!()
        };
        let mut d = DynamicMessage::new(dd);
        d.set_field_by_name("seconds", Value::I64(9_500_000_000));
        m.set_field(&fd, Value::Message(d));
    }),
];

/// Every shape the writer writes, the decoder and the parser accept, and
/// the writer refuses the rest, for each way of nesting and each leaf.
#[test]
fn depth_agrees_with_decoder_and_parser() {
    let p = pool();
    let d = t_desc(p);
    for (wn, wrap) in WRAPS {
        for (ln, leaf) in LEAVES {
            let (mut written, mut refused) = (0, 0);
            for n in 1..=MAX_NESTING_DEPTH + 2 {
                let mut m = DynamicMessage::new(d.clone());
                leaf(&mut m);
                for _ in 0..n {
                    m = wrap(&d, m);
                }
                let out = match marshal(&m, &d, MarshalOptions::default()) {
                    Ok(out) => out,
                    Err(e) => {
                        assert!(e.msg.contains(DEPTH_ERROR), "{wn}/{ln} {n}: {}", e.msg);
                        refused += 1;
                        continue;
                    }
                };
                written += 1;
                let back = unmarshal(&out, &d, UnmarshalOptions::default())
                    .unwrap_or_else(|e| panic!("{wn}/{ln} {n}: {e}"));
                assert!(same(&back, &m), "{wn}/{ln} {n}");
                parse(&out).unwrap_or_else(|e| panic!("{wn}/{ln} {n}: parse: {e}"));
            }
            assert!(written > 0, "{wn}/{ln}: nothing written");
            assert!(refused > 0, "{wn}/{ln}: nothing refused");
        }
    }
}

/// The largest shape that fits is written: refusal is not a level early.
#[test]
fn depth_bound() {
    let p = pool();
    let d = t_desc(p);
    let at = deep(&d, DynamicMessage::new(d.clone()), MAX_NESTING_DEPTH);
    let out = marshal(&at, &d, MarshalOptions::default()).expect("100 descents");
    unmarshal(&out, &d, UnmarshalOptions::default()).expect("reads back");
    let past = deep(&d, DynamicMessage::new(d.clone()), MAX_NESTING_DEPTH + 1);
    let err = marshal(&past, &d, MarshalOptions::default()).unwrap_err();
    assert!(err.msg.contains(DEPTH_ERROR), "{}", err.msg);
}

#[test]
fn max_nesting_depth_is_per_call() {
    let p = pool();
    let d = t_desc(p);
    let m = deep(&d, DynamicMessage::new(d.clone()), 150);
    let raised = MarshalOptions {
        max_nesting_depth: 150,
        ..Default::default()
    };
    let out = marshal(&m, &d, raised).expect("150 under 150");
    assert!(same(&unmarshal(&out, &d, reader(150)).unwrap(), &m));

    let lowered = MarshalOptions {
        max_nesting_depth: 149,
        ..Default::default()
    };
    let err = marshal(&m, &d, lowered).unwrap_err();
    assert!(err.msg.contains("MaxNestingDepth=149"), "{}", err.msg);
    let two = MarshalOptions {
        max_nesting_depth: 2,
        ..Default::default()
    };
    let three = deep(&d, DynamicMessage::new(d.clone()), 3);
    let err = marshal(&three, &d, two).unwrap_err();
    assert!(err.msg.contains("MaxNestingDepth=2"), "{}", err.msg);
}

/// A T whose payload holds `value` as a T.
fn t_any(d: &MessageDescriptor, value: &DynamicMessage) -> DynamicMessage {
    let mut m = DynamicMessage::new(d.clone());
    let fd = d.get_field_by_name("payload").unwrap();
    let prost_reflect::Kind::Message(any_desc) = fd.kind() else {
        unreachable!()
    };
    let mut a = DynamicMessage::new(any_desc);
    a.set_field_by_name(
        "type_url",
        Value::String("type.googleapis.com/mdepth.v1.T".into()),
    );
    a.set_field_by_name("value", Value::Bytes(value.encode_to_vec().into()));
    m.set_field(&fd, Value::Message(a));
    m
}

/// An Any is not refused for what its bytes hold: when a body would be too
/// deep, the Any stays raw, at any limit.
#[test]
fn any_too_deep_stays_raw() {
    let p = pool();
    let d = t_desc(p);
    let res = resolver(p);
    let m = t_any(&d, &deep(&d, DynamicMessage::new(d.clone()), 150));

    let opts = MarshalOptions {
        type_resolver: Some(res),
        ..Default::default()
    };
    let out = marshal(&m, &d, opts).expect("raw Any");
    assert!(!out.contains("@type"), "{out}");
    let read = UnmarshalOptions {
        type_resolver: Some(res),
        ..Default::default()
    };
    assert!(same(&unmarshal(&out, &d, read).unwrap(), &m));

    // DIVERGENCE from protowire-go, pinned: there the body fits under a
    // raised limit and is written unpacked. Here the value is read with
    // prost-reflect's `DynamicMessage::decode`, whose recursion limit is
    // prost's fixed 100, so a value nested past 100 can't be unpacked at any
    // writer limit and stays raw. Raw always conforms (draft -01 § Any
    // Blocks), and it reads back. This fails the day prost reads deeper;
    // then assert `@type`, as the reference does.
    let opts = MarshalOptions {
        max_nesting_depth: 200,
        ..opts
    };
    let out = marshal(&m, &d, opts).expect("raw Any");
    assert!(!out.contains("@type"), "{out}");
    let read = UnmarshalOptions {
        type_resolver: Some(res),
        ..reader(200)
    };
    assert!(same(&unmarshal(&out, &d, read).unwrap(), &m));
}

/// An Any that sits past the limit itself has no room even for its raw block.
#[test]
fn any_past_the_limit_is_refused() {
    let p = pool();
    let d = t_desc(p);
    let mut inner = DynamicMessage::new(d.clone());
    inner.set_field_by_name("v", Value::I32(1));
    let m = deep(&d, t_any(&d, &inner), MAX_NESTING_DEPTH);
    let opts = MarshalOptions {
        type_resolver: Some(resolver(p)),
        ..Default::default()
    };
    let err = marshal(&m, &d, opts).unwrap_err();
    assert!(err.msg.contains(DEPTH_ERROR), "{}", err.msg);
}

/// The writer stops descending at the limit, so a chain far past it is
/// refused without walking it.
#[test]
fn a_deep_chain_stops_at_the_limit() {
    let p = pool();
    let d = t_desc(p);
    let m = deep(&d, DynamicMessage::new(d.clone()), 20_000);
    let err = marshal(&m, &d, MarshalOptions::default()).unwrap_err();
    assert!(err.msg.contains(DEPTH_ERROR), "{}", err.msg);
    // Dropping the chain recurses once per level; leak it instead.
    std::mem::forget(m);
}

/// The raised limit does unpack an Any whose value prost can read: 60
/// levels down, a 60-level value needs 121, so it stays raw at the default
/// and is written as a body at 200.
#[test]
fn any_body_fits_under_a_raised_limit() {
    let p = pool();
    let d = t_desc(p);
    let res = resolver(p);
    let m = deep(
        &d,
        t_any(&d, &deep(&d, DynamicMessage::new(d.clone()), 60)),
        60,
    );
    let opts = MarshalOptions {
        type_resolver: Some(res),
        ..Default::default()
    };
    let out = marshal(&m, &d, opts).expect("raw Any");
    assert!(!out.contains("@type"), "{out}");

    let opts = MarshalOptions {
        max_nesting_depth: 200,
        ..opts
    };
    let out = marshal(&m, &d, opts).expect("unpacked Any");
    assert!(out.contains("@type"), "{out}");
    let read = UnmarshalOptions {
        type_resolver: Some(res),
        ..reader(200)
    };
    assert!(same(&unmarshal(&out, &d, read).unwrap(), &m));
}
