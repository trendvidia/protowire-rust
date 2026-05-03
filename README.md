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

- **No native `BigInt` / `Decimal` / `BigFloat` implementations** for the `pxf.*` schemas. The codec faithfully encodes/decodes the bytes, but the user-facing types are `Vec<u8>` rather than dedicated arbitrary-precision wrappers. Rust has `num-bigint` / `rust_decimal` available — wiring them up is open work.
- **SBE XML round-trip via in-process `protoc` is not implemented.** The Go reference uses `protocompile` to compile a `.proto` schema to a `FileDescriptorSet` at runtime; Rust doesn't have a comparable in-process compiler in the prost ecosystem. Workaround: pre-compile to `.binpb` and check the descriptor set into the testdata tree.
- **The shared CLI lives in [trendvidia/protowire/cmd/protowire](https://github.com/trendvidia/protowire/tree/main/cmd/protowire), not here.** This repo ships only library crates plus the three cross-port harnesses.
- **`prost` minor-version drift.** `prost-reflect`'s `FieldOptions` extension API has changed shape across recent releases; bumping the dependency may require small migrations in `protowire-pxf`'s annotation reader.

## Contributing & governance

This repository is part of the `protowire-*` family and is governed by [**Steward**](https://github.com/trendvidia/steward) — the meritocratic, AI-driven governance engine that runs all of the ports. Voting weight is per-directory expertise, the constitution is public in [`governance.pxf`](https://github.com/trendvidia/steward/blob/main/governance.pxf), and Steward routes draft / first-time PRs through a [private mentorship pipeline](https://github.com/trendvidia/steward#-private-mentorship-mode) so initial contributions get private feedback rather than public-review friction.

If any of the items above sound interesting, pull requests are welcome. New contributors start at zero trust and accumulate influence by shipping merged PRs in the directories they actually work on — the [escrow pipeline](https://github.com/trendvidia/steward#%EF%B8%8F-the-escrow-pipeline-zero-trust-onboarding) auto-routes large first-time PRs through 2–3 sandbox issues before unlocking them for community review.

See the [Steward README](https://github.com/trendvidia/steward) for a longer walkthrough of vector reputation, escrow, and the immune system.
