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
WITH_RUST=1 bash ../protowire/scripts/cross_envelope_check.sh
```

Off by default until Slice 1 (envelope) lands a real `dump-envelope`.

## Status

Slice 0 (scaffolding) only. See `CLAUDE.md` for the slice plan.
