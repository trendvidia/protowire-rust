// SPDX-License-Identifier: MIT
// Copyright (c) 2026 TrendVidia, LLC.
//! HARDENING.md § Mandatory limits, per call (protowire-rust#33): every
//! limit but `MaxVarintBytes` is configurable per call, the constant being
//! the default. Each test shows a lowered value rejecting input the
//! default accepts, and the message-size test shows the default rejecting
//! 64 MiB + 1. Mirrors protowire-go's `limits_size_test.go`.

use std::io::Cursor;

use prost_reflect::{DescriptorPool, MessageDescriptor};
use protowire_pxf::{
    parse, parse_with_limits, unmarshal, unmarshal_full, DatasetReader, Limits, UnmarshalOptions,
    MAX_MESSAGE_SIZE,
};

const TEST_FDS: &[u8] = include_bytes!("../testdata/test.binpb");

fn all_types() -> MessageDescriptor {
    DescriptorPool::decode(TEST_FDS)
        .expect("decode test.binpb")
        .get_message_by_name("test.v1.AllTypes")
        .expect("test.v1.AllTypes")
}

fn opts(limits: Limits) -> UnmarshalOptions<'static> {
    UnmarshalOptions {
        limits,
        ..Default::default()
    }
}

fn err_of(doc: &str, limits: Limits) -> String {
    unmarshal(doc, &all_types(), opts(limits))
        .expect_err("must reject")
        .msg
}

#[test]
fn max_message_size_default_rejects_64_mib_plus_one_and_accepts_64_mib() {
    let desc = all_types();
    let prefix = "string_field = \"";
    let suffix = "\"";
    // Exactly MAX_MESSAGE_SIZE + 1 bytes.
    let body = "a".repeat(MAX_MESSAGE_SIZE + 1 - prefix.len() - suffix.len());
    let doc = format!("{prefix}{body}{suffix}");
    assert_eq!(doc.len(), MAX_MESSAGE_SIZE + 1);
    let want = format!(
        "input of {} bytes exceeds MaxMessageSize={}",
        doc.len(),
        MAX_MESSAGE_SIZE
    );
    assert_eq!(err_of(&doc, Limits::default()), want);
    assert_eq!(
        unmarshal_full(&doc, &desc, UnmarshalOptions::default())
            .unwrap_err()
            .msg,
        want,
        "the full variant"
    );
    assert_eq!(parse(&doc).unwrap_err().msg, want, "the AST parser");

    // One byte fewer is at the bound and decodes.
    let doc = format!("{}{}", &doc[..MAX_MESSAGE_SIZE - 1], suffix);
    assert_eq!(doc.len(), MAX_MESSAGE_SIZE);
    unmarshal(&doc, &desc, UnmarshalOptions::default()).expect("64 MiB is within the limit");
}

#[test]
fn max_message_size_lowered_per_call() {
    let desc = all_types();
    let doc = "string_field = \"hello, world\"";
    unmarshal(doc, &desc, UnmarshalOptions::default()).expect("default");
    let at = Limits {
        max_message_size: doc.len(),
        ..Limits::default()
    };
    unmarshal(doc, &desc, opts(at)).expect("at the bound");
    let low = Limits {
        max_message_size: 8,
        ..Limits::default()
    };
    let want = format!("input of {} bytes exceeds MaxMessageSize=8", doc.len());
    assert_eq!(err_of(doc, low), want);
    assert_eq!(
        unmarshal_full(doc, &desc, opts(low)).unwrap_err().msg,
        want,
        "the full variant"
    );
    assert_eq!(
        parse_with_limits(doc, low).unwrap_err().msg,
        want,
        "the AST parser"
    );
}

/// A b"…" literal is refused from its length before it is decoded, in the
/// decoder and in the AST parser, naming the limit.
#[test]
fn max_bytes_literal_length() {
    let desc = all_types();
    let doc = "bytes_field = b\"AAAAAAAA\""; // eight characters decode to six bytes
    unmarshal(doc, &desc, UnmarshalOptions::default()).expect("default");
    let six = Limits {
        max_bytes_literal_length: 6,
        ..Limits::default()
    };
    unmarshal(doc, &desc, opts(six)).expect("at the bound");
    parse_with_limits(doc, six).expect("at the bound, AST parser");
    let five = Limits {
        max_bytes_literal_length: 5,
        ..Limits::default()
    };
    let want = "bytes literal decodes to more than MaxBytesLiteralLength=5 bytes";
    assert!(err_of(doc, five).contains(want), "{}", err_of(doc, five));
    let e = parse_with_limits(doc, five).unwrap_err().msg;
    assert!(e.contains(want), "the AST parser: {e}");
}

/// The element count of a bracketed list and the entry count of a map
/// literal are capped, refused before the element past the bound is added.
#[test]
fn max_repeated_count() {
    let desc = all_types();
    let doc = "repeated_string = [\"a\", \"b\", \"c\", \"d\", \"e\"]\nstring_map = { a: \"1\"\n b: \"2\"\n c: \"3\" }";
    unmarshal(doc, &desc, UnmarshalOptions::default()).expect("default");
    let five = Limits {
        max_repeated_count: 5,
        ..Limits::default()
    };
    unmarshal(doc, &desc, opts(five)).expect("at the bound");
    let four = Limits {
        max_repeated_count: 4,
        ..Limits::default()
    };
    assert!(
        err_of(doc, four).contains("repeated field \"repeated_string\" exceeds MaxRepeatedCount=4"),
        "{}",
        err_of(doc, four)
    );
    let two = Limits {
        max_repeated_count: 2,
        ..Limits::default()
    };
    let map_doc = "string_map = { a: \"1\"\n b: \"2\"\n c: \"3\" }";
    assert!(
        err_of(map_doc, two).contains("map field \"string_map\" exceeds MaxRepeatedCount=2"),
        "{}",
        err_of(map_doc, two)
    );
    // Three entries at the bound decode.
    let three = Limits {
        max_repeated_count: 3,
        ..Limits::default()
    };
    unmarshal(map_doc, &desc, opts(three)).expect("at the bound");
}

/// The root is depth 0 and every `{` is one descent: a document exactly
/// `max_nesting_depth` deep decodes and one level more is rejected, in
/// the decoder and the AST parser alike.
#[test]
fn max_nesting_depth_lowered_per_call() {
    let desc = all_types();
    let nested = |n: usize| {
        let mut s = String::new();
        for _ in 0..n {
            s.push_str("nested_field {");
        }
        s.push_str(" name = \"leaf\" ");
        for _ in 0..n {
            s.push('}');
        }
        s
    };
    // test.v1.Nested has no self-reference, so nest through a block of an
    // unknown-but-skipped shape for the parser and a two-level doc for the
    // decoder.
    let two = Limits {
        max_nesting_depth: 1,
        ..Limits::default()
    };
    unmarshal(&nested(1), &desc, opts(two)).expect("one descent at the bound");
    let e = parse_with_limits(&nested(2), two).unwrap_err().msg;
    assert!(e.contains("nesting depth exceeds MaxNestingDepth=1"), "{e}");
    let e = err_of("repeated_nested = [{ name = \"x\" }]", two);
    assert!(e.contains("nesting depth exceeds MaxNestingDepth=1"), "{e}");
}

/// The stream reader caps the bytes it holds while looking for a row
/// boundary, so a stream that never ends a row cannot grow the buffer
/// without bound.
#[test]
fn dataset_reader_max_message_size() {
    let input = "@dataset test.v1.Nested (name, value)\n(\"AAPL\", 1)\n(\"MSFT\", 2)";
    let mut r = DatasetReader::new(Cursor::new(input)).expect("header");
    assert_eq!(r.by_ref().count(), 2, "default reads both rows");

    // A row far longer than the cap, never closed.
    let never_ends = format!(
        "@dataset test.v1.Nested (name, value)\n(\"{}\", 1)",
        "a".repeat(10_000)
    );
    let limits = Limits {
        max_message_size: 4096,
        ..Limits::default()
    };
    let mut r = DatasetReader::with_limits(Cursor::new(never_ends), limits).expect("header");
    let err = r.next().expect("a result").expect_err("must reject");
    assert!(err.msg.contains("MaxMessageSize=4096"), "{}", err.msg);
}
