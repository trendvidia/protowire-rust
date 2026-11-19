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
    let doc = parse("@proto {\n  string symbol = 1;\n  double price = 2;\n}\n").unwrap();
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
    let doc =
        parse("@proto \"\"\"\nsyntax = \"proto3\";\nmessage Trade { string symbol = 1; }\n\"\"\"")
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
    let doc =
        parse("@proto {\n  message Side {\n    string label = 1;\n  }\n  Side side = 1;\n}\n")
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
    let doc = parse("@type some.pkg.Foo\n@proto some.pkg.Foo {\n  string name = 1;\n}\n").unwrap();
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

// ---- A source body is triple-quoted only (#205) ------------------

// The grammar admits only `proto_source_body = triple_string`. A plain
// quoted body's escapes could put `"""` in it, which no formatter can write
// back. protowire-go refuses it as of protowire-go#311, with this text.
const NOT_TRIPLE: &str = r#"1:8: @proto source body must be a triple-quoted string ("""...""")"#;

const TEST_FDS: &[u8] = include_bytes!("../testdata/test.binpb");

fn all_types() -> prost_reflect::MessageDescriptor {
    prost_reflect::DescriptorPool::decode(TEST_FDS)
        .expect("decode test.binpb")
        .get_message_by_name("test.v1.AllTypes")
        .expect("missing test.v1.AllTypes")
}

fn dataset_reader(
    src: &str,
) -> Result<protowire_pxf::DatasetReader<std::io::Cursor<Vec<u8>>>, protowire_pxf::PxfError> {
    protowire_pxf::DatasetReader::new(std::io::Cursor::new(src.as_bytes().to_vec()))
}

#[test]
fn source_body_must_be_triple_quoted() {
    let desc = all_types();
    for body in [
        r#""syntax = \"proto3\";""#,
        r#""syntax = \"proto3\"; // a \"\"\" b""#,
        r#""""#,
        // Not UTF-8: still a plain string, whose bytes `parse` cannot hold
        // (#191) and the decoder keeps (#170). Both refuse its form first.
        r#""\xff""#,
    ] {
        let doc = format!("@proto {body}\nstring_field = \"x\"\n");
        let at = |what: &str, got: Result<(), protowire_pxf::PxfError>| {
            let err = got.expect_err(&format!("{what} accepted {doc:?}"));
            assert_eq!(err.to_string(), NOT_TRIPLE, "{what} on {doc:?}");
        };
        at("parse", parse(&doc).map(|_| ()));
        let opts = protowire_pxf::UnmarshalOptions::default();
        at(
            "unmarshal",
            protowire_pxf::unmarshal(&doc, &desc, opts).map(|_| ()),
        );
        at(
            "unmarshal_full",
            protowire_pxf::unmarshal_full(&doc, &desc, opts).map(|_| ()),
        );
        // A dataset stream reads its header's directives on its own path.
        let stream = format!("@proto {body}\n@dataset test.v1.AllTypes (string_field)\n(\"x\")\n");
        let err = dataset_reader(&stream)
            .map(|_| ())
            .expect_err("DatasetReader accepted");
        assert_eq!(err.to_string(), NOT_TRIPLE, "DatasetReader on {stream:?}");
    }
}

#[test]
fn source_body_after_type_must_be_triple_quoted() {
    let doc = "@type test.v1.AllTypes\n@proto \"syntax = \\\"proto3\\\";\"\nstring_field = \"x\"\n";
    let err = parse(doc).unwrap_err();
    assert_eq!(err.to_string(), NOT_TRIPLE.replacen("1:8", "2:8", 1));
    let err = protowire_pxf::unmarshal(doc, &all_types(), Default::default()).unwrap_err();
    assert_eq!(err.to_string(), NOT_TRIPLE.replacen("1:8", "2:8", 1));
}

#[test]
fn triple_quoted_source_body_still_reads() {
    let desc = all_types();
    for doc in [
        "@proto \"\"\"syntax = \"proto3\";\"\"\"\nstring_field = \"x\"\n",
        "@proto \"\"\"\nsyntax = \"proto3\";\n\"\"\"\nstring_field = \"x\"\n",
        "@proto\n\"\"\"syntax = \"proto3\";\"\"\"\nstring_field = \"x\"\n",
    ] {
        let parsed = parse(doc).unwrap_or_else(|e| panic!("parse {doc:?}: {e}"));
        assert_eq!(parsed.protos.len(), 1);
        assert_eq!(parsed.protos[0].shape, ProtoShape::Source);
        assert_eq!(body_str(&parsed.protos[0]), "syntax = \"proto3\";");
        assert_eq!(parsed.entries.len(), 1);
        let msg = protowire_pxf::unmarshal(doc, &desc, Default::default())
            .unwrap_or_else(|e| panic!("unmarshal {doc:?}: {e}"));
        let field = desc.get_field_by_name("string_field").unwrap();
        assert_eq!(msg.get_field(&field).as_str(), Some("x"));
        let stream = doc.replace(
            "string_field = \"x\"\n",
            "@dataset test.v1.AllTypes (string_field)\n(\"x\")\n",
        );
        let r = dataset_reader(&stream).unwrap_or_else(|e| panic!("DatasetReader {stream:?}: {e}"));
        assert_eq!(r.type_name(), "test.v1.AllTypes");
    }
}

// The descriptor form is a bytes literal, `proto_descr_body = bytes`, and is
// not a plain quoted string: it stays accepted, empty or not.
#[test]
fn descriptor_body_is_not_refused() {
    let desc = all_types();
    for (doc, body) in [
        ("@proto b\"\"\nstring_field = \"x\"\n", &b""[..]),
        (
            "@proto b\"aGVsbG8=\"\nstring_field = \"x\"\n",
            &b"hello"[..],
        ),
    ] {
        let parsed = parse(doc).unwrap();
        assert_eq!(parsed.protos[0].shape, ProtoShape::Descriptor);
        assert_eq!(parsed.protos[0].body, body);
        protowire_pxf::unmarshal(doc, &desc, Default::default()).unwrap();
        let stream = doc.replace(
            "string_field = \"x\"\n",
            "@dataset test.v1.AllTypes (string_field)\n(\"x\")\n",
        );
        dataset_reader(&stream).unwrap();
    }
}

// `parse_tolerant` reports a plain body once, consumes it, and reads on: the
// string is not taken for an entry's key.
#[test]
fn tolerant_reports_a_plain_source_body_and_parses_on() {
    let (doc, errs) = protowire_pxf::parse_tolerant("@proto \"syntax\"\n@type a.v1.M\n\nx = 1\n");
    assert_eq!(
        errs.iter().map(|e| e.to_string()).collect::<Vec<_>>(),
        [NOT_TRIPLE]
    );
    assert!(doc.protos.is_empty());
    assert_eq!(doc.type_url, "a.v1.M");
    assert_eq!(doc.entries.len(), 1);
}
