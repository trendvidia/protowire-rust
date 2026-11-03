// SPDX-License-Identifier: MIT
// Copyright (c) 2026 TrendVidia, LLC.
//! Draft -01 § Recursion (trendvidia/protowire#502): a producer refuses a
//! message nested more deeply than `MaxNestingDepth`, counted as a decoder
//! reading the output counts it, so it never writes bytes the reader at the
//! same limit refuses. Mirrors protowire-go's `encoding/pb/marshal_depth_test.go`.

use std::collections::BTreeMap;

use protowire_pb::wire::{Reader, Result, WireType, Writer};
use protowire_pb::{
    marshal, marshal_with, read_message, unmarshal, unmarshal_with, write_map_entry, write_message,
    Error, Limits, MarshalOptions, Message,
};

/// `adversarial.v1.Tree`: a child, a map of message values and a map of
/// scalar values.
#[derive(Default, Debug, PartialEq)]
struct Tree {
    child: Option<Box<Tree>>,
    branches: BTreeMap<String, Tree>,
    tags: BTreeMap<String, String>,
}

#[derive(Default)]
struct BranchEntry {
    key: String,
    value: Tree,
}
impl Message for BranchEntry {
    fn encode_to(&self, _: &mut Writer) {
        unreachable!("entries are written by write_map_entry")
    }
    fn merge_field(&mut self, num: u32, wt: WireType, r: &mut Reader<'_>) -> Result<()> {
        match num {
            1 => self.key = r.string()?,
            2 => self.value = read_message(r)?,
            _ => r.skip(wt)?,
        }
        Ok(())
    }
}

#[derive(Default)]
struct TagEntry {
    key: String,
    value: String,
}
impl Message for TagEntry {
    fn encode_to(&self, _: &mut Writer) {
        unreachable!("entries are written by write_map_entry")
    }
    fn merge_field(&mut self, num: u32, wt: WireType, r: &mut Reader<'_>) -> Result<()> {
        match num {
            1 => self.key = r.string()?,
            2 => self.value = r.string()?,
            _ => r.skip(wt)?,
        }
        Ok(())
    }
}

impl Message for Tree {
    fn encode_to(&self, w: &mut Writer) {
        if let Some(c) = &self.child {
            write_message(w, 1, c.as_ref());
        }
        for (k, v) in &self.branches {
            write_map_entry(w, 4, k.as_str(), v);
        }
        for (k, v) in &self.tags {
            write_map_entry(w, 5, k.as_str(), v.as_str());
        }
    }
    fn merge_field(&mut self, num: u32, wt: WireType, r: &mut Reader<'_>) -> Result<()> {
        match num {
            1 => self.child = Some(Box::new(read_message(r)?)),
            4 => {
                let e: BranchEntry = read_message(r)?;
                self.branches.insert(e.key, e.value);
            }
            5 => {
                let e: TagEntry = read_message(r)?;
                self.tags.insert(e.key, e.value);
            }
            _ => r.skip(wt)?,
        }
        Ok(())
    }
}

/// `n` nested children under the root: n descents.
fn chain(n: usize) -> Tree {
    let mut t = Tree::default();
    for _ in 0..n {
        t = Tree {
            child: Some(Box::new(t)),
            ..Tree::default()
        };
    }
    t
}

/// `n` map entries, each holding the next tree as its value: 2n descents.
fn map_chain(n: usize) -> Tree {
    let mut t = Tree::default();
    for _ in 0..n {
        let mut parent = Tree::default();
        parent.branches.insert("b".into(), t);
        t = parent;
    }
    t
}

fn is_depth_error(r: &Result<Vec<u8>>, limit: usize) -> bool {
    matches!(r, Err(Error::DepthExceeded(n)) if *n == limit)
}

#[test]
fn submessages_at_the_limit_are_written_and_read() {
    let bytes = marshal(&chain(100)).expect("100 descents");
    let back: Tree = unmarshal(&bytes).expect("a reader at the default reads them");
    assert_eq!(back, chain(100));
}

#[test]
fn one_submessage_past_the_limit_is_refused() {
    let got = marshal(&chain(101));
    assert!(is_depth_error(&got, 100), "{got:?}");
    // The reader at the same limit refuses the same shape.
    let raised = marshal_with(
        &chain(101),
        MarshalOptions {
            max_nesting_depth: 101,
        },
    )
    .expect("a raised limit writes it");
    assert!(matches!(
        unmarshal::<Tree>(&raised),
        Err(Error::DepthExceeded(100))
    ));
}

#[test]
fn map_entries_count_as_descents() {
    // 50 entries with message values: 100 descents, at the bound.
    let bytes = marshal(&map_chain(50)).expect("100 descents");
    assert_eq!(unmarshal::<Tree>(&bytes).unwrap(), map_chain(50));

    // One more submessage at the deepest point: 101.
    let mut deeper = map_chain(50);
    let mut leaf = &mut deeper;
    while let Some(next) = leaf.branches.get_mut("b") {
        leaf = next;
    }
    leaf.child = Some(Box::default());
    assert!(is_depth_error(&marshal(&deeper), 100));

    // A scalar-valued entry is a descent too: 100 submessages and a tag
    // entry is 101, which the reader refuses.
    let mut tagged = chain(100);
    let mut leaf = &mut tagged;
    while let Some(next) = leaf.child.as_deref_mut() {
        leaf = next;
    }
    leaf.tags.insert("k".into(), "v".into());
    assert!(is_depth_error(&marshal(&tagged), 100));
    let mut at_bound = chain(99);
    let mut leaf = &mut at_bound;
    while let Some(next) = leaf.child.as_deref_mut() {
        leaf = next;
    }
    leaf.tags.insert("k".into(), "v".into());
    let bytes = marshal(&at_bound).expect("99 submessages and an entry");
    unmarshal::<Tree>(&bytes).expect("100 descents");
}

#[test]
fn the_limit_is_per_call() {
    let two = MarshalOptions {
        max_nesting_depth: 2,
    };
    assert!(marshal_with(&chain(2), two).is_ok());
    assert!(is_depth_error(&marshal_with(&chain(3), two), 2));

    // Raised, paired with a raised reader.
    let deep = MarshalOptions {
        max_nesting_depth: 500,
    };
    let bytes = marshal_with(&chain(500), deep).expect("500 under 500");
    let lim = Limits {
        max_nesting_depth: 500,
        ..Limits::default()
    };
    assert_eq!(unmarshal_with::<Tree>(&bytes, lim).unwrap(), chain(500));
}

#[test]
fn a_deep_chain_stops_at_the_limit() {
    // The writer stops descending at the limit, so a chain far past it is
    // refused without walking it.
    let deep = chain(100_000);
    assert!(is_depth_error(&marshal(&deep), 100));
    // Dropping the chain recurses once per level; leak it instead.
    std::mem::forget(deep);
}
