// SPDX-License-Identifier: MIT
// Copyright (c) 2026 TrendVidia, LLC.
//! Node spans (#131), as protowire-go's `EntrySpan` and `ValueSpan`.
//!
//! `tolerant_oracle.rs` holds every entry's and value's span to
//! protowire-go's on each of its cases. The tests here are protowire-go's
//! own span tests, ported, and read the spans back as source text.

use protowire_pxf::{
    entry_span, format, parse, parse_tolerant, value_span, Entry, Position, Value,
};

fn text(doc: &str, (start, end): (Position, Position)) -> &str {
    &doc[start.offset..end.offset]
}

/// protowire-go's `TestAnyTypeEntry_ParseAndFormat`: a type entry spans
/// `@type = "<url>"`, without its comments.
#[test]
fn a_type_entry_spans_its_url() {
    let doc =
        "payload {\n  # the type\n  @type = \"anyentry.v1.Leaf\" # trailing\n  tag = \"t\"\n}\n";
    let d = parse(doc).unwrap();
    let Entry::Block(blk) = &d.entries[0] else {
        panic!("{:?}", d.entries[0]);
    };
    let Entry::TypeEntry(te) = &blk.entries[0] else {
        panic!("{:?}", blk.entries[0]);
    };
    assert_eq!(te.url, "anyentry.v1.Leaf");
    assert_eq!(te.leading_comments[0].text, "# the type");
    assert_eq!(te.trailing_comment, "# trailing");
    assert_eq!(
        text(doc, entry_span(&blk.entries[0])),
        r#"@type = "anyentry.v1.Leaf""#
    );
    assert_eq!(text(doc, entry_span(&d.entries[0])), &doc[..doc.len() - 1]);
    assert_eq!(format(&d), doc);
}

/// protowire-go's `TestParseTolerantDepthRecoverySpan`: a list skipped past
/// the depth limit ends just past its own `]`, not at the token after it.
#[test]
fn a_list_skipped_past_the_depth_limit_ends_at_its_bracket() {
    let src = format!("xs = {}1{}\nnext = 1\n", "[".repeat(101), " ]".repeat(101));
    let (doc, errs) = parse_tolerant(&src);
    assert_eq!(errs.len(), 1, "{errs:?}");
    assert!(errs[0].msg.contains("MaxNestingDepth"), "{}", errs[0]);
    let Entry::Assignment(a) = &doc.entries[0] else {
        panic!("{:?}", doc.entries[0]);
    };
    let mut v = &a.value;
    for _ in 0..100 {
        let Value::List(l) = v else { panic!("{v:?}") };
        v = &l.elements[0];
    }
    assert!(matches!(v, Value::List(_)), "{v:?}");
    let (_, end) = value_span(v);
    assert_eq!(end.offset, src.find(']').unwrap() + 1);
}

/// Each kind of entry and value spans exactly its source text: key to the
/// value's end for an entry, the literal with its suffix for a scalar, the
/// brackets for a list or block.
#[test]
fn every_node_spans_its_source_text() {
    let doc = concat!(
        "s = \"x\" # c\n",
        "i = 12p53\n",
        "f = 1.5e3\n",
        "bo = true\n",
        "by = b\"AQI=\"\n",
        "n = null\n",
        "id = RED\n",
        "ts = 2024-01-02T03:04:05Z\n",
        "du = 1h30m\n",
        "l = [1, [2], { k: v }]\n",
        "m { \"a\": 1 }\n",
        "\"quoted\" = {}\n",
    );
    let d = parse(doc).unwrap();
    let mut got = Vec::new();
    for e in &d.entries {
        got.push(text(doc, entry_span(e)).to_string());
        let v = match e {
            Entry::Assignment(a) => &a.value,
            Entry::Block(b) => {
                for inner in &b.entries {
                    got.push(text(doc, entry_span(inner)).to_string());
                }
                continue;
            }
            other => panic!("{other:?}"),
        };
        got.push(text(doc, value_span(v)).to_string());
        if let Value::List(l) = v {
            for el in &l.elements {
                got.push(text(doc, value_span(el)).to_string());
            }
        }
    }
    assert_eq!(
        got,
        [
            "s = \"x\"",
            "\"x\"",
            "i = 12p53",
            "12p53",
            "f = 1.5e3",
            "1.5e3",
            "bo = true",
            "true",
            "by = b\"AQI=\"",
            "b\"AQI=\"",
            "n = null",
            "null",
            "id = RED",
            "RED",
            "ts = 2024-01-02T03:04:05Z",
            "2024-01-02T03:04:05Z",
            "du = 1h30m",
            "1h30m",
            "l = [1, [2], { k: v }]",
            "[1, [2], { k: v }]",
            "1",
            "[2]",
            "{ k: v }",
            "m { \"a\": 1 }",
            "\"a\": 1",
            "\"quoted\" = {}",
            "{}",
        ]
    );
}

/// A placeholder spans nothing: it starts and ends where the value belongs.
#[test]
fn a_placeholder_spans_nothing() {
    let (doc, _) = parse_tolerant("a = \nb = 1\n");
    let Entry::Assignment(a) = &doc.entries[0] else {
        panic!("{:?}", doc.entries[0]);
    };
    let (start, end) = value_span(&a.value);
    assert!(matches!(a.value, Value::Bad(_)), "{:?}", a.value);
    assert_eq!(start, end);
    assert_eq!(entry_span(&doc.entries[0]).1, end);
}

fn keyed_node() -> prost_reflect::MessageDescriptor {
    prost_reflect::DescriptorPool::decode(&include_bytes!("../testdata/keyed/keyed.binpb")[..])
        .expect("keyed.binpb")
        .get_message_by_name("keyed.v1.Node")
        .expect("keyed.v1.Node")
}

/// `canonicalize_keyed` rewrites an assignment or a list element as a block
/// that keeps the source span of what it replaced, as protowire-go's
/// `CanonicalizeKeyed` does.
#[test]
fn a_canonicalized_keyed_block_keeps_its_source_span() {
    let desc = keyed_node();
    for (src, want) in [
        (
            "children = {\n  greeting = { type = \"Label\" }\n}\n",
            vec![
                "children = {\n  greeting = { type = \"Label\" }\n}",
                "greeting = { type = \"Label\" }",
            ],
        ),
        (
            "children = [\n  { id = \"g\" },\n  { type = \"L\" id = \"h\" }\n]\n",
            vec![
                "children = [\n  { id = \"g\" },\n  { type = \"L\" id = \"h\" }\n]",
                "{ id = \"g\" }",
                "{ type = \"L\" id = \"h\" }",
            ],
        ),
    ] {
        let mut doc = parse(src).unwrap();
        protowire_pxf::canonicalize_keyed(&mut doc, &desc);
        let Entry::Block(b) = &doc.entries[0] else {
            panic!("{src}: {:?}", doc.entries[0]);
        };
        let mut got = vec![text(src, entry_span(&doc.entries[0]))];
        got.extend(b.entries.iter().map(|e| text(src, entry_span(e))));
        assert_eq!(got, want, "{src}");
    }
}
