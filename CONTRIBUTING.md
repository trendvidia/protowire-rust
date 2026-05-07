# Contributing to protowire-rust

Welcome — this is the Rust port of [protowire](https://protowire.org), a
language-neutral wire-format toolkit. It tracks the canonical specification
in [`trendvidia/protowire`](https://github.com/trendvidia/protowire) and is
one of nine sibling ports (Go, C++, Rust, Java, TypeScript, Python, C#,
Swift, Dart). The port is standalone (no FFI) and descriptor-driven via
[`prost-reflect`](https://crates.io/crates/prost-reflect).

> **Steward integration is rolling out.** The governance described in
> [GOVERNANCE.md](GOVERNANCE.md) is the steady-state model. While Steward
> is being finalised, pull requests are reviewed by human maintainers in
> the conventional way — open a PR, expect review, iterate.

## Where bugs go

| Symptom | File against |
|---|---|
| Rust port-only crash, wrong API ergonomics, performance regression in this port only | `trendvidia/protowire-rust` |
| The same input produces different output here vs another port | upstream [`trendvidia/protowire`](https://github.com/trendvidia/protowire) (cross-port wire-equivalence regression) |
| Spec / grammar / proto annotation question | upstream [`trendvidia/protowire`](https://github.com/trendvidia/protowire) |
| Decoder crash / hang / OOM on adversarial input | **email security@trendvidia.com**, do not file public issue (see [SECURITY.md](SECURITY.md)) |

## Toolchain

Rust 1.74+ (the workspace uses `edition = "2021"` and depends on `prost
0.13`). Tested in CI on:

- `stable` × {Linux, macOS, Windows}
- `beta` × Linux (early-warning for breaking changes)
- MSRV pin on Linux (lowest supported toolchain)

Plus `cargo fmt --check` + `cargo clippy --all-targets --all-features --
-D warnings` as separate gating jobs.

## Local development

```sh
# Build + test the whole workspace
cargo build --workspace
cargo test --workspace

# Run benches (release mode)
cargo bench -p protowire-pxf
cargo bench -p protowire-sbe

# Run a single crate's tests
cargo test -p protowire-pxf

# HARDENING.md adversarial corpus (pulls in the upstream test fixtures)
cargo run -p check-decode -- --format pxf --input ../protowire/testdata/adversarial/pxf/...
```

### protobuf code-gen

`prost-build` is wired into each crate that needs generated proto code
via `build.rs`. No external `protoc` dependency at build time —
`prost-build` ships a vendored `protoc` binary.

## Sending changes

1. Open a draft PR early.
2. **For changes that touch parser/encoder behaviour**: comment with
   which fixtures from `crates/protowire-pxf/testdata/` (or the
   upstream adversarial corpus) you exercised. Cross-port
   wire-equivalence means a wrong move here can break six other ports'
   contracts.
3. **For changes that touch the wire format itself** — annotation field
   numbers in `proto/`, the PXF grammar, the SBE schema-id semantics —
   open the upstream PR in
   [`trendvidia/protowire`](https://github.com/trendvidia/protowire)
   first. This port shouldn't lead spec changes; it implements them.
4. **Anything that adds a new public symbol** must be re-exported from
   the umbrella `protowire` crate, not just live in the sub-crate.

## Code style

- `cargo fmt` is enforced in CI; your editor should be configured to
  format on save against the workspace `rustfmt.toml`.
- `cargo clippy --all-targets --all-features` runs in CI with
  `-D warnings`. Suppress with `#[allow(clippy::rule_name)]` and a
  one-line comment explaining why.
- Avoid `unsafe` outside of `crates/protowire-pb/src/wire/...` — the
  hot wire-codec path is the only place we currently accept it, and
  every block needs an `# Safety` comment per the `clippy::undocumented_unsafe_blocks` lint.
- New public APIs must have at least one rustdoc example exercising
  them. The example block should compile under `cargo test --doc`.
- Match the existing zero-allocation patterns in `protowire-sbe::View` —
  the `View` API is the "zero allocation" reference point.

## What we don't accept

- Changes that break wire-equivalence with another sibling port.
- New top-level dependencies without a one-line justification in the
  PR description. We currently depend only on the prost stack +
  thiserror + bytes + quick-xml.
- Static analysis suppressions on a whole file or whole module. Keep
  them function- or block-scoped.

## Releases

This port releases in lockstep with the rest of the `protowire-*` stack.
The version line is `0.70.x` for the first coordinated public release;
ports that share a `0.70.x` minor implement the same wire contract.

Cutting a release:

1. Bump `[workspace.package].version` in the root `Cargo.toml`.
2. Add a `## [X.Y.Z]` section to `CHANGELOG.md`.
3. Tag `vX.Y.Z` on `main`.
4. The `.github/workflows/publish.yml` workflow runs `cargo publish`
   in dependency order (`protowire-pb` → `protowire-envelope`,
   `protowire-pxf`, `protowire-sbe` → `protowire`) and posts a GitHub
   Release.
