// SPDX-License-Identifier: MIT
// Copyright (c) 2026 TrendVidia, LLC.
//! Field-presence metadata returned by [`crate::unmarshal_full`].
//!
//! Mirrors `protowire/encoding/pxf/result.go` and the TS port's
//! `pxf/result.ts`. Distinguishes three states for any dotted field path:
//!
//! - **set**: explicitly assigned a non-null value
//! - **null**: present but explicitly assigned `null` (the "intentional null"
//!   channel that survives PXF-PB-PXF round-trips via `_null` FieldMask)
//! - **absent**: not mentioned in the input — eligible for `(pxf.default)`
//!   application and `(pxf.required)` validation
//!
//! Renamed from the upstream `Result` to avoid collision with `std::result::Result`.
//!
//! Also surfaces the document-root directives the decoder saw (PXF v0.72+):
//!   - [`Presence::directives`] — generic `@<name> *(prefix) [{ ... }]`
//!     blocks, in source order, excluding `@type` and `@dataset` (which
//!     have their own handling).
//!   - [`Presence::tables`] — `@dataset <type> ( cols ) row*` directives,
//!     in source order. A document with any `@dataset` has no body
//!     entries, so the rows are the document's payload — consumers walk
//!     `DatasetDirective.rows` and bind each row's cells to a fresh
//!     instance of `DatasetDirective.type` via their own schema.

use std::collections::HashSet;

use crate::ast::{DatasetDirective, Directive, ProtoDirective};

#[derive(Debug, Default, Clone)]
pub struct Presence {
    present: HashSet<String>,
    nulls: HashSet<String>,
    directives: Vec<Directive>,
    datasets: Vec<DatasetDirective>,
    protos: Vec<ProtoDirective>,
}

impl Presence {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn mark_present(&mut self, path: impl Into<String>) {
        self.present.insert(path.into());
    }

    pub fn mark_null(&mut self, path: impl Into<String>) {
        let p = path.into();
        self.present.insert(p.clone());
        self.nulls.insert(p);
    }

    /// True iff the field was assigned a non-null value.
    pub fn is_set(&self, path: &str) -> bool {
        self.present.contains(path) && !self.nulls.contains(path)
    }

    /// True iff the field was explicitly assigned `null`.
    pub fn is_null(&self, path: &str) -> bool {
        self.nulls.contains(path)
    }

    /// True iff the field was not mentioned at all.
    pub fn is_absent(&self, path: &str) -> bool {
        !self.present.contains(path)
    }

    /// Iterate over all paths explicitly set to `null`, in insertion order is
    /// not guaranteed (HashSet). Callers that need stable order should sort.
    pub fn null_paths(&self) -> impl Iterator<Item = &str> {
        self.nulls.iter().map(|s| s.as_str())
    }

    // Directive accessors (PXF v0.72+).

    pub fn directives(&self) -> &[Directive] {
        &self.directives
    }

    pub fn datasets(&self) -> &[DatasetDirective] {
        &self.datasets
    }

    pub fn protos(&self) -> &[ProtoDirective] {
        &self.protos
    }

    pub fn add_directive(&mut self, d: Directive) {
        self.directives.push(d);
    }

    pub fn add_dataset(&mut self, t: DatasetDirective) {
        self.datasets.push(t);
    }

    pub fn add_proto(&mut self, p: ProtoDirective) {
        self.protos.push(p);
    }
}
