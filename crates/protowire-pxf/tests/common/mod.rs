// SPDX-License-Identifier: MIT
// Copyright (c) 2026 TrendVidia, LLC.
//! Helpers for the spec corpora vendored under `testdata/<dir>/`.
//!
//! Each directory is copied verbatim from `testdata/<dir>/` in
//! trendvidia/protowire, whose README states every file's verdict. A file
//! named `err-*.pxf` must be rejected and any other `.pxf` accepted, so a
//! fixture added on resync joins the test with nothing to edit, and the
//! tests pin each directory's file count so one that goes missing shows.

#![allow(dead_code)]

use std::path::{Path, PathBuf};

use prost_reflect::{DescriptorPool, MessageDescriptor};

pub fn dir(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("testdata")
        .join(name)
}

pub fn pool(fds: &[u8]) -> DescriptorPool {
    DescriptorPool::decode(fds).expect("decode the fixture's FileDescriptorSet")
}

pub fn message(pool: &DescriptorPool, name: &str) -> MessageDescriptor {
    pool.get_message_by_name(name)
        .unwrap_or_else(|| panic!("no message {name}"))
}

/// The `.pxf` files directly in `testdata/<name>/`, sorted, each with its
/// stem and contents.
pub fn fixtures(name: &str) -> Vec<(String, String)> {
    let dir = dir(name);
    let mut paths: Vec<PathBuf> = std::fs::read_dir(&dir)
        .unwrap_or_else(|e| panic!("read {}: {e}", dir.display()))
        .map(|e| e.unwrap().path())
        .filter(|p| p.extension().is_some_and(|x| x == "pxf"))
        .collect();
    paths.sort();
    paths
        .into_iter()
        .map(|p| {
            let stem = p.file_stem().unwrap().to_string_lossy().into_owned();
            let doc = std::fs::read_to_string(&p).unwrap();
            (stem, doc)
        })
        .collect()
}

pub fn must_reject(stem: &str) -> bool {
    stem.starts_with("err-")
}

/// The type a fixture's `@type` line names, which is how a corpus with
/// several row types says which one a file binds to.
pub fn declared_type(doc: &str) -> Option<&str> {
    doc.lines()
        .find_map(|l| l.trim().strip_prefix("@type "))
        .map(str::trim)
}
