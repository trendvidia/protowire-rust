// SPDX-License-Identifier: MIT
// Copyright (c) 2026 TrendVidia, LLC.
//! The entries of a block are separated by whitespace and nothing else
//! (draft -01 § Blocks and Map Tails; protowire#312). `testdata/blocks/` is
//! vendored from trendvidia/protowire at e9e0a17.

mod common;

use common::{fixtures, message, must_reject, pool};
use protowire_pxf::{parse, unmarshal, UnmarshalOptions};

const FDS: &[u8] = include_bytes!("../testdata/blocks/blocks.binpb");

#[test]
fn every_fixture_meets_its_verdict_with_and_without_a_schema() {
    let doc_desc = message(&pool(FDS), "blocks.v1.Doc");
    let files = fixtures("blocks");
    assert_eq!(files.len(), 7, "the README lists seven fixtures");
    for (stem, doc) in files {
        let parsed = parse(&doc);
        let decoded = unmarshal(&doc, &doc_desc, UnmarshalOptions::default());
        if must_reject(&stem) {
            assert!(parsed.is_err(), "{stem}: parse must reject");
            assert!(decoded.is_err(), "{stem}: decode must reject");
        } else {
            parsed.unwrap_or_else(|e| panic!("{stem}: parse: {e}"));
            decoded.unwrap_or_else(|e| panic!("{stem}: decode: {e}"));
        }
    }
}

/// `discard_unknown` drops a field, not the grammar: a separator inside a
/// dropped value is as wrong as one anywhere else, and the fixtures don't
/// reach this path.
#[test]
fn a_separator_in_a_discarded_value_is_rejected() {
    let doc = message(&pool(FDS), "blocks.v1.Doc");
    let opts = UnmarshalOptions {
        discard_unknown: true,
        ..Default::default()
    };
    for bad in [
        "nope { a = 1, b = 2 }\n",
        "nope { a = 1; b = 2 }\n",
        "nope = { a = 1, b = 2 }\n",
        "nope = [ { a = 1, b = 2 } ]\n",
        "nope {\n",
    ] {
        assert!(unmarshal(bad, &doc, opts).is_err(), "{bad:?} must fail");
    }
    for good in [
        "nope { a = 1 b = 2 }\n",
        "nope = { a: 1 b: 2 }\n",
        "nope = [ { a = 1 }, { b = 2 } ]\n",
        "nope = 3\n",
        "address { city = \"Berlin\" nope { x = 1 } zip = \"10115\" }\n",
    ] {
        unmarshal(good, &doc, opts).unwrap_or_else(|e| panic!("{good:?}: {e}"));
    }
}

/// A directive's body is PXF, so the decoder holds it to what `parse` does.
#[test]
fn a_separator_in_a_directive_body_is_rejected_by_the_decoder() {
    let doc = message(&pool(FDS), "blocks.v1.Doc");
    let opts = UnmarshalOptions::default();
    assert!(unmarshal("@x { a = 1, b = 2 }\n", &doc, opts).is_err());
    unmarshal("@x { a = 1 b = 2 }\n", &doc, opts).unwrap();
}
