// SPDX-License-Identifier: MIT
// Copyright (c) 2026 TrendVidia, LLC.
//! Bind-time checks cover the import closure of the bound descriptor, not
//! the file declaring it (draft -01 §schema-constraints, "Scope of
//! Bind-Time Checks"; protowire-rust#25). The tests here cover the shapes
//! the closure adds — depth, diamonds, cross-file ordering — and the
//! per-descriptor memo. Mirrors protowire-go's `schema_closure_test.go`.

use prost::Message as _;
use prost_reflect::{DescriptorPool, DynamicMessage, MessageDescriptor, ReflectMessage, Value};
use protowire_pxf::{validate_descriptor, validate_file, ViolationKind};

fn msg(fds: &[u8], name: &str) -> MessageDescriptor {
    DescriptorPool::decode(fds)
        .expect("decode descriptor set")
        .get_message_by_name(name)
        .unwrap_or_else(|| panic!("missing {name}"))
}

const RESERVED: &[u8] = include_bytes!("../testdata/closure-reserved-root.binpb");
const DEPTH: &[u8] = include_bytes!("../testdata/closure-depth-a.binpb");
const DIAMOND: &[u8] = include_bytes!("../testdata/closure-diamond-top.binpb");
const SORT: &[u8] = include_bytes!("../testdata/closure-sort-zz_root.binpb");
const ACCEPTED: &[u8] = include_bytes!("../testdata/placement-accepted-test.binpb");
const DEFAULTS: &[u8] = include_bytes!("../testdata/default-placement-test.binpb");

/// The issue's runtime probe: a reserved name in an imported file was
/// findable from the imported descriptor but never reached from the one a
/// caller binds. The reserved-name check has been file-scoped since it
/// landed; the closure widens it too.
#[test]
fn reserved_name_in_imported_file_is_reported() {
    let vs = validate_descriptor(&msg(RESERVED, "closure_reserved_test.root.Root"));
    assert_eq!(vs.len(), 1, "{vs:?}");
    assert_eq!(vs[0].kind, ViolationKind::Field);
    assert_eq!(vs[0].file, "closure-reserved/imported.proto");
    assert_eq!(vs[0].element, "closure_reserved_test.imported.Inner.null");
}

/// Depth: the violation is three imports down, and no field of Root is
/// typed by the file that declares it.
#[test]
fn transitive_import_depth() {
    let vs = validate_descriptor(&msg(DEPTH, "closure_depth_test.a.Root"));
    assert_eq!(vs.len(), 1, "{vs:?}");
    assert_eq!(vs[0].kind, ViolationKind::DefaultOption);
    assert_eq!(vs[0].file, "closure-depth/c.proto");
    assert_eq!(vs[0].element, "closure_depth_test.c.Leaf.labels");
    assert!(
        vs[0].detail.contains("not valid on map fields"),
        "{}",
        vs[0].detail
    );
}

/// A diamond reaches the offending file twice. It is checked once.
#[test]
fn diamond_reports_once() {
    let vs = validate_descriptor(&msg(DIAMOND, "closure_diamond_test.top.Top"));
    assert_eq!(
        vs.len(),
        1,
        "shared.proto is reached through both arms and must be reported once: {vs:?}"
    );
    assert_eq!(vs[0].file, "closure-diamond/shared.proto");
    assert_eq!(vs[0].element, "closure_diamond_test.shared.Shared.tags");
}

/// Violations from several files sort by declaring file first, then by
/// element, so one file's violations stay together in the error text.
#[test]
fn sorted_by_file_then_element() {
    let vs = validate_descriptor(&msg(SORT, "closure_sort_test.root.Root"));
    let got: Vec<(&str, &str)> = vs
        .iter()
        .map(|v| (v.file.as_str(), v.element.as_str()))
        .collect();
    assert_eq!(
        got,
        vec![
            (
                "closure-sort/mm_mid.proto",
                "closure_sort_test.mid.Mid.null"
            ),
            (
                "closure-sort/mm_mid.proto",
                "closure_sort_test.mid.Mid.true"
            ),
            (
                "closure-sort/zz_root.proto",
                "closure_sort_test.root.Root.false"
            ),
        ]
    );
}

/// A conformant closure is still conformant: importing files with no
/// violations reports nothing, and in particular pxf/annotations.proto,
/// pxf/bignum.proto and google/protobuf/descriptor.proto — which every
/// annotated schema drags in — are clean.
#[test]
fn conformant_closure_is_clean() {
    let desc = msg(
        ACCEPTED,
        "placement_accepted_test.v1.EveryAcceptedPlacement",
    );
    let file = desc.parent_file();
    let deps: Vec<String> = file.dependencies().map(|d| d.name().to_string()).collect();
    assert!(
        deps.iter().any(|d| d == "pxf/annotations.proto"),
        "{deps:?}"
    );
    assert!(validate_file(&file).is_empty());
    let pool = desc.parent_pool();
    let descriptor_proto = pool
        .get_file_by_name("google/protobuf/descriptor.proto")
        .expect("descriptor.proto is in the closure");
    assert!(validate_file(&descriptor_proto).is_empty());
}

// ---------------- The memo ----------------

/// The memo is keyed by descriptor identity, not by path. Two
/// independently-built pools can hold different files at the same path,
/// and must not see each other's results.
///
/// Pool B is the violating descriptor set with its file renamed to the
/// clean file's path. The rename goes through a `DynamicMessage` of
/// `FileDescriptorSet`, not `prost_types`: prost drops unknown fields on
/// decode, which is where the `(pxf.*)` extension options live, and a
/// copy without them would validate clean for the wrong reason.
#[test]
fn memo_is_per_descriptor_not_per_path() {
    // Pool A: the clean file at its own path.
    let clean = msg(
        ACCEPTED,
        "placement_accepted_test.v1.EveryAcceptedPlacement",
    )
    .parent_file();
    assert!(validate_file(&clean).is_empty());

    // Pool B: the violating file renamed to the clean file's path,
    // extensions intact.
    let fds_desc = clean
        .parent_pool()
        .get_message_by_name("google.protobuf.FileDescriptorSet")
        .expect("descriptor.proto is in the closure");
    let mut set = DynamicMessage::decode(fds_desc, DEFAULTS).expect("decode set");
    let files_fd = set.descriptor().get_field_by_name("file").unwrap();
    let Value::List(mut files) = set.get_field(&files_fd).into_owned() else {
        panic!("file is a list")
    };
    let mut renamed = 0;
    for f in files.iter_mut() {
        let Value::Message(fdp) = f else {
            panic!("file entry")
        };
        let name_fd = fdp.descriptor().get_field_by_name("name").unwrap();
        if fdp.get_field(&name_fd).into_owned()
            == Value::String("default-placement-test.proto".into())
        {
            fdp.set_field(&name_fd, Value::String(clean.name().to_string()));
            renamed += 1;
        }
    }
    assert_eq!(renamed, 1);
    set.set_field(&files_fd, Value::List(files));
    let pool_b = DescriptorPool::decode(set.encode_to_vec().as_slice()).expect("pool B");
    let dirty = pool_b
        .get_file_by_name(clean.name())
        .expect("same path in pool B");
    assert_eq!(dirty.name(), clean.name());
    assert_eq!(
        validate_file(&dirty).len(),
        5,
        "pool B's file at the same path has its own result"
    );

    // Pool A is unaffected, in both directions.
    assert!(validate_file(&clean).is_empty());
    assert_eq!(validate_file(&dirty).len(), 5);
}

/// Repeated calls return equal results, and the caller may modify what
/// it gets back without corrupting the memo for the next caller.
#[test]
fn result_is_caller_owned() {
    let desc = msg(DEFAULTS, "default_placement_test.v1.Sibling");
    let mut first = validate_descriptor(&desc);
    assert_eq!(first.len(), 5);
    first.clear();
    first.push(validate_descriptor(&desc)[0].clone());
    let second = validate_descriptor(&desc);
    assert_eq!(second.len(), 5);
    let third = validate_descriptor(&desc);
    assert_eq!(second, third);
}

/// The memo is shared across threads; decoding concurrently is the normal
/// case for a server.
#[test]
fn concurrent_use() {
    let dirty = msg(DEFAULTS, "default_placement_test.v1.Sibling");
    let clean = msg(
        ACCEPTED,
        "placement_accepted_test.v1.EveryAcceptedPlacement",
    );
    let handles: Vec<_> = (0..8)
        .map(|i| {
            let dirty = dirty.clone();
            let clean = clean.clone();
            std::thread::spawn(move || {
                for _ in 0..200 {
                    if i % 2 == 0 {
                        assert_eq!(validate_descriptor(&dirty).len(), 5);
                    } else {
                        assert!(validate_descriptor(&clean).is_empty());
                    }
                }
            })
        })
        .collect();
    for h in handles {
        h.join().expect("thread");
    }
}
