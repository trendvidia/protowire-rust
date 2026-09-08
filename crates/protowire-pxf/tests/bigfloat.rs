// SPDX-License-Identifier: MIT
// Copyright (c) 2026 TrendVidia, LLC.
//! `pxf.BigFloat` literals, byte for byte against the reference
//! (protowire-rust#39). `testdata/bigfloat-oracle.tsv` is what
//! protowire-go v1.6.0 wrote for each literal — mantissa, exponent, prec,
//! sign — and the text it marshalled back; the header says how it was
//! produced. STABILITY.md promise 2 makes those bytes the contract, and the
//! reference's conversion is not correctly rounded, so agreement here means
//! the port mirrors `math/big`'s steps, not merely the nearest value.

use prost::Message as _;
use prost_reflect::{DescriptorPool, DynamicMessage, MessageDescriptor, Value};
use protowire_pxf::{marshal, unmarshal, MarshalOptions, UnmarshalOptions};

const FDS: &[u8] = include_bytes!("../testdata/bignum-test.binpb");
const ORACLE: &str = include_str!("../testdata/bigfloat-oracle.tsv");

fn demo() -> MessageDescriptor {
    DescriptorPool::decode(FDS)
        .expect("decode bignum-test.binpb")
        .get_message_by_name("bignum_test.v1.BigNumDemo")
        .expect("BigNumDemo")
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

fn fields(m: &DynamicMessage) -> (String, i32, u32, bool) {
    let mantissa = match m.get_field_by_name("mantissa").map(|v| v.into_owned()) {
        Some(Value::Bytes(b)) => hex(&b),
        _ => String::new(),
    };
    let exponent = match m.get_field_by_name("exponent").map(|v| v.into_owned()) {
        Some(Value::I32(n)) => n,
        _ => 0,
    };
    let prec = match m.get_field_by_name("prec").map(|v| v.into_owned()) {
        Some(Value::U32(n)) => n,
        _ => 0,
    };
    let neg = matches!(
        m.get_field_by_name("negative").map(|v| v.into_owned()),
        Some(Value::Bool(true))
    );
    (mantissa, exponent, prec, neg)
}

#[test]
fn every_oracle_row_matches_the_reference() {
    let desc = demo();
    let fd = desc.get_field_by_name("big_float_field").unwrap();
    let mut rows = 0;
    for line in ORACLE.lines() {
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let cols: Vec<&str> = line.split('\t').collect();
        let lit = cols[0];
        let doc = format!("big_float_field = {lit}");
        rows += 1;
        if cols[1] == "ERR" {
            let want = cols[2].split_once(": ").map_or(cols[2], |x| x.1);
            let err = unmarshal(&doc, &desc, UnmarshalOptions::default())
                .expect_err(&format!("{lit}: the reference rejects it"));
            assert_eq!(err.msg, want, "{lit}");
            continue;
        }
        let m = unmarshal(&doc, &desc, UnmarshalOptions::default())
            .unwrap_or_else(|e| panic!("{lit}: {e}"));
        let Value::Message(sub) = m.get_field(&fd).into_owned() else {
            panic!("{lit}: not a message")
        };
        let (mant, exp, prec, neg) = fields(&sub);
        assert_eq!(mant, cols[1], "{lit}: mantissa");
        assert_eq!(exp.to_string(), cols[2], "{lit}: exponent");
        assert_eq!(prec.to_string(), cols[3], "{lit}: prec");
        assert_eq!(neg.to_string(), cols[4], "{lit}: negative");
        // The marshalled line, exactly.
        let out = marshal(&m, &desc, MarshalOptions::default());
        assert_eq!(
            out,
            format!("big_float_field = {}\n", cols[5].trim_start_matches("f = ")),
            "{lit}: text"
        );
        // And the text reads back to the same bytes.
        let back = unmarshal(&out, &desc, UnmarshalOptions::default())
            .unwrap_or_else(|e| panic!("{lit}: re-reading {out:?}: {e}"));
        assert_eq!(
            back.encode_to_vec(),
            m.encode_to_vec(),
            "{lit}: pb bytes after re-read"
        );
    }
    assert_eq!(rows, 31, "the oracle has 31 rows");
}
