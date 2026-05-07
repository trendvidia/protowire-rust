// SPDX-License-Identifier: MIT
// Copyright (c) 2026 TrendVidia, LLC.
//! HARDENING.md §Recursion conformance for the PXF decoder.
//!
//! Drives the decoder's MaxNestingDepth = 100 cap end-to-end against a
//! self-recursive `Tree` schema (mirrors `adversarial.v1.Tree` in the
//! cross-port corpus). The 100k-deep input here is the SIGABRT case from
//! the M8 baseline — the decoder must convert it into a clean
//! `PxfError`, not crash the process.

use prost_reflect::{DescriptorPool, MessageDescriptor};
use protowire_pxf::{unmarshal, PxfError, UnmarshalOptions};

const TEST_FDS: &[u8] = include_bytes!("../testdata/hardening-test.binpb");

fn tree() -> MessageDescriptor {
    DescriptorPool::decode(TEST_FDS)
        .expect("decode hardening-test.binpb")
        .get_message_by_name("hardening_test.v1.Tree")
        .expect("missing hardening_test.v1.Tree")
}

fn build_nested_pxf(depth: usize) -> String {
    let mut s = String::with_capacity(depth * 8 + 16);
    for _ in 0..depth {
        s.push_str("child {");
    }
    s.push_str(" label = \"leaf\" ");
    for _ in 0..depth {
        s.push('}');
    }
    s
}

#[test]
fn deep_nesting_at_limit_decodes() {
    // Top-level Tree has depth 0; each `child {` opens a new `{` block.
    // 99 child levels reaches depth 99 in decode_fields and stays under
    // the cap.
    let src = build_nested_pxf(99);
    unmarshal(&src, &tree(), UnmarshalOptions::default()).expect("decode ok");
}

#[test]
fn deep_nesting_past_limit_is_rejected() {
    let src = build_nested_pxf(200);
    let err: PxfError =
        unmarshal(&src, &tree(), UnmarshalOptions::default()).expect_err("must reject");
    assert!(
        err.msg.contains("MaxNestingDepth"),
        "expected depth error, got: {}",
        err.msg
    );
}

#[test]
fn deep_nesting_extreme_does_not_overflow_stack() {
    // 100k-deep is the SIGABRT case from M8 issue #1. The cap must trip
    // before native recursion exhausts the thread stack.
    let src = build_nested_pxf(100_000);
    let err: PxfError =
        unmarshal(&src, &tree(), UnmarshalOptions::default()).expect_err("must reject");
    assert!(
        err.msg.contains("MaxNestingDepth"),
        "expected depth error, got: {}",
        err.msg
    );
}
