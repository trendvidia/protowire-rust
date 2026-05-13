// SPDX-License-Identifier: MIT
// Copyright (c) 2026 TrendVidia, LLC.
//! Tests for the @proto directive (draft §3.4.5).
//!
//! Four body shapes lexically distinguished: anonymous, named,
//! source, descriptor. Plus reserved-directive-name rejection
//! (draft §3.4.6).

use protowire_pxf::ast::ProtoShape;
use protowire_pxf::parse;

fn body_str(p: &protowire_pxf::ast::ProtoDirective) -> String {
    String::from_utf8(p.body.clone()).unwrap()
}

#[test]
fn anonymous_body() {
    let doc = parse(
        "@proto {\n  string symbol = 1;\n  double price = 2;\n}\n",
    )
    .unwrap();
    assert_eq!(doc.protos.len(), 1);
    let p = &doc.protos[0];
    assert_eq!(p.shape, ProtoShape::Anonymous);
    assert_eq!(p.type_name, "");
    let body = body_str(p);
    assert!(body.contains("string symbol = 1;"));
    assert!(body.contains("double price = 2;"));
}

#[test]
fn named_body() {
    let doc = parse("@proto trades.v1.Trade {\n  string symbol = 1;\n}\n").unwrap();
    assert_eq!(doc.protos.len(), 1);
    assert_eq!(doc.protos[0].shape, ProtoShape::Named);
    assert_eq!(doc.protos[0].type_name, "trades.v1.Trade");
    assert!(body_str(&doc.protos[0]).contains("string symbol = 1;"));
}

#[test]
fn source_body() {
    let doc = parse(
        "@proto \"\"\"\nsyntax = \"proto3\";\nmessage Trade { string symbol = 1; }\n\"\"\"",
    )
    .unwrap();
    assert_eq!(doc.protos.len(), 1);
    assert_eq!(doc.protos[0].shape, ProtoShape::Source);
    assert!(body_str(&doc.protos[0]).contains("message Trade"));
}

#[test]
fn descriptor_body() {
    // "hello" → "aGVsbG8="
    let doc = parse("@proto b\"aGVsbG8=\"").unwrap();
    assert_eq!(doc.protos.len(), 1);
    assert_eq!(doc.protos[0].shape, ProtoShape::Descriptor);
    assert_eq!(doc.protos[0].body, b"hello");
}

#[test]
fn multiple_protos() {
    let doc = parse(
        "@proto trades.v1.Trade { string symbol = 1; }\n\
         @proto orders.v1.Order { string id = 1; }\n",
    )
    .unwrap();
    assert_eq!(doc.protos.len(), 2);
    assert_eq!(doc.protos[0].type_name, "trades.v1.Trade");
    assert_eq!(doc.protos[1].type_name, "orders.v1.Order");
}

#[test]
fn anonymous_followed_by_untyped_dataset() {
    // One-shot binding: anonymous @proto types the next untyped
    // @dataset (draft §3.4.4 Anonymous binding).
    let doc = parse(
        "@proto {\n  string symbol = 1;\n  double price = 2;\n}\n\
         @dataset (symbol, price)\n(\"AAPL\", 192.34)\n(\"MSFT\", 410.10)\n",
    )
    .unwrap();
    assert_eq!(doc.protos.len(), 1);
    assert_eq!(doc.protos[0].shape, ProtoShape::Anonymous);
    assert_eq!(doc.datasets.len(), 1);
    assert!(doc.datasets[0].r#type.is_empty());
    assert_eq!(doc.datasets[0].rows.len(), 2);
}

#[test]
fn nested_braces_in_body() {
    let doc = parse(
        "@proto {\n  message Side {\n    string label = 1;\n  }\n  Side side = 1;\n}\n",
    )
    .unwrap();
    let body = body_str(&doc.protos[0]);
    assert!(body.contains("message Side"));
    assert!(body.contains("Side side = 1;"));
}

#[test]
fn rejects_bad_shape() {
    let err = parse("@proto 42").unwrap_err();
    assert!(err.to_string().contains("after @proto"));
}

#[test]
fn rejects_named_missing_brace() {
    let err = parse("@proto trades.v1.Trade 42").unwrap_err();
    assert!(err.to_string().contains("'{'"));
}

#[test]
fn rejects_anonymous_unmatched_brace() {
    let err = parse("@proto { string symbol = 1;").unwrap_err();
    assert!(err.to_string().contains("unmatched"));
}

#[test]
fn coexists_with_type() {
    let doc = parse(
        "@type some.pkg.Foo\n@proto some.pkg.Foo {\n  string name = 1;\n}\n",
    )
    .unwrap();
    assert_eq!(doc.type_url, "some.pkg.Foo");
    assert_eq!(doc.protos.len(), 1);
    assert_eq!(doc.protos[0].shape, ProtoShape::Named);
}

#[test]
fn proto_shape_name_lookup() {
    assert_eq!(ProtoShape::Anonymous.name(), "anonymous");
    assert_eq!(ProtoShape::Named.name(), "named");
    assert_eq!(ProtoShape::Source.name(), "source");
    assert_eq!(ProtoShape::Descriptor.name(), "descriptor");
}

// ---- Reserved directive names (draft §3.4.6) ---------------------

#[test]
fn rejects_reserved_directive_names() {
    for name in &[
        "table",
        "datasource",
        "view",
        "procedure",
        "function",
        "permissions",
    ] {
        let input = format!("@{} {{ x = 1 }}", name);
        let err = parse(&input).unwrap_err();
        assert!(
            err.to_string().contains("spec-reserved"),
            "@{} should be rejected as spec-reserved: {}",
            name,
            err
        );
    }
}
