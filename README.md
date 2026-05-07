# protowire-rust

[![License: MIT](https://img.shields.io/badge/license-MIT-blue.svg)](LICENSE)
[![Crates.io](https://img.shields.io/crates/v/protowire.svg)](https://crates.io/crates/protowire)
[![docs.rs](https://img.shields.io/docsrs/protowire)](https://docs.rs/protowire)
[![CI](https://github.com/trendvidia/protowire-rust/actions/workflows/ci.yml/badge.svg)](https://github.com/trendvidia/protowire-rust/actions/workflows/ci.yml)

Rust port of [protowire](https://protowire.org) — a protobuf-backed
wire-format toolkit. Standalone (no FFI), descriptor-driven via
[`prost-reflect`](https://crates.io/crates/prost-reflect). Verified for
byte-equivalence against the canonical Go reference and seven other
sibling ports.

CI exercises **stable** × {Linux, macOS, Windows} plus **beta** on
Linux and an **MSRV (1.74)** pin, with `cargo fmt --check`,
`cargo clippy -- -D warnings`, and `cargo miri test` on the codec
crates as separate gating jobs.

## Crates

This is a Cargo workspace. The crates published to crates.io are:

| Crate | Purpose |
|---|---|
| [`protowire`](https://crates.io/crates/protowire) | Umbrella — re-exports the four sub-crates. |
| [`protowire-pb`](https://crates.io/crates/protowire-pb) | Schema-free protobuf wire codec. |
| [`protowire-envelope`](https://crates.io/crates/protowire-envelope) | API response envelope (status, data, error). |
| [`protowire-pxf`](https://crates.io/crates/protowire-pxf) | PXF (Proto eXpressive Format) text codec. |
| [`protowire-sbe`](https://crates.io/crates/protowire-sbe) | FIX SBE binary codec + XML schema conversion. |

The `bench-pxf`, `bench-sbe`, `dump-envelope`, and `check-decode`
workspace members are internal cross-port harnesses and stay
unpublished.

Vendored proto annotation sources live in `proto/` — they're the
cross-port wire contract (extension field numbers in the 50000s).

## Use it

```toml
[dependencies]
protowire = "0.70"
```

```rust
use protowire::{pxf, sbe, envelope, pb};
```

Or pull in just the sub-crate you need:

```toml
[dependencies]
protowire-pxf = "0.70"
protowire-pb  = "0.70"
```

## Build from source

```sh
cargo build --workspace
cargo test --workspace
```

Required: Rust 1.74+ (the workspace's `rust-version` pin). No external
`protoc` dependency at build time — `prost-build` ships a vendored
`protoc` binary.

## Command-line tool

The `protowire` CLI is shared across every port and lives in the spec
repo at
[github.com/trendvidia/protowire/cmd/protowire](https://github.com/trendvidia/protowire/tree/main/cmd/protowire).
Install:

```sh
go install github.com/trendvidia/protowire/cmd/protowire@latest
```

Rust users use this library for in-process encode/decode and the
shared CLI for command-line operations. There is no separate Rust
CLI binary.

## Wire compatibility

Verified for byte-equivalence against the canonical Go reference and
the other ports through:

```sh
bash ../protowire/scripts/cross_envelope_check.sh
```

The Rust dumper is on by default; pass `WITH_RUST=0` to skip it.

## Limitations & open gaps

The Rust port is descriptor-driven via
[`prost-reflect`](https://crates.io/crates/prost-reflect) —
`DynamicMessage` everywhere, no codegen-bound types. A few items fall
out of that or are explicit deferred work:

- **No native `BigInt` / `Decimal` / `BigFloat` implementations**
  (planned for 0.73.0). The codec faithfully encodes/decodes the
  bytes for the `pxf.*` arbitrary-precision schemas, but the
  user-facing type is `Vec<u8>` — callers convert to
  `num-bigint::BigInt` / `rust_decimal::Decimal` themselves. The Go
  reference's `bignum_test.go` is not yet ported (none of the sibling
  ports have it either).
- **No runtime `.proto` compilation.** The Go port uses `protocompile`
  to turn a `.proto` schema into a `FileDescriptorSet` in-process;
  the prost ecosystem has no comparable embeddable compiler. You must
  pre-build a `.binpb` `FileDescriptorSet` (with `buf build` or
  `protoc --include_imports --descriptor_set_out=…`) and load it via
  `DescriptorPool::decode`. This is also the reason SBE XML
  round-trip is not implemented here.
- **`prost-reflect` upstream-API drift.** The `FieldOptions`
  extension API has shifted shape across recent releases, so the
  workspace pins `prost-reflect = "0.14"` (and `prost = "0.13"`).
  Bumping either may require small migrations in `protowire-pxf`'s
  annotation reader; tracked but not breaking today.
- **The shared CLI lives in
  [trendvidia/protowire/cmd/protowire](https://github.com/trendvidia/protowire/tree/main/cmd/protowire),
  not here.** This repo ships only library crates plus the four
  cross-port harnesses.

### Implemented (mentioned because external reviews keep flagging them)

- **PXF decoder is the fused single-pass path** — mirrors Go's
  `decode_fast.go::unmarshalDirect`. The lexer drives a descriptor
  walk in lockstep and writes straight into `DynamicMessage`; there
  is no separate AST-walking slow path to swap in. See
  `crates/protowire-pxf/src/decode.rs`.
- **HARDENING.md decoder safety (M8)**: bounded recursion depth and
  PB length-prefix overflow rejection. The `check-decode` harness
  under `crates/check-decode/` runs the upstream adversarial corpus
  on every PR.

## Repository layout

```
protowire-rust/
├── LICENSE                                   # MIT
├── README.md
├── CHANGELOG.md
├── CONTRIBUTING.md, SECURITY.md,
│   GOVERNANCE.md, CODE_OF_CONDUCT.md
├── Cargo.toml                                # workspace + shared deps
├── Cargo.lock
├── crates/
│   ├── protowire/                            # umbrella
│   ├── protowire-pb/
│   ├── protowire-pxf/
│   ├── protowire-sbe/
│   ├── protowire-envelope/
│   ├── check-decode/                         # HARDENING corpus runner
│   ├── dump-envelope/, bench-pxf/, bench-sbe/  # cross-port harnesses
├── proto/                                    # vendored .proto annotations
└── .github/                                  # CI: build matrix + miri + CodeQL + publish
```
