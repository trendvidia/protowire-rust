// SPDX-License-Identifier: MIT
// Copyright (c) 2026 TrendVidia, LLC.
//! Bool map-key spellings (draft -01 § Entries and Keys; protowire#284,
//! protowire-rust#31).
//!
//! The fixture corpus under `testdata/map-keys/` is vendored verbatim from
//! the spec repository (trendvidia/protowire `testdata/map-keys/`, commit
//! fb3bff7) and is shared by every port; keep the two in sync when the spec
//! repo adds fixtures. Its README states each document's verdict: the three
//! at the top MUST bind to the keys true and false, and every document
//! under `invalid/` MUST be rejected with an error naming the key.

use std::path::{Path, PathBuf};

use prost_reflect::{
    DescriptorPool, DynamicMessage, MapKey, MessageDescriptor, ReflectMessage, Value,
};
use protowire_pxf::{format, parse, unmarshal, Entry, UnmarshalOptions};

const FLAGS_FDS: &[u8] = include_bytes!("../testdata/map-keys/bool-keys.binpb");
const TEST_FDS: &[u8] = include_bytes!("../testdata/test.binpb");

fn flags() -> MessageDescriptor {
    DescriptorPool::decode(FLAGS_FDS)
        .expect("decode bool-keys.binpb")
        .get_message_by_name("mapkeys.v1.Flags")
        .expect("mapkeys.v1.Flags")
}

fn all_types() -> MessageDescriptor {
    DescriptorPool::decode(TEST_FDS)
        .expect("decode test.binpb")
        .get_message_by_name("test.v1.AllTypes")
        .expect("test.v1.AllTypes")
}

fn map_of(msg: &DynamicMessage, field: &str) -> std::collections::HashMap<MapKey, Value> {
    let fd = msg.descriptor().get_field_by_name(field).unwrap();
    match msg.get_field(&fd).into_owned() {
        Value::Map(m) => m,
        other => panic!("{field} is not a map: {other:?}"),
    }
}

fn fixture_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("testdata/map-keys")
}

fn pxf_files(dir: &Path) -> Vec<PathBuf> {
    let mut files: Vec<PathBuf> = std::fs::read_dir(dir)
        .unwrap_or_else(|e| panic!("read {}: {e}", dir.display()))
        .map(|e| e.unwrap().path())
        .filter(|p| p.extension().is_some_and(|x| x == "pxf"))
        .collect();
    files.sort();
    files
}

/// The key as the document spells it — the token before the ':' of a
/// `key: "value"` entry, quotes included when the spelling is quoted — so a
/// rejection can be checked to name it.
fn entry_key(doc: &str) -> String {
    let line = doc
        .lines()
        .find(|l| l.contains(": \""))
        .expect("a `key: \"value\"` entry");
    let inner = line.trim().trim_start_matches("by_flag = {").trim();
    inner.split(": \"").next().unwrap().trim().to_string()
}

// ---------------- The spec corpus ----------------

#[test]
fn fixtures_that_must_bind_do() {
    let files = pxf_files(&fixture_dir());
    assert_eq!(files.len(), 3, "the README lists three MUST-bind documents");
    for path in files {
        let doc = std::fs::read_to_string(&path).unwrap();
        // The AST parser admits every spelling too: fmt and validate go
        // through it before the decoder does.
        parse(&doc).unwrap_or_else(|e| panic!("{}: parse: {e}", path.display()));
        let msg = unmarshal(&doc, &flags(), UnmarshalOptions::default())
            .unwrap_or_else(|e| panic!("{}: {e}", path.display()));
        let m = map_of(&msg, "by_flag");
        assert_eq!(
            m.len(),
            2,
            "{}: want exactly true and false",
            path.display()
        );
        assert!(m.contains_key(&MapKey::Bool(true)), "{}", path.display());
        assert!(m.contains_key(&MapKey::Bool(false)), "{}", path.display());
    }
}

#[test]
fn fixtures_that_must_not_bind_are_rejected_naming_the_key() {
    let files = pxf_files(&fixture_dir().join("invalid"));
    assert_eq!(
        files.len(),
        13,
        "the README lists thirteen MUST-NOT-bind documents"
    );
    for path in files {
        let doc = std::fs::read_to_string(&path).unwrap();
        let key = entry_key(&doc);
        let err = unmarshal(&doc, &flags(), UnmarshalOptions::default())
            .expect_err(&format!("{} must not bind", path.display()));
        assert!(
            err.msg
                .contains(&format!("invalid bool map key {key} for field \"by_flag\"")),
            "{}: error must name the key {key}: {}",
            path.display(),
            err.msg
        );
    }
}

// ---------------- Every ParseBool spelling, bare and quoted ----------------

/// Pins the grammar's answer for each spelling `strconv.ParseBool` accepts,
/// plus the two words, tried bare and quoted: the keyword true / false
/// bare, bare 0 / 1 and quoted "true" / "false" bind; nothing else does.
/// The bare/quoted split is what makes this non-obvious.
#[test]
fn bool_map_key_spellings() {
    let desc = flags();
    let spellings = [
        "1", "t", "T", "TRUE", "true", "True", "0", "f", "F", "FALSE", "false", "False",
    ];
    let bare = |s: &str| match s {
        "1" | "true" => Some(true),
        "0" | "false" => Some(false),
        _ => None,
    };
    let quoted = |s: &str| match s {
        "true" => Some(true),
        "false" => Some(false),
        _ => None,
    };
    for sp in spellings {
        let doc = format!("by_flag = {{ {sp}: \"v\" }}");
        match (
            bare(sp),
            unmarshal(&doc, &desc, UnmarshalOptions::default()),
        ) {
            (Some(want), Ok(msg)) => {
                assert_eq!(
                    map_of(&msg, "by_flag").get(&MapKey::Bool(want)),
                    Some(&Value::String("v".into())),
                    "bare {sp}"
                );
            }
            (Some(_), Err(e)) => panic!("bare {sp} must bind: {e}"),
            (None, Ok(_)) => panic!("bare {sp} must not bind"),
            (None, Err(e)) => {
                assert!(
                    e.msg
                        .contains(&format!("invalid bool map key {sp} for field \"by_flag\"")),
                    "bare {sp}: {}",
                    e.msg
                );
                assert!(
                    e.msg
                        .contains("a bool key is true, false, 0, 1, \"true\" or \"false\""),
                    "bare {sp}: {}",
                    e.msg
                );
            }
        }
        let doc = format!("by_flag = {{ \"{sp}\": \"v\" }}");
        match (
            quoted(sp),
            unmarshal(&doc, &desc, UnmarshalOptions::default()),
        ) {
            (Some(want), Ok(msg)) => {
                assert_eq!(
                    map_of(&msg, "by_flag").get(&MapKey::Bool(want)),
                    Some(&Value::String("v".into())),
                    "quoted {sp:?}"
                );
            }
            (Some(_), Err(e)) => panic!("quoted {sp:?} must bind: {e}"),
            (None, Ok(_)) => panic!("quoted {sp:?} must not bind"),
            (None, Err(e)) => {
                // Includes "1" and "0": an integer literal inside a string
                // is not a bool literal.
                assert!(
                    e.msg.contains(&format!(
                        "invalid bool map key \"{sp}\" for field \"by_flag\""
                    )),
                    "quoted {sp:?}: {}",
                    e.msg
                );
            }
        }
    }
}

/// All three admitted spellings of each value land on the same key; the
/// later spelling wins, as any duplicate key does.
#[test]
fn admitted_spellings_share_one_key() {
    let msg = unmarshal(
        "by_flag = {\n  1: \"a\"\n  \"false\": \"b\"\n  true: \"c\"\n  false: \"d\"\n}",
        &flags(),
        UnmarshalOptions::default(),
    )
    .expect("binds");
    let m = map_of(&msg, "by_flag");
    assert_eq!(m.len(), 2);
    assert_eq!(m[&MapKey::Bool(true)], Value::String("c".into()));
    assert_eq!(m[&MapKey::Bool(false)], Value::String("d".into()));
}

// ---------------- The keyword on other key types ----------------

/// The keyword is a key on a map<bool,V> field and nothing else. On a
/// string K it is rejected naming the key — the string "true" is spelled
/// quoted — and on an integer K it fails the way an identifier does.
#[test]
fn bool_keyword_key_on_string_map_is_rejected() {
    let desc = all_types();
    for kw in ["true", "false"] {
        let err = unmarshal(
            &format!("string_map = {{ {kw}: \"v\" }}"),
            &desc,
            UnmarshalOptions::default(),
        )
        .expect_err("bare keyword on a string key must not bind");
        assert!(
            err.msg.contains(&format!(
                "invalid string map key {kw} for field \"string_map\""
            )),
            "{}",
            err.msg
        );
        assert!(
            err.msg.contains(&format!("write \"{kw}\" for the string")),
            "{}",
            err.msg
        );
    }
    // Quoted, it is the string.
    let msg = unmarshal(
        "string_map = { \"true\": \"v\" }",
        &desc,
        UnmarshalOptions::default(),
    )
    .expect("quoted binds as the string");
    assert_eq!(
        map_of(&msg, "string_map")[&MapKey::String("true".into())],
        Value::String("v".into())
    );
}

#[test]
fn bool_keyword_key_on_int_map_is_rejected() {
    let err = unmarshal(
        "int_map = { true: \"v\" }",
        &all_types(),
        UnmarshalOptions::default(),
    )
    .expect_err("must not bind");
    assert!(
        err.msg.contains("invalid int32 map key: true"),
        "{}",
        err.msg
    );
}

// ---------------- The AST side ----------------

/// fmt and validate parse before they decode, so the keyword must be a key
/// there too — and only with the ':' tail, exactly as an integer key is.
#[test]
fn bool_keyword_key_in_parser() {
    let src = "m = {\n  true: \"a\"\n  false: \"b\"\n}\n";
    let doc = parse(src).expect("parses");
    assert_eq!(doc.entries.len(), 1);
    let Entry::Assignment(asg) = &doc.entries[0] else {
        panic!("expected assignment, got {:?}", doc.entries[0]);
    };
    let protowire_pxf::Value::Block(blk) = &asg.value else {
        panic!("expected block, got {:?}", asg.value);
    };
    assert_eq!(blk.entries.len(), 2);
    for (entry, want) in blk.entries.iter().zip(["true", "false"]) {
        let Entry::MapEntry(me) = entry else {
            panic!("expected map entry, got {entry:?}");
        };
        assert_eq!(me.key, want);
    }
    // fmt reproduces the keyword bare.
    assert_eq!(format(&doc), src);

    for src in ["m = {\n  true = \"a\"\n}\n", "m = {\n  true { }\n}\n"] {
        let err = parse(src).expect_err("a bool key takes only the ':' tail");
        assert!(
            err.msg.contains("requires an identifier key, got bool"),
            "{src:?}: {}",
            err.msg
        );
    }
    // A bool value is still a value: the keyword at value position is not
    // mistaken for the next entry's key.
    let doc = parse("flag = true\nother = 1\n").expect("parses");
    assert_eq!(doc.entries.len(), 2);
}
