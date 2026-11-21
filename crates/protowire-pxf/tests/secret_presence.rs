// SPDX-License-Identifier: MIT
// Copyright (c) 2026 TrendVidia, LLC.
//! A `pxf.Secret` literal in a list element or a map value records `value`
//! present, as its block form does there and as a field's literal does,
//! with or without the hook (protowire-go#299, #206). Before, whether
//! `backup_keys[1].value` was present depended on which spelling of one
//! value the document used. The documents and paths of
//! `secret_element_literal_marks_inner_value_present` are protowire-go's
//! `TestSecretElementLiteral_MarksInnerValuePresent`.
//!
//! Secret is the only such type (protowire-go#298): a Duration literal in a
//! list or map still records the element alone.

use prost_reflect::{DescriptorPool, MessageDescriptor};
use protowire_pxf::{unmarshal_full, SecretHook, UnmarshalOptions};

const SECRET_FDS: &[u8] = include_bytes!("../testdata/secret-hook-test.binpb");
const DURATION_FDS: &[u8] = include_bytes!("../testdata/duration-block-test.binpb");

fn message(fds: &[u8], name: &str) -> MessageDescriptor {
    DescriptorPool::decode(fds)
        .unwrap()
        .get_message_by_name(name)
        .unwrap()
}

/// The sorted present paths of `doc`, decoded with the hook when `hook`.
fn present(desc: &MessageDescriptor, doc: &str, hook: bool) -> Vec<String> {
    let ok = |_: &str, _: &str| Ok(());
    let ok: &SecretHook = &ok;
    let opts = UnmarshalOptions {
        on_secret_field: hook.then_some(ok),
        ..Default::default()
    };
    let (_, presence) = unmarshal_full(doc, desc, opts).unwrap_or_else(|e| panic!("{e}\n{doc}"));
    let mut got: Vec<String> = presence.present_fields().map(str::to_string).collect();
    got.sort_unstable();
    got
}

fn sorted(paths: &[&str]) -> Vec<String> {
    let mut v: Vec<String> = paths.iter().map(|p| p.to_string()).collect();
    v.sort_unstable();
    v
}

#[test]
fn secret_element_literal_marks_inner_value_present() {
    let desc = message(SECRET_FDS, "secret_hook_test.v1.SecretDemo");
    let doc = r#"
db_password = "x"
backup_keys = [{ value = "a"  hint = "h" }, "b"]
tenant_keys = { "acme": "k"  "z": { value = "v" } }
"#;
    let want = sorted(&[
        "backup_keys",
        "backup_keys[0].hint",
        "backup_keys[0].value",
        "backup_keys[1].value",
        "db_password",
        "db_password.value",
        "tenant_keys",
        r#"tenant_keys["acme"].value"#,
        r#"tenant_keys["z"].value"#,
    ]);
    for hook in [false, true] {
        assert_eq!(present(&desc, doc, hook), want, "hook set: {hook}");
    }
}

#[test]
fn secret_literal_under_every_key_type() {
    for (name, doc, want) in [
        (
            "SecretDemo",
            r#"backup_keys = ["a", "b", "c"]"#,
            &[
                "backup_keys",
                "backup_keys[0].value",
                "backup_keys[1].value",
                "backup_keys[2].value",
            ][..],
        ),
        (
            "SecretDemo",
            r#"tenant_keys = { "a\"b": "k" }"#,
            &["tenant_keys", r#"tenant_keys["a\"b"].value"#][..],
        ),
        (
            "Holder",
            r#"by_id = { 7: "s" -1: "t" }"#,
            &["by_id", "by_id[-1].value", "by_id[7].value"][..],
        ),
        (
            "Holder",
            r#"by_flag = { true: "s" }"#,
            &["by_flag", "by_flag[true].value"][..],
        ),
    ] {
        let desc = message(SECRET_FDS, &format!("secret_hook_test.v1.{name}"));
        for hook in [false, true] {
            assert_eq!(
                present(&desc, doc, hook),
                sorted(want),
                "{doc}, hook set: {hook}"
            );
        }
    }
}

#[test]
fn another_well_known_literal_records_the_element_alone() {
    let desc = message(DURATION_FDS, "durblock.Holder");
    for (doc, want) in [
        (
            "dur_field = 2s",
            &["dur_field", "dur_field.nanos", "dur_field.seconds"][..],
        ),
        ("list = [2s]", &["list"][..]),
        (r#"by_name = { "a": 2s }"#, &["by_name"][..]),
        ("list = [{ seconds = 2 }]", &["list", "list[0].seconds"][..]),
    ] {
        assert_eq!(present(&desc, doc, false), sorted(want), "{doc}");
    }
}
