---
name: Feature request
about: Propose a Rust-port-only API addition or ergonomics improvement
title: "feat: "
labels: enhancement
---

<!--
Wire-format / spec / annotation proposals belong upstream at
trendvidia/protowire — they affect every port. This template is for
RUST-PORT-ONLY changes (better ergonomics, new convenience methods,
performance improvements that don't affect the wire output, support
for a new toolchain version).
-->

## Problem

What's awkward to express today, or what's missing?

## Proposal

What you'd like to add. If it's a new public API, sketch the signature
and the typical call-site. If it's a perf change, ideally include a
criterion bench number from `crates/protowire-pxf/benches/` or the
`bench-pxf` / `bench-sbe` harnesses.

## Alternatives considered

What else you tried, and why it isn't enough.

## Out of scope (optional)

Things this proposal is **not** trying to do, to keep review focused.
