// SPDX-License-Identifier: MIT
// Copyright (c) 2026 TrendVidia, LLC.
//! Dotted `@dataset` column paths (draft -01 §3.4.4; protowire#342). A
//! column may be a path such as `desc.energy`: a non-empty cell in it stands
//! for `energy = <value>` inside the block `desc { ... }`, and every cell of
//! a row sharing a path prefix lands in that one block.

use std::collections::HashMap;

use prost_reflect::{Kind, MessageDescriptor};

/// A path's prefix, in a trie of the columns seen so far.
#[derive(Default)]
struct PathNode<'a> {
    children: HashMap<&'a str, usize>,
    /// The column that ends here, if one does.
    column: Option<usize>,
    /// The first column that came through here.
    first: usize,
}

/// Check the header rules that need no schema: no path has an empty
/// segment, no path appears twice, and no column is a prefix of another
/// (`a` beside `a.b`), since a row could then set one sub-message twice.
/// Returns the first column that breaks one, with the message.
///
/// A trie keeps this linear in the header's bytes: a parser meets this
/// header before any limit but `MaxMessageSize` applies to it, so comparing
/// every pair of columns would be quadratic in attacker-chosen input.
pub(crate) fn column_path_error(cols: &[String]) -> Option<(usize, String)> {
    let mut nodes: Vec<PathNode<'_>> = vec![PathNode::default()];
    for (i, c) in cols.iter().enumerate() {
        if c.starts_with('.') || c.ends_with('.') || c.contains("..") {
            return Some((
                i,
                format!("@dataset column {c:?}: a column path has an empty segment (draft §3.4.4)"),
            ));
        }
        let overlap = |prev: &str| {
            format!(
                "@dataset column {c:?} overlaps column {prev:?}: a field and a path inside it cannot both be columns (draft §3.4.4)"
            )
        };
        let mut at = 0;
        let mut created = false;
        for seg in c.split('.') {
            if let Some(prev) = nodes[at].column {
                return Some((i, overlap(&cols[prev])));
            }
            at = match nodes[at].children.get(seg) {
                Some(&next) => next,
                None => {
                    let next = nodes.len();
                    nodes.push(PathNode {
                        first: i,
                        ..Default::default()
                    });
                    nodes[at].children.insert(seg, next);
                    created = true;
                    next
                }
            };
        }
        if nodes[at].column.is_some() {
            return Some((
                i,
                format!("@dataset column {c:?} appears twice (draft §3.4.4)"),
            ));
        }
        if !created {
            return Some((i, overlap(&cols[nodes[at].first])));
        }
        nodes[at].column = Some(i);
    }
    None
}

/// A type written as a literal of its own, so a path can't go into it. A
/// `pxf.Secret` and a `google.protobuf.Any` have block forms people write,
/// so they aren't on the list.
fn is_literal_type(full: &str) -> bool {
    matches!(
        full,
        "google.protobuf.Timestamp"
            | "google.protobuf.Duration"
            | "google.protobuf.DoubleValue"
            | "google.protobuf.FloatValue"
            | "google.protobuf.Int64Value"
            | "google.protobuf.UInt64Value"
            | "google.protobuf.Int32Value"
            | "google.protobuf.UInt32Value"
            | "google.protobuf.BoolValue"
            | "google.protobuf.StringValue"
            | "google.protobuf.BytesValue"
            | "pxf.BigInt"
            | "pxf.Decimal"
            | "pxf.BigFloat"
    )
}

/// Check every column against the row type: each segment names a field, and
/// each segment but the last names a singular message field not written as
/// a literal. A path has no index, so it can't go through a repeated or map
/// field. A plain column is a path of one segment, so naming no field is a
/// header error for it too, whether its cells are empty or not.
///
/// A path deeper than `max_depth` is refused here, because a row's body is
/// built by recursing over the path before the decoder can count its
/// depth, and a recursive row type admits a path of any length.
pub(crate) fn check_column_paths(
    desc: &MessageDescriptor,
    cols: &[String],
    max_depth: usize,
) -> Option<String> {
    if let Some((_, msg)) = column_path_error(cols) {
        return Some(msg);
    }
    for c in cols {
        let mut md = desc.clone();
        let mut consumed = 0;
        let mut depth = 0;
        let mut segs = c.split('.').peekable();
        while let Some(seg) = segs.next() {
            let Some(fd) = md.get_field_by_name(seg) else {
                return Some(format!(
                    "@dataset column {c:?}: {} has no field {seg:?}",
                    md.full_name()
                ));
            };
            consumed += seg.len();
            if segs.peek().is_none() {
                break;
            }
            let at = &c[..consumed];
            consumed += 1; // the dot
            if fd.is_map() {
                return Some(format!(
                    "@dataset column {c:?}: {at:?} is a map field, and a column path cannot go through one; use a value cell for {at:?} (draft §3.4.4)"
                ));
            }
            if fd.is_list() {
                return Some(format!(
                    "@dataset column {c:?}: {at:?} is a repeated field, and a column path cannot go through one; use a value cell for {at:?} (draft §3.4.4)"
                ));
            }
            let Kind::Message(inner) = fd.kind() else {
                return Some(format!(
                    "@dataset column {c:?}: {at:?} is not a message field"
                ));
            };
            if is_literal_type(inner.full_name()) {
                return Some(format!(
                    "@dataset column {c:?}: {at:?} is a {}, which is written as a literal; a column path cannot go into it (draft §3.4.4)",
                    inner.full_name()
                ));
            }
            depth += 1;
            if depth > max_depth {
                return Some(format!(
                    "@dataset column {c:?}: path exceeds MaxNestingDepth={max_depth}"
                ));
            }
            md = inner;
        }
    }
    None
}
