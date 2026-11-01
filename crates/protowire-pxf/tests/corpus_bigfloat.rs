// SPDX-License-Identifier: MIT
// Copyright (c) 2026 TrendVidia, LLC.
//! A `pxf.BigFloat`'s text form (draft -01 § PXF Annotations; protowire#406,
//! #410, #281). `testdata/bigfloat/` is vendored from trendvidia/protowire
//! at e9e0a17; its `generate.py` computes every expected value with exact
//! rational arithmetic, independently of any port.

mod common;

use common::{dir, message, pool};
use prost::Message as _;
use prost_reflect::{DynamicMessage, MessageDescriptor};
use protowire_pxf::{marshal, parse, unmarshal, Limits, MarshalOptions, UnmarshalOptions};

const FDS: &[u8] = include_bytes!("../testdata/bigfloat/bigfloat.fds");

fn holder() -> MessageDescriptor {
    message(&pool(FDS), "bf.v1.H")
}

/// Every case's stem, sorted.
fn cases() -> Vec<String> {
    let mut stems: Vec<String> = std::fs::read_dir(dir("bigfloat"))
        .unwrap()
        .map(|e| e.unwrap().path())
        .filter(|p| p.extension().is_some_and(|x| x == "pxf"))
        .map(|p| p.file_stem().unwrap().to_string_lossy().into_owned())
        .collect();
    stems.sort();
    stems
}

fn read(stem: &str, ext: &str) -> Vec<u8> {
    std::fs::read(dir("bigfloat").join(format!("{stem}.{ext}"))).unwrap()
}

/// The document's body, without its comment lines.
fn entry_text(doc: &str) -> String {
    doc.lines()
        .filter(|l| !l.is_empty() && !l.starts_with('#') && !l.starts_with("@type"))
        .collect::<Vec<_>>()
        .join("\n")
}

#[test]
fn every_case_meets_its_verdict() {
    let d = holder();
    let all = cases();
    let count = |p: &str| all.iter().filter(|c| c.starts_with(p)).count();
    assert_eq!(all.len(), 35, "the README lists thirty-five cases");
    assert_eq!((count("enc-"), count("dec-"), count("err-")), (13, 18, 4));
    let mut problems = Vec::new();
    for stem in &all {
        let text = String::from_utf8(read(stem, "pxf")).unwrap();
        if stem.starts_with("err-") {
            parse(&text).unwrap_or_else(|e| panic!("{stem}: a suffix lexes: {e}"));
            if unmarshal(&text, &d, UnmarshalOptions::default()).is_ok() {
                problems.push(format!("{stem}: accepted"));
            }
        } else if stem.starts_with("dec-") {
            match unmarshal(&text, &d, UnmarshalOptions::default()) {
                Ok(m) if m.encode_to_vec() == read(stem, "binpb") => {}
                Ok(m) => problems.push(format!("{stem}: {:?}", m.encode_to_vec())),
                Err(e) => problems.push(format!("{stem}: {e}")),
            }
        } else {
            let msg = DynamicMessage::decode(d.clone(), read(stem, "binpb").as_slice()).unwrap();
            match marshal(&msg, &d, MarshalOptions::default()) {
                Ok(out) if entry_text(&out) == entry_text(&text) => {}
                Ok(out) => problems.push(format!(
                    "{stem}: wrote {:?}, want {:?}",
                    entry_text(&out),
                    entry_text(&text)
                )),
                Err(e) => problems.push(format!("{stem}: {e}")),
            }
        }
    }
    assert!(problems.is_empty(), "\n{}", problems.join("\n"));
}

/// What an encoder writes reads back to the message it came from: the
/// point of N digits and the suffix.
#[test]
fn every_written_literal_reads_back_exactly() {
    let d = holder();
    for stem in cases().iter().filter(|c| c.starts_with("enc-")) {
        if ["enc-wide-mantissa-prec53"].contains(&stem.as_str()) {
            continue; // the mantissa is wider than prec: rounded on write
        }
        let bytes = read(stem, "binpb");
        let msg = DynamicMessage::decode(d.clone(), bytes.as_slice()).unwrap();
        let text = marshal(&msg, &d, MarshalOptions::default()).unwrap();
        let back = unmarshal(&text, &d, UnmarshalOptions::default()).unwrap();
        let again = marshal(&back, &d, MarshalOptions::default()).unwrap();
        assert_eq!(text, again, "{stem}");
    }
}

#[test]
fn a_suffix_error_names_the_field_on_every_path() {
    let d = holder();
    let o = UnmarshalOptions::default();
    let e = unmarshal("count = 5p64\n", &d, o).unwrap_err();
    assert!(e.msg.contains("not on field \"count\""), "{}", e.msg);
    let e = unmarshal("count = 5.5p64\n", &d, o).unwrap_err();
    assert!(e.msg.contains("precision suffix"), "{}", e.msg);
}

#[test]
fn a_key_cannot_carry_a_suffix() {
    let doc = "m = { 1p2: 3 }\n";
    assert!(parse(doc).is_err());
}

#[test]
fn format_keeps_a_suffix() {
    let doc = parse("f = 1.5p53\ng = [1p8]\n").unwrap();
    assert_eq!(protowire_pxf::format(&doc), "f = 1.5p53\ng = [\n  1p8\n]\n");
}

#[test]
fn the_exponent_bound_is_per_call_and_applies_after_rounding() {
    let d = holder();
    let o = UnmarshalOptions::default();
    unmarshal("bf = 1e4932\n", &d, o).unwrap();
    let e = unmarshal("bf = 1e4933\n", &d, o).unwrap_err();
    assert!(e.msg.contains("MaxBigFloatExponent=16384"), "{}", e.msg);
    let e = unmarshal("bf = 1e-4933\n", &d, o).unwrap_err();
    assert!(e.msg.contains("MaxBigFloatExponent"), "{}", e.msg);
    let tight = UnmarshalOptions {
        limits: Limits {
            max_big_float_exponent: 10,
            ..Limits::default()
        },
        ..Default::default()
    };
    unmarshal("bf = 1000\n", &d, tight).unwrap();
    assert!(unmarshal("bf = 2000\n", &d, tight).is_err());
}

#[test]
fn a_producer_fails_on_a_value_past_the_bound() {
    let d = holder();
    let bf_desc = message(&pool(FDS), "pxf.BigFloat");
    for (exponent, ok) in [
        (16383, true),
        (16384, false),
        (-16385, true),
        (-16386, false),
    ] {
        let mut bf = DynamicMessage::new(bf_desc.clone());
        bf.set_field_by_name("mantissa", prost_reflect::Value::Bytes(vec![1].into()));
        bf.set_field_by_name("exponent", prost_reflect::Value::I32(exponent));
        bf.set_field_by_name("prec", prost_reflect::Value::U32(64));
        let mut msg = DynamicMessage::new(d.clone());
        msg.set_field_by_name("bf", prost_reflect::Value::Message(bf));
        let got = marshal(&msg, &d, MarshalOptions::default());
        assert_eq!(got.is_ok(), ok, "{exponent}: {got:?}");
        if !ok {
            assert!(got.unwrap_err().to_string().contains("MaxBigFloatExponent"));
        }
    }
}

#[test]
fn digit_count_of_a_precision() {
    use protowire_pxf::bigfloat::big_float_digits;
    assert_eq!(big_float_digits(53), 17);
    assert_eq!(big_float_digits(64), 21);
    assert_eq!(big_float_digits(13603), 4096);
    assert_eq!(big_float_digits(13604), 4097);
    // Where an f64 product is one digit high, per the reference.
    assert_eq!(big_float_digits(146_964_308), 44_240_666);
}
