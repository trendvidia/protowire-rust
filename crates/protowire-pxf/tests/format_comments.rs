// SPDX-License-Identifier: MIT
// Copyright (c) 2026 TrendVidia, LLC.
//! Comments that precede no entry, at the end of a document or of a block
//! and around dataset rows, and comments after a value on its line,
//! survive `parse` then `format` in place (#119). A port of protowire-go
//! v1.17.0's `format_comments_test.go` (protowire-go#279), except its
//! `DatasetReader` case, which is #117's. The `same_line_comments` rows
//! are what Go's `FormatDocument` wrote for each document.

use prost_reflect::{DescriptorPool, MessageDescriptor};
use protowire_pxf::{canonicalize_keyed, format, parse, Entry};

fn round_trip(src: &str) -> String {
    format(&parse(src).unwrap_or_else(|e| panic!("{e}\n{src}")))
}

#[test]
fn comments_at_the_end() {
    for (name, src, want) in [
        (
            "after an entry",
            "@type a.v1.M\nname = \"n\"\n# end of doc\n",
            "@type a.v1.M\n\nname = \"n\"\n# end of doc\n",
        ),
        (
            "after a directive",
            "@type a.v1.M\n@flag\n# end of doc\n",
            "",
        ),
        (
            "after a row",
            "@dataset t.v1.T (id)\n(1)\n# end of doc\n",
            "",
        ),
        (
            "a run of comments",
            "@type a.v1.M\nname = \"n\" # inline\n# a\n// b\n/* c */\n",
            "@type a.v1.M\n\nname = \"n\" # inline\n# a\n// b\n/* c */\n",
        ),
        (
            "after @type alone",
            "@type a.v1.M\n# only\n",
            "@type a.v1.M\n\n# only\n",
        ),
    ] {
        let want = if want.is_empty() { src } else { want };
        assert_eq!(round_trip(src), want, "{name}");
    }

    let doc = parse("@type a.v1.M\nname = \"n\"\n# end of doc\n").unwrap();
    assert_eq!(doc.trailing_comments.len(), 1);
    assert_eq!(doc.trailing_comments[0].text, "# end of doc");
    let Entry::Assignment(a) = &doc.entries[0] else {
        panic!("{:?}", doc.entries[0])
    };
    assert!(a.leading_comments.is_empty());
}

#[test]
fn comments_before_closing_brace() {
    for (name, src) in [
        // Before, this comment moved out of m, onto y.
        (
            "block then entry",
            "@type a.v1.M\n\nm {\n  k: 1\n  # end of m\n}\ny = 2\n",
        ),
        (
            "last block",
            "@type a.v1.M\n\nb {\n  x = 1\n  # end of b\n}\n",
        ),
        ("empty block", "@type a.v1.M\n\nb {\n  # empty\n}\n"),
        (
            "block value",
            "@type a.v1.M\n\nb = {\n  x = 1\n  # end of b\n}\n",
        ),
        (
            "nested",
            "@type a.v1.M\n\na {\n  b {\n    x = 1\n    # end of b\n  }\n  # end of a\n}\n",
        ),
        (
            "after the brace stays after",
            "@type a.v1.M\n\na {\n  x = 1\n}\n# about y\ny = 2\n",
        ),
        (
            "block cell",
            "@dataset t.v1.T (id, m)\n(1, {\n  x = 1\n  # end of m\n})\n",
        ),
    ] {
        assert_eq!(round_trip(src), src, "{name}");
    }

    let doc = parse("@type a.v1.M\nm {\n  k: 1\n  # end of m\n}\ny = 2\n").unwrap();
    let Entry::Block(m) = &doc.entries[0] else {
        panic!("{:?}", doc.entries[0])
    };
    assert_eq!(m.trailing_comments.len(), 1);
    assert_eq!(m.trailing_comments[0].text, "# end of m");
    let Entry::Assignment(y) = &doc.entries[1] else {
        panic!("{:?}", doc.entries[1])
    };
    assert!(y.leading_comments.is_empty());
}

#[test]
fn dataset_row_comments() {
    let src = "@dataset t.v1.T (id)\n# first\n(1)\n# between\n(2)\n# end\n";
    assert_eq!(round_trip(src), src);

    // A row is written on one line, so a comment inside it moves before it.
    assert_eq!(
        round_trip("@dataset t.v1.T (a, b)\n(1, # one\n 2)\n(3, 4)\n"),
        "@dataset t.v1.T (a, b)\n# one\n(1, 2)\n(3, 4)\n"
    );

    let doc = parse(src).unwrap();
    let rows = &doc.datasets[0].rows;
    assert_eq!(rows.len(), 2);
    assert_eq!(rows[1].leading_comments.len(), 1);
    assert_eq!(rows[1].leading_comments[0].text, "# between");
    assert_eq!(doc.trailing_comments.len(), 1);
    assert_eq!(doc.trailing_comments[0].text, "# end");
}

#[test]
fn same_line_comments() {
    for (src, want) in [
        (
            "@type a.v1.M\n\nany {\n  @type = \"x/y\" # t\n  a = 1\n}\n",
            "@type a.v1.M\n\nany {\n  @type = \"x/y\" # t\n  a = 1\n}\n",
        ),
        (
            "@type a.v1.M\n\nm {\n  k: 1 # inline\n}\n",
            "@type a.v1.M\n\nm {\n  k: 1 # inline\n}\n",
        ),
        // A comment inside a list is written after the list.
        (
            "@type a.v1.M\n\nl = [1, # in list\n 2]\n",
            "@type a.v1.M\n\nl = [\n  1,\n  2\n]\n# in list\n",
        ),
        (
            "@type a.v1.M\n\nb = {\n  x = 1\n} # after brace\ny = 2\n",
            "@type a.v1.M\n\nb = {\n  x = 1\n} # after brace\ny = 2\n",
        ),
        // A block entry has no same-line comment; it ends the document.
        (
            "@type a.v1.M\n\nb {\n  x = 1\n} # after block\n",
            "@type a.v1.M\n\nb {\n  x = 1\n}\n# after block\n",
        ),
        // Only the first comment on the line trails the value.
        (
            "@type a.v1.M\n\nname = \"n\" /* c */ # d\n",
            "@type a.v1.M\n\nname = \"n\" /* c */\n# d\n",
        ),
    ] {
        assert_eq!(round_trip(src), want, "{src:?}");
    }
}

fn keyed_node() -> MessageDescriptor {
    DescriptorPool::decode(&include_bytes!("../testdata/keyed/keyed.binpb")[..])
        .expect("keyed.binpb")
        .get_message_by_name("keyed.v1.Node")
        .expect("keyed.v1.Node")
}

// canonicalize_keyed keeps a block's closing comments when it rewrites the
// block, and the comments of a redundant key assignment it drops.
#[test]
fn canonicalize_keyed_keeps_closing_comments() {
    let desc = keyed_node();
    for (name, src, want) in [
        (
            "dropped last key assignment",
            "@type keyed.v1.Node\n\nchildren {\n  greeting {\n    type = \"Label\"\n    # about the key\n    id = \"greeting\"\n  }\n  # end of children\n}\n",
            "@type keyed.v1.Node\n\nchildren {\n  greeting {\n    type = \"Label\"\n    # about the key\n  }\n  # end of children\n}\n",
        ),
        (
            "assignment form",
            "@type keyed.v1.Node\n\nchildren = {\n  greeting = {\n    type = \"Label\"\n    # end of greeting\n  }\n  # end of children\n}\n",
            "@type keyed.v1.Node\n\nchildren {\n  greeting {\n    type = \"Label\"\n    # end of greeting\n  }\n  # end of children\n}\n",
        ),
        (
            "anonymous list",
            "@type keyed.v1.Node\n\nchildren = [\n  {\n    type = \"L\"\n    # about the key\n    id = \"g\"\n  },\n  {\n    id = \"h\"\n    # end of h\n  }\n]\n",
            "@type keyed.v1.Node\n\nchildren {\n  g {\n    type = \"L\"\n    # about the key\n  }\n  h {\n    # end of h\n  }\n}\n",
        ),
    ] {
        let mut doc = parse(src).unwrap();
        canonicalize_keyed(&mut doc, &desc);
        let out = format(&doc);
        assert_eq!(out, want, "{name}");

        let mut again = parse(&out).unwrap();
        canonicalize_keyed(&mut again, &desc);
        assert_eq!(format(&again), out, "{name}: formatting is a fixed point");
    }
}
