// SPDX-License-Identifier: MIT
// Copyright (c) 2026 TrendVidia, LLC.
//! An entry's key, and the token after it, read as protowire-go reads them
//! (#123): the key set, integer and bool keys before `=` or `{` (refused
//! whatever `discard_unknown` says, since no field has such a name), a
//! precision suffix on a key, a `:` at the top level, and the wording of
//! each error, in message bodies, map bodies, keyed blocks and type
//! entries.
//!
//! Each row is what protowire-go v1.17.0 gave for the document: `Unmarshal`
//! and `UnmarshalFull` with `DiscardUnknown` off, then on (the two decoders
//! agreed on every row), then `Parse`. The two `$` rows name the illegal
//! token with the lexer's text (#149).

use prost_reflect::{DescriptorPool, MessageDescriptor};
use protowire_pxf::{parse, unmarshal, unmarshal_full, PoolResolver, UnmarshalOptions};

const ALL: (&[u8], &str) = (include_bytes!("../testdata/test.binpb"), "test.v1.AllTypes");
const KEYED: (&[u8], &str) = (
    include_bytes!("../testdata/keyed-entry-name-test.binpb"),
    "keyedname.v1.P",
);
const ANY: (&[u8], &str) = (
    include_bytes!("../testdata/any-test.binpb"),
    "any_test.v1.Container",
);

type Row = (
    (&'static [u8], &'static str),
    &'static str,
    Option<&'static str>,
    Option<&'static str>,
    Option<&'static str>,
);

#[rustfmt::skip]
const ROWS: &[Row] = &[
    (ALL, r#"1 = 2"#, Some(r#"1:1: field assignment with '=' requires an identifier or string key, got integer ("1"); use ':' for map entries"#), Some(r#"1:1: field assignment with '=' requires an identifier or string key, got integer ("1"); use ':' for map entries"#), Some(r#"1:1: field assignment with '=' requires an identifier or string key, got integer ("1"); use ':' for map entries"#)),
    (ALL, r#"1 { }"#, Some(r#"1:1: submessage block requires an identifier or string key, got integer ("1")"#), Some(r#"1:1: submessage block requires an identifier or string key, got integer ("1")"#), Some(r#"1:1: submessage block requires an identifier or string key, got integer ("1")"#)),
    (ALL, r#"true = 1"#, Some(r#"1:1: field assignment with '=' requires an identifier or string key, got bool ("true"); use ':' for map entries"#), Some(r#"1:1: field assignment with '=' requires an identifier or string key, got bool ("true"); use ':' for map entries"#), Some(r#"1:1: field assignment with '=' requires an identifier or string key, got bool ("true"); use ':' for map entries"#)),
    (ALL, r#"true { }"#, Some(r#"1:1: submessage block requires an identifier or string key, got bool ("true")"#), Some(r#"1:1: submessage block requires an identifier or string key, got bool ("true")"#), Some(r#"1:1: submessage block requires an identifier or string key, got bool ("true")"#)),
    (ALL, r#"false = 1"#, Some(r#"1:1: field assignment with '=' requires an identifier or string key, got bool ("false"); use ':' for map entries"#), Some(r#"1:1: field assignment with '=' requires an identifier or string key, got bool ("false"); use ':' for map entries"#), Some(r#"1:1: field assignment with '=' requires an identifier or string key, got bool ("false"); use ':' for map entries"#)),
    (ALL, r#"1p5 = 2"#, Some(r#"1:1: field assignment with '=' requires an identifier or string key, got integer ("1p5"); use ':' for map entries"#), Some(r#"1:1: field assignment with '=' requires an identifier or string key, got integer ("1p5"); use ':' for map entries"#), Some(r#"1:1: field assignment with '=' requires an identifier or string key, got integer ("1p5"); use ':' for map entries"#)),
    (ALL, r#"1p5 { }"#, Some(r#"1:1: submessage block requires an identifier or string key, got integer ("1p5")"#), Some(r#"1:1: submessage block requires an identifier or string key, got integer ("1p5")"#), Some(r#"1:1: submessage block requires an identifier or string key, got integer ("1p5")"#)),
    (ALL, r#"1.5 = 2"#, Some(r#"1:1: expected identifier, string, integer, or bool, got float ("1.5")"#), Some(r#"1:1: expected identifier, string, integer, or bool, got float ("1.5")"#), Some(r#"1:1: expected identifier, string, integer, or bool, got float ("1.5")"#)),
    (ALL, r#"[ ] = 1"#, Some(r#"1:1: expected identifier, string, integer, or bool, got [ ("[")"#), Some(r#"1:1: expected identifier, string, integer, or bool, got [ ("[")"#), Some(r#"1:1: expected identifier, string, integer, or bool, got [ ("[")"#)),
    (ALL, r#"= 1"#, Some(r#"1:1: expected identifier, string, integer, or bool, got = ("=")"#), Some(r#"1:1: expected identifier, string, integer, or bool, got = ("=")"#), Some(r#"1:1: expected identifier, string, integer, or bool, got = ("=")"#)),
    (ALL, r#"1 : 2"#, Some(r#"1:1: map entry (':' form) is only allowed inside a '{ … }' block; use '=' for top-level field assignments"#), Some(r#"1:1: map entry (':' form) is only allowed inside a '{ … }' block; use '=' for top-level field assignments"#), Some(r#"1:1: map entry (':' form) is only allowed inside a '{ … }' block; use '=' for top-level field assignments"#)),
    (ALL, r#"1"#, Some(r#"1:2: expected '=', ':', or '{' after "1", got EOF"#), Some(r#"1:2: expected '=', ':', or '{' after "1", got EOF"#), Some(r#"1:2: expected '=', ':', or '{' after "1", got EOF"#)),
    (ALL, r#"true"#, Some(r#"1:5: expected '=', ':', or '{' after "true", got EOF"#), Some(r#"1:5: expected '=', ':', or '{' after "true", got EOF"#), Some(r#"1:5: expected '=', ':', or '{' after "true", got EOF"#)),
    (ALL, r#"1 = 2 string_field = "x""#, Some(r#"1:1: field assignment with '=' requires an identifier or string key, got integer ("1"); use ':' for map entries"#), Some(r#"1:1: field assignment with '=' requires an identifier or string key, got integer ("1"); use ':' for map entries"#), Some(r#"1:1: field assignment with '=' requires an identifier or string key, got integer ("1"); use ':' for map entries"#)),
    (ALL, r#"string_field = "x" 1 { }"#, Some(r#"1:20: submessage block requires an identifier or string key, got integer ("1")"#), Some(r#"1:20: submessage block requires an identifier or string key, got integer ("1")"#), Some(r#"1:20: submessage block requires an identifier or string key, got integer ("1")"#)),
    (ALL, r#""x" = 1"#, Some(r#"1:1: quoted entry name "x" is only valid inside a keyed repeated field's block (draft -01 §3.13)"#), Some(r#"1:1: quoted entry name "x" is only valid inside a keyed repeated field's block (draft -01 §3.13)"#), None),
    (ALL, r#"x : 1"#, Some(r#"1:1: map entry (':' form) is only allowed inside a '{ … }' block; use '=' for top-level field assignments"#), Some(r#"1:1: map entry (':' form) is only allowed inside a '{ … }' block; use '=' for top-level field assignments"#), Some(r#"1:1: map entry (':' form) is only allowed inside a '{ … }' block; use '=' for top-level field assignments"#)),
    (ALL, r#"nested_field { 1 = 2 }"#, Some(r#"1:16: field assignment with '=' requires an identifier or string key, got integer ("1"); use ':' for map entries"#), Some(r#"1:16: field assignment with '=' requires an identifier or string key, got integer ("1"); use ':' for map entries"#), Some(r#"1:16: field assignment with '=' requires an identifier or string key, got integer ("1"); use ':' for map entries"#)),
    (ALL, r#"nested_field { true { } }"#, Some(r#"1:16: submessage block requires an identifier or string key, got bool ("true")"#), Some(r#"1:16: submessage block requires an identifier or string key, got bool ("true")"#), Some(r#"1:16: submessage block requires an identifier or string key, got bool ("true")"#)),
    (ALL, r#"bogus = 1 1 = 2"#, Some(r#"1:1: unknown field "bogus" in test.v1.AllTypes"#), Some(r#"1:11: field assignment with '=' requires an identifier or string key, got integer ("1"); use ':' for map entries"#), Some(r#"1:11: field assignment with '=' requires an identifier or string key, got integer ("1"); use ':' for map entries"#)),
    (ALL, r#"0x10 = 1"#, Some(r#"1:2: expected '=', ':', or '{' after "0", got identifier"#), Some(r#"1:2: expected '=', ':', or '{' after "0", got identifier"#), Some(r#"1:2: expected '=', ':', or '{' after "0", got identifier"#)),
    (ALL, r#"-1 = 2"#, Some(r#"1:1: field assignment with '=' requires an identifier or string key, got integer ("-1"); use ':' for map entries"#), Some(r#"1:1: field assignment with '=' requires an identifier or string key, got integer ("-1"); use ':' for map entries"#), Some(r#"1:1: field assignment with '=' requires an identifier or string key, got integer ("-1"); use ':' for map entries"#)),
    (ALL, r#"inf = 1"#, Some(r#"1:1: unknown field "inf" in test.v1.AllTypes"#), None, None),
    (ALL, r#"null = 1"#, Some(r#"1:1: expected identifier, string, integer, or bool, got null ("null")"#), Some(r#"1:1: expected identifier, string, integer, or bool, got null ("null")"#), Some(r#"1:1: expected identifier, string, integer, or bool, got null ("null")"#)),
    (ALL, r#"@x = 1"#, Some(r#"1:4: expected identifier, string, integer, or bool, got = ("=")"#), Some(r#"1:4: expected identifier, string, integer, or bool, got = ("=")"#), Some(r#"1:4: expected identifier, string, integer, or bool, got = ("=")"#)),
    (ALL, r#"1p5"#, Some(r#"1:4: expected '=', ':', or '{' after "1", got EOF"#), Some(r#"1:4: expected '=', ':', or '{' after "1", got EOF"#), Some(r#"1:4: expected '=', ':', or '{' after "1", got EOF"#)),
    (ALL, r#"bogus p5 = 1"#, Some(r#"1:7: expected '=', ':', or '{' after "bogus", got identifier"#), Some(r#"1:7: expected '=', ':', or '{' after "bogus", got identifier"#), Some(r#"1:7: expected '=', ':', or '{' after "bogus", got identifier"#)),
    (ALL, r#""abc"#, Some(r#"1:1: expected identifier, string, integer, or bool, got unterminated string"#), Some(r#"1:1: expected identifier, string, integer, or bool, got unterminated string"#), Some(r#"1:1: expected identifier, string, integer, or bool, got unterminated string"#)),
    (ALL, r#"int_map = { 1p5: "a" }"#, Some(r#"1:13: map field "int_map": a key cannot carry a precision suffix (p5)"#), Some(r#"1:13: map field "int_map": a key cannot carry a precision suffix (p5)"#), Some(r#"1:13: map key 1: a key cannot carry a precision suffix (p5)"#)),
    (ALL, r#"nested_field { x : 1 }"#, Some(r#"1:16: unexpected ':' in message context, use '=' for field assignments"#), Some(r#"1:16: unexpected ':' in message context, use '=' for field assignments"#), None),
    (ALL, r#"nested_field { 1p5 : 2 }"#, Some(r#"1:16: unexpected ':' in message context, use '=' for field assignments"#), Some(r#"1:16: unexpected ':' in message context, use '=' for field assignments"#), Some(r#"1:16: map key 1: a key cannot carry a precision suffix (p5)"#)),
    (ALL, r#"nested_field { 1p5 = 2 }"#, Some(r#"1:16: field assignment with '=' requires an identifier or string key, got integer ("1p5"); use ':' for map entries"#), Some(r#"1:16: field assignment with '=' requires an identifier or string key, got integer ("1p5"); use ':' for map entries"#), Some(r#"1:16: field assignment with '=' requires an identifier or string key, got integer ("1p5"); use ':' for map entries"#)),
    (ALL, r#"1p5 : 2"#, Some(r#"1:1: map entry (':' form) is only allowed inside a '{ … }' block; use '=' for top-level field assignments"#), Some(r#"1:1: map entry (':' form) is only allowed inside a '{ … }' block; use '=' for top-level field assignments"#), Some(r#"1:1: map entry (':' form) is only allowed inside a '{ … }' block; use '=' for top-level field assignments"#)),
    (ALL, r#"nested_field { true : 1 }"#, Some(r#"1:16: unexpected ':' in message context, use '=' for field assignments"#), Some(r#"1:16: unexpected ':' in message context, use '=' for field assignments"#), None),
    (ALL, r#"1p12345678901234567890123456789012345678901234567890 = 2"#, Some(r#"1:1: field assignment with '=' requires an identifier or string key, got integer ("1p123456789012345678901234…"); use ':' for map entries"#), Some(r#"1:1: field assignment with '=' requires an identifier or string key, got integer ("1p123456789012345678901234…"); use ':' for map entries"#), Some(r#"1:1: field assignment with '=' requires an identifier or string key, got integer ("1p123456789012345678901234…"); use ':' for map entries"#)),
    (ALL, r#"nested_field { 1p12345678901234567890123456789012345678901234567890 : 2 }"#, Some(r#"1:16: unexpected ':' in message context, use '=' for field assignments"#), Some(r#"1:16: unexpected ':' in message context, use '=' for field assignments"#), Some(r#"1:16: map key 1: a key cannot carry a precision suffix (p123456789012345678901234…)"#)),
    (ALL, r#"int_map = { 1p12345678901234567890123456789012345678901234567890: "a" }"#, Some(r#"1:13: map field "int_map": a key cannot carry a precision suffix (p123456789012345678901234…)"#), Some(r#"1:13: map field "int_map": a key cannot carry a precision suffix (p123456789012345678901234…)"#), Some(r#"1:13: map key 1: a key cannot carry a precision suffix (p123456789012345678901234…)"#)),
    (ALL, r#"int_map = { [ : "a" }"#, Some(r#"1:13: expected map key, got [ ("[")"#), Some(r#"1:13: expected map key, got [ ("[")"#), Some(r#"1:13: expected identifier, string, integer, or bool, got [ ("[")"#)),
    (ALL, r#"int_map = { 1.5: "a" }"#, Some(r#"1:13: expected map key, got float ("1.5")"#), Some(r#"1:13: expected map key, got float ("1.5")"#), Some(r#"1:13: expected identifier, string, integer, or bool, got float ("1.5")"#)),
    (ALL, r#"int_map = { "abc }"#, Some(r#"1:13: expected map key, got unterminated string"#), Some(r#"1:13: expected map key, got unterminated string"#), Some(r#"1:13: expected identifier, string, integer, or bool, got unterminated string"#)),
    (ALL, r#"int_map = { 1 = "a" }"#, Some(r#"1:15: unexpected '=' in map, use ':' for map entries"#), Some(r#"1:15: unexpected '=' in map, use ':' for map entries"#), Some(r#"1:13: field assignment with '=' requires an identifier or string key, got integer ("1"); use ':' for map entries"#)),
    (ALL, r#"int_map = { 1 "a" }"#, Some(r#"1:15: expected ':' after map key, got string"#), Some(r#"1:15: expected ':' after map key, got string"#), Some(r#"1:15: expected '=', ':', or '{' after "1", got string"#)),
    (ALL, r#"nested_map = { k { } }"#, Some(r#"1:18: expected ':' after map key, got {"#), Some(r#"1:18: expected ':' after map key, got {"#), None),
    (KEYED, r#"kd { 1 { } }"#, Some(r#"1:6: expected entry name (identifier or string) in keyed field "kd", got integer ("1")"#), Some(r#"1:6: expected entry name (identifier or string) in keyed field "kd", got integer ("1")"#), Some(r#"1:6: submessage block requires an identifier or string key, got integer ("1")"#)),
    (KEYED, r#"kd { [ ] }"#, Some(r#"1:6: expected entry name (identifier or string) in keyed field "kd", got [ ("[")"#), Some(r#"1:6: expected entry name (identifier or string) in keyed field "kd", got [ ("[")"#), Some(r#"1:6: expected identifier, string, integer, or bool, got [ ("[")"#)),
    (KEYED, r#"kd { "abc }"#, Some(r#"1:6: expected entry name (identifier or string) in keyed field "kd", got unterminated string"#), Some(r#"1:6: expected entry name (identifier or string) in keyed field "kd", got unterminated string"#), Some(r#"1:6: expected identifier, string, integer, or bool, got unterminated string"#)),
    (KEYED, r#"kd { a 1 }"#, Some(r#"1:8: expected '{' or '=' after entry name "a" in keyed field "kd", got integer"#), Some(r#"1:8: expected '{' or '=' after entry name "a" in keyed field "kd", got integer"#), Some(r#"1:8: expected '=', ':', or '{' after "a", got integer"#)),
    (KEYED, r#"kd { a : 1 }"#, Some(r#"1:8: expected '{' or '=' after entry name "a" in keyed field "kd", got :"#), Some(r#"1:8: expected '{' or '=' after entry name "a" in keyed field "kd", got :"#), None),
    (KEYED, r#"kd { a [ }"#, Some(r#"1:8: expected '{' or '=' after entry name "a" in keyed field "kd", got ["#), Some(r#"1:8: expected '{' or '=' after entry name "a" in keyed field "kd", got ["#), Some(r#"1:8: expected '=', ':', or '{' after "a", got ["#)),
    (KEYED, r#"kd { true { } }"#, Some(r#"1:6: expected entry name (identifier or string) in keyed field "kd", got bool ("true")"#), Some(r#"1:6: expected entry name (identifier or string) in keyed field "kd", got bool ("true")"#), Some(r#"1:6: submessage block requires an identifier or string key, got bool ("true")"#)),
    (KEYED, r#"kd { 1.5 = { } }"#, Some(r#"1:6: expected entry name (identifier or string) in keyed field "kd", got float ("1.5")"#), Some(r#"1:6: expected entry name (identifier or string) in keyed field "kd", got float ("1.5")"#), Some(r#"1:6: expected identifier, string, integer, or bool, got float ("1.5")"#)),
    (ANY, r#"payload { @type 5 }"#, Some(r#"1:17: expected '=' after @type, got integer ("5")"#), Some(r#"1:17: expected '=' after @type, got integer ("5")"#), Some(r#"1:17: expected '=' after @type, got integer ("5")"#)),
    (ANY, r#"payload { @type [ }"#, Some(r#"1:17: expected '=' after @type, got [ ("[")"#), Some(r#"1:17: expected '=' after @type, got [ ("[")"#), Some(r#"1:17: expected '=' after @type, got [ ("[")"#)),
    (ANY, r#"payload { @type = 5 }"#, Some(r#"1:19: expected a string type URL after @type =, got integer ("5")"#), Some(r#"1:19: expected a string type URL after @type =, got integer ("5")"#), Some(r#"1:19: expected a string type URL after @type =, got integer ("5")"#)),
    (ANY, r#"payload { @type = [ }"#, Some(r#"1:19: expected a string type URL after @type =, got [ ("[")"#), Some(r#"1:19: expected a string type URL after @type =, got [ ("[")"#), Some(r#"1:19: expected a string type URL after @type =, got [ ("[")"#)),
    (ANY, r#"payload { @type = "abc }"#, Some(r#"1:19: expected a string type URL after @type =, got unterminated string"#), Some(r#"1:19: expected a string type URL after @type =, got unterminated string"#), Some(r#"1:19: expected a string type URL after @type =, got unterminated string"#)),
    (ANY, r#"payload { @type }"#, Some(r#"1:17: expected '=' after @type, got } ("}")"#), Some(r#"1:17: expected '=' after @type, got } ("}")"#), Some(r#"1:17: expected '=' after @type, got } ("}")"#)),
    (ALL, r#"@type 5"#, Some(r#"1:7: expected type name after @type, got integer ("5")"#), Some(r#"1:7: expected type name after @type, got integer ("5")"#), Some(r#"1:7: expected type name after @type, got integer"#)),
    (ALL, r#"@type ["#, Some(r#"1:7: expected type name after @type, got [ ("[")"#), Some(r#"1:7: expected type name after @type, got [ ("[")"#), Some(r#"1:7: expected type name after @type, got ["#)),
    (ALL, r#"@type "abc"#, Some(r#"1:7: expected type name after @type, got unterminated string"#), Some(r#"1:7: expected type name after @type, got unterminated string"#), Some(r#"1:7: expected type name after @type, got ILLEGAL"#)),
    (ALL, r#"$ = 1"#, Some(r#"1:1: expected identifier, string, integer, or bool, got unexpected character '$'"#), Some(r#"1:1: expected identifier, string, integer, or bool, got unexpected character '$'"#), Some(r#"1:1: expected identifier, string, integer, or bool, got unexpected character '$'"#)),
    (ALL, r#"nested_field { $ = 1 }"#, Some(r#"1:16: expected identifier, string, integer, or bool, got unexpected character '$'"#), Some(r#"1:16: expected identifier, string, integer, or bool, got unexpected character '$'"#), Some(r#"1:16: expected identifier, string, integer, or bool, got unexpected character '$'"#)),
];

fn verdict<T>(r: Result<T, protowire_pxf::PxfError>) -> Option<String> {
    r.err().map(|e| e.to_string())
}

#[test]
fn an_entry_key_is_read_as_protowire_go_reads_it() {
    let mut checked = 0;
    for &((fds, name), doc, off, on, parsed) in ROWS {
        let pool = DescriptorPool::decode(fds).expect("descriptor set");
        let desc: MessageDescriptor = pool.get_message_by_name(name).expect("message");
        let res = PoolResolver(&pool);
        for (discard, want) in [(false, off), (true, on)] {
            let opts = UnmarshalOptions {
                discard_unknown: discard,
                type_resolver: Some(&res),
                ..Default::default()
            };
            let want = want.map(str::to_string);
            assert_eq!(
                verdict(unmarshal(doc, &desc, opts)),
                want,
                "unmarshal {doc:?}, discard_unknown {discard}"
            );
            assert_eq!(
                verdict(unmarshal_full(doc, &desc, opts)),
                want,
                "unmarshal_full {doc:?}, discard_unknown {discard}"
            );
        }
        assert_eq!(
            verdict(parse(doc)),
            parsed.map(str::to_string),
            "parse {doc:?}"
        );
        checked += 1;
    }
    assert_eq!(checked, 62);
}
