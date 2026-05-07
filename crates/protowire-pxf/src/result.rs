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

use std::collections::HashSet;

#[derive(Debug, Default, Clone)]
pub struct Presence {
    present: HashSet<String>,
    nulls: HashSet<String>,
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
}
