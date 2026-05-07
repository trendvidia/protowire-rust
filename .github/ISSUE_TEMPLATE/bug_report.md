---
name: Bug report
about: Report a defect — wrong output, panic, parse error on valid input, etc.
title: "bug: "
labels: bug
---

<!--
Cross-port issues (the same input behaves differently on multiple ports)
belong upstream at trendvidia/protowire, not here. See CONTRIBUTING.md.

Security issues (decoder panic/hang/OOM on adversarial input, Miri
findings, soundness violations in `unsafe` blocks) go to
security@trendvidia.com instead. See SECURITY.md.
-->

## What happened

A clear description of the bug.

## How to reproduce

Smallest possible PXF / PB / SBE / envelope input + Rust snippet that
triggers it.

```rust
use protowire::pxf;
// ...
```

## What you expected

What you thought should happen.

## Versions

- crate version (`cargo tree -p protowire`):
- `rustc --version`:
- OS / arch:
- Cargo features enabled:

## Miri / sanitizer findings (if any)

If you can reproduce under `cargo miri test`, paste the report here.
A Miri hit is the highest-priority class of bug.
