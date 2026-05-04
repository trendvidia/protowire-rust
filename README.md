# protowire4rust

Rust port of [`github.com/trendvidia/protowire`](https://github.com/trendvidia/protowire).
Standalone (no FFI), descriptor-driven via [`prost-reflect`].

Sister ports: Go (canonical), C++, TypeScript, Java, C#, Python.
Wire-format equivalence is verified across ports by
`protowire/scripts/cross_envelope_check.sh`.

## Layout

This is a Cargo workspace.

| Crate | Purpose |
|---|---|
| `protowire` | Umbrella — re-exports the four sub-crates. |
| `protowire-pb` | Schema-free protobuf wire codec. |
| `protowire-envelope` | API response envelope (status, data, error). |
| `protowire-pxf` | PXF (Proto eXpressive Format) text codec. |
| `protowire-sbe` | FIX SBE binary codec + XML schema conversion. |
| `dump-envelope`, `bench-pxf`, `bench-sbe` | Cross-port harnesses (test infrastructure). |

Vendored proto annotation sources live in `proto/` — they're the
cross-port wire contract (extension field numbers in the 50000s).

## Build

```sh
cargo build --workspace
cargo test --workspace
```

## Command-line tool

The `protowire` CLI is shared across every port and lives in the spec repo at
[github.com/trendvidia/protowire/cmd/protowire](https://github.com/trendvidia/protowire/tree/main/cmd/protowire). Install:

```sh
go install github.com/trendvidia/protowire/cmd/protowire@latest
```

Rust users use this library for in-process encode/decode and the shared CLI
for command-line operations. There is no separate Rust CLI binary.

## Cross-port wire check

After touching `protowire-pb` or `protowire-envelope`:

```sh
bash ../protowire/scripts/cross_envelope_check.sh
```

The Rust dumper is on by default; pass `WITH_RUST=0` to skip it (useful when the Rust toolchain isn't available locally).

## Status

All 15 originally-planned slices have landed; the workspace runs ~300 tests across `pb`, `pxf`, `sbe`, and `envelope`. See `CLAUDE.md` for the per-slice breakdown and design calls.

## Limitations & open gaps

The Rust port is descriptor-driven via [`prost-reflect`](https://crates.io/crates/prost-reflect) — `DynamicMessage` everywhere, no codegen-bound types. A few items fall out of that or are explicit deferred work:

- **No native `BigInt` / `Decimal` / `BigFloat` implementations** (planned for 0.73.0). The codec faithfully encodes/decodes the bytes for the `pxf.*` arbitrary-precision schemas, but the user-facing type is `Vec<u8>` — callers convert to `num-bigint::BigInt` / `rust_decimal::Decimal` themselves. The Go reference's `bignum_test.go` is not yet ported (none of the sibling ports have it either).
- **No runtime `.proto` compilation.** The Go port uses `protocompile` to turn a `.proto` schema into a `FileDescriptorSet` in-process; the prost ecosystem has no comparable embeddable compiler. You must pre-build a `.binpb` `FileDescriptorSet` (with `buf build` or `protoc --include_imports --descriptor_set_out=…`) and load it via `DescriptorPool::decode`. This is also the reason SBE XML round-trip is not implemented here.
- **`prost-reflect` upstream-API drift.** The `FieldOptions` extension API has shifted shape across recent releases, so the workspace pins `prost-reflect = "0.14"` (and `prost = "0.13"`). Bumping either may require small migrations in `protowire-pxf`'s annotation reader; tracked but not breaking today.
- **The shared CLI lives in [trendvidia/protowire/cmd/protowire](https://github.com/trendvidia/protowire/tree/main/cmd/protowire), not here.** This repo ships only library crates plus the three cross-port harnesses.

### Implemented (mentioned because external reviews keep flagging them)

- **PXF decoder is the fused single-pass path** — mirrors Go's `decode_fast.go::unmarshalDirect`. The lexer drives a descriptor walk in lockstep and writes straight into `DynamicMessage`; there is no separate AST-walking slow path to swap in. See `crates/protowire-pxf/src/decode.rs`.

## Contributing & governance

This repository is part of the `protowire-*` family and is governed by [**Steward**](https://github.com/trendvidia/steward) — the meritocratic, AI-driven governance engine that runs all of the ports. Voting weight is per-directory expertise, the constitution is public in [`governance.pxf`](https://github.com/trendvidia/steward/blob/main/governance.pxf), and Steward routes draft / first-time PRs through a [private mentorship pipeline](https://github.com/trendvidia/steward#-private-mentorship-mode) so initial contributions get private feedback rather than public-review friction.

If any of the items above sound interesting, pull requests are welcome. New contributors start at zero trust and accumulate influence by shipping merged PRs in the directories they actually work on — the [escrow pipeline](https://github.com/trendvidia/steward#%EF%B8%8F-the-escrow-pipeline-zero-trust-onboarding) auto-routes large first-time PRs through 2–3 sandbox issues before unlocking them for community review.

See the [Steward README](https://github.com/trendvidia/steward) for a longer walkthrough of vector reputation, escrow, and the immune system.
