// SPDX-License-Identifier: MIT
// Copyright (c) 2026 TrendVidia, LLC.
//! SBE saxlite tests. Mirrors `src/sbe/saxlite.test.ts` in the TS port.

use std::collections::HashMap;

use protowire_sbe::saxlite::{parse_xml, SaxError, SaxHandler};

#[derive(Debug, Clone, PartialEq, Eq)]
enum Event {
    Open(String, Vec<(String, String)>),
    Close(String),
    Text(String),
}

#[derive(Default)]
struct Collector {
    events: Vec<Event>,
}

impl SaxHandler for Collector {
    fn open(&mut self, name: &str, attrs: &HashMap<String, String>) -> Result<(), SaxError> {
        let mut pairs: Vec<(String, String)> =
            attrs.iter().map(|(k, v)| (k.clone(), v.clone())).collect();
        pairs.sort();
        self.events.push(Event::Open(name.to_string(), pairs));
        Ok(())
    }
    fn close(&mut self, name: &str) -> Result<(), SaxError> {
        self.events.push(Event::Close(name.to_string()));
        Ok(())
    }
    fn text(&mut self, value: &str) -> Result<(), SaxError> {
        self.events.push(Event::Text(value.to_string()));
        Ok(())
    }
}

fn collect(xml: &str) -> Vec<Event> {
    let mut c = Collector::default();
    parse_xml(xml, &mut c).expect("parse");
    c.events
}

fn try_collect(xml: &str) -> Result<Vec<Event>, SaxError> {
    let mut c = Collector::default();
    parse_xml(xml, &mut c)?;
    Ok(c.events)
}

fn open(name: &str, pairs: &[(&str, &str)]) -> Event {
    let mut v: Vec<(String, String)> = pairs
        .iter()
        .map(|(k, val)| (k.to_string(), val.to_string()))
        .collect();
    v.sort();
    Event::Open(name.to_string(), v)
}

#[test]
fn parses_open_and_close_tags_with_attributes() {
    assert_eq!(
        collect(r#"<root a="1" b='two'></root>"#),
        vec![open("root", &[("a", "1"), ("b", "two")]), Event::Close("root".into())]
    );
}

#[test]
fn self_closing_tags_emit_open_then_close() {
    assert_eq!(
        collect("<x/>"),
        vec![open("x", &[]), Event::Close("x".into())]
    );
}

#[test]
fn self_closing_with_attributes_and_slash_boundary() {
    assert_eq!(
        collect(r#"<type name="str8" length="8"/>"#),
        vec![
            open("type", &[("name", "str8"), ("length", "8")]),
            Event::Close("type".into()),
        ]
    );
}

#[test]
fn emits_char_data_between_tags_and_decodes_entities() {
    assert_eq!(
        collect(r#"<v>1 &amp; 2 &lt;3&gt; &quot;x&quot; &apos;y&apos;</v>"#),
        vec![
            open("v", &[]),
            Event::Text(r#"1 & 2 <3> "x" 'y'"#.into()),
            Event::Close("v".into()),
        ]
    );
}

#[test]
fn skips_xml_prolog_and_comments() {
    assert_eq!(
        collect(r#"<?xml version="1.0"?><!-- a comment --><root/>"#),
        vec![open("root", &[]), Event::Close("root".into())]
    );
}

#[test]
fn strips_namespace_prefixes_on_element_and_attribute_names() {
    assert_eq!(
        collect(r#"<sbe:root xmlns:sbe="http://x" sbe:tag="t" plain="p"></sbe:root>"#),
        vec![
            open("root", &[("tag", "t"), ("plain", "p")]),
            Event::Close("root".into()),
        ]
    );
}

#[test]
fn preserves_whitespace_text_events_between_tags() {
    let ev = collect("<a>\n  <b/>\n</a>");
    assert_eq!(ev[0], open("a", &[]));
    assert_eq!(ev[1], Event::Text("\n  ".into()));
    assert_eq!(ev[2], open("b", &[]));
}

#[test]
fn rejects_unterminated_comment() {
    let err = try_collect("<!-- oops").expect_err("unterminated comment");
    assert!(err.message.contains("unterminated comment"), "{}", err.message);
}

#[test]
fn rejects_attribute_without_equals_sign() {
    let err = try_collect(r#"<x foo "1"/>"#).expect_err("expected '='");
    assert!(err.message.contains("expected '='"), "{}", err.message);
}

#[test]
fn rejects_attribute_without_quoted_value() {
    let err = try_collect("<x foo=1/>").expect_err("quoted value");
    assert!(err.message.contains("expected quoted value"), "{}", err.message);
}
