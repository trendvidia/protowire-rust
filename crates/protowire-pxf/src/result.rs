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
//! A path through a list element or a map value carries its index or key:
//! `items[0].name`, `by_id["a"].label`, `by_num[7].label`. An entry name of
//! a keyed repeated field counts as setting the key field: `kd { alpha {} }`
//! reports `kd[0].name` as set.
//!
//! Renamed from the upstream `Result` to avoid collision with `std::result::Result`.
//!
//! Also surfaces the document-root directives the decoder saw (PXF v0.72+):
//!   - [`Presence::directives`] — generic `@<name> *(prefix) [{ ... }]`
//!     blocks, in source order, excluding `@type` and `@dataset` (which
//!     have their own handling).
//!   - [`Presence::datasets`] — `@dataset <type> ( cols ) row*` directives,
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
    /// The paths of `nulls`, in the order they were marked: document order,
    /// which the root's `_null` mask keeps, as protowire-go's does (#174).
    null_order: Vec<String>,
    directives: Vec<Directive>,
    datasets: Vec<DatasetDirective>,
    protos: Vec<ProtoDirective>,
    report: Option<protowire_check::Report>,
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
        if self.nulls.insert(p.clone()) {
            self.null_order.push(p);
        }
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

    /// Iterate over all paths explicitly set to `null`, in the order they
    /// were marked. The decoder marks each as it binds it, so that is the
    /// document's order, the order of the root's `_null` mask.
    pub fn null_paths(&self) -> impl Iterator<Item = &str> {
        self.null_order.iter().map(|s| s.as_str())
    }

    /// Iterate over every path the document mentioned: set to a value or to
    /// `null`, the union of [`Presence::is_set`] and [`Presence::is_null`].
    /// The order is not guaranteed; callers that need one should sort.
    /// protowire-go's `Result.PresentFields`. A layered-config caller unions
    /// it across layers, then runs [`crate::apply_default`] and its required
    /// check over the merged message (see
    /// [`crate::UnmarshalOptions::skip_post_decode`]).
    pub fn present_fields(&self) -> impl Iterator<Item = &str> {
        self.present.iter().map(|s| s.as_str())
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

    /// The report of [`crate::UnmarshalOptions::validator`], or `None` when
    /// no validator ran. A decode returns presence only when the report is
    /// clean; the report of one with violations is on the error, as
    /// [`crate::PxfError::check_error`]. protowire-go's `Result.Report`.
    pub fn report(&self) -> Option<&protowire_check::Report> {
        self.report.as_ref()
    }

    pub(crate) fn set_report(&mut self, report: Option<protowire_check::Report>) {
        self.report = report;
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
