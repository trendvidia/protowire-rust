// SPDX-License-Identifier: MIT
// Copyright (c) 2026 TrendVidia, LLC.
//! Conformance against the reference. `testdata/conformance/presence-oracle.tsv`
//! is what protowire-go did with each document, over the same descriptor sets:
//! its verdict, its error, its message bytes and its null paths, from both
//! `UnmarshalFull` and `Unmarshal`. Its header names the commit, and
//! `scripts/gen-presence-oracle.sh` regenerates it. The documents cover
//! `(pxf.default)` and `(pxf.required)` in every message body
//! (protowire-go#161) and a keyed entry's name (protowire-go#214, #215).
//! `timestamp-oracle.tsv`, made the same way, holds timestamp literals to
//! what protowire-go's lexer and `time.Parse` decide (#118), and
//! `message-forms-oracle.tsv` the error for a token no message value opens
//! (#147).

use std::collections::HashMap;

use prost::Message as _;
use prost_reflect::{DescriptorPool, DynamicMessage, ReflectMessage, Value};
use protowire_pxf::{unmarshal, unmarshal_full, PoolResolver, UnmarshalOptions};

const ORACLE: &str = include_str!("../testdata/conformance/presence-oracle.tsv");
const TIMESTAMP_ORACLE: &str = include_str!("../testdata/conformance/timestamp-oracle.tsv");
const MESSAGE_FORMS_ORACLE: &str = include_str!("../testdata/conformance/message-forms-oracle.tsv");

fn pools() -> HashMap<&'static str, DescriptorPool> {
    let sets: [(&str, &[u8]); 6] = [
        ("test.binpb", include_bytes!("../testdata/test.binpb")),
        (
            "bignum-test.binpb",
            include_bytes!("../testdata/bignum-test.binpb"),
        ),
        (
            "secret-literal-test.binpb",
            include_bytes!("../testdata/secret-literal-test.binpb"),
        ),
        (
            "element-defaults-test.binpb",
            include_bytes!("../testdata/element-defaults-test.binpb"),
        ),
        (
            "keyed-entry-name-test.binpb",
            include_bytes!("../testdata/keyed-entry-name-test.binpb"),
        ),
        (
            "keyed/keyed.binpb",
            include_bytes!("../testdata/keyed/keyed.binpb"),
        ),
    ];
    sets.into_iter()
        .map(|(name, fds)| (name, DescriptorPool::decode(fds).expect(name)))
        .collect()
}

fn unhex(s: &str) -> Vec<u8> {
    (0..s.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(&s[i..i + 2], 16).expect("hex"))
        .collect()
}

/// Rewrite every packed `Any` body in `m` in this encoder's field order. Go
/// writes oneof members last and this port writes in tag order, which are the
/// same message, so the body is compared unpacked. A body whose type the pool
/// doesn't hold stays as it is, and so has to match byte for byte. Fields
/// holding nothing are dropped, as they are on the wire.
fn normalize(m: &mut DynamicMessage, pool: &DescriptorPool) {
    if m.descriptor().full_name() == "google.protobuf.Any" {
        let url = m.get_field_by_name("type_url").unwrap();
        let url = url
            .as_str()
            .unwrap()
            .rsplit('/')
            .next()
            .unwrap()
            .to_string();
        let bytes = m
            .get_field_by_name("value")
            .unwrap()
            .as_bytes()
            .unwrap()
            .clone();
        if let Some(inner) = pool.get_message_by_name(&url) {
            let mut body = DynamicMessage::decode(inner, bytes).expect("packed body");
            normalize(&mut body, pool);
            m.set_field_by_name("value", Value::Bytes(body.encode_to_vec().into()));
        }
        return;
    }
    for fd in m.descriptor().fields() {
        if !m.has_field(&fd) {
            // `items = []` leaves an empty list here, nothing on the wire.
            m.clear_field(&fd);
            continue;
        }
        match m.get_field_mut(&fd) {
            Value::Message(sub) => normalize(sub, pool),
            Value::List(items) => items.iter_mut().for_each(|v| {
                if let Value::Message(sub) = v {
                    normalize(sub, pool);
                }
            }),
            Value::Map(map) => map.values_mut().for_each(|v| {
                if let Value::Message(sub) = v {
                    normalize(sub, pool);
                }
            }),
            _ => {}
        }
    }
}

/// One decode against the oracle's three columns for it. Bytes are compared
/// as decoded messages, because a map's order on the wire is not the contract.
fn check(
    pool: &DescriptorPool,
    what: &str,
    got: Result<(DynamicMessage, Vec<String>), protowire_pxf::PxfError>,
    want: &[&str],
    desc: &prost_reflect::MessageDescriptor,
) {
    match (want[0], got) {
        ("ERR", Err(e)) => assert_eq!(e.to_string(), want[1], "{what}"),
        ("ERR", Ok(_)) => panic!("{what}: the reference rejects it: {}", want[1]),
        ("OK", Err(e)) => panic!("{what}: the reference accepts it: {e}"),
        ("OK", Ok((mut m, nulls))) => {
            let mut expected = DynamicMessage::decode(desc.clone(), unhex(want[1]).as_slice())
                .unwrap_or_else(|e| panic!("{what}: oracle bytes: {e}"));
            normalize(&mut m, pool);
            normalize(&mut expected, pool);
            assert!(m == expected, "{what}\n got: {m}\nwant: {expected}");
            assert_eq!(nulls.join(","), want[2], "{what}: null paths");
        }
        (other, _) => panic!("{what}: oracle verdict {other:?}"),
    }
}

#[test]
fn every_document_meets_the_reference() {
    // A positive marker: the rows ran, and both verdicts are among them.
    assert_eq!(run(ORACLE), (63, 22));
}

/// A token that opens no value of a message field: the error names the
/// forms the field takes and the token it got (#147).
#[test]
fn every_wrong_token_for_a_message_reads_as_the_reference() {
    assert_eq!(run(MESSAGE_FORMS_ORACLE), (27, 27));
}

#[test]
fn every_timestamp_literal_meets_the_reference() {
    assert_eq!(run(TIMESTAMP_ORACLE), (66, 34));
}

/// Checks every row of `oracle`, and returns how many rows there were and
/// how many the reference rejected.
fn run(oracle: &str) -> (usize, usize) {
    let pools = pools();
    let (mut rows, mut rejected) = (0, 0);
    for line in oracle.lines().filter(|l| !l.starts_with('#')) {
        let c: Vec<&str> = line.split('\t').collect();
        assert_eq!(c.len(), 9, "{line}");
        let (set, message, doc) = (c[0], c[1], c[2]);
        let pool = &pools[set];
        let desc = pool.get_message_by_name(message).expect(message);
        let resolver = PoolResolver(pool);
        let opts = || UnmarshalOptions {
            type_resolver: Some(&resolver),
            ..Default::default()
        };
        let full = unmarshal_full(doc, &desc, opts()).map(|(m, presence)| {
            let mut nulls: Vec<String> = presence.null_paths().map(str::to_string).collect();
            nulls.sort();
            (m, nulls)
        });
        check(
            pool,
            &format!("{message} full: {doc}"),
            full,
            &c[3..6],
            &desc,
        );
        let plain = unmarshal(doc, &desc, opts()).map(|m| (m, Vec::new()));
        check(
            pool,
            &format!("{message} plain: {doc}"),
            plain,
            &c[6..9],
            &desc,
        );
        rows += 1;
        rejected += usize::from(c[3] == "ERR");
    }
    (rows, rejected)
}
