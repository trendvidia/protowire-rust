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
| `protowire-cli` | `protowire` CLI binary (encode / decode / validate / fmt). |
| `dump-envelope` | Cross-port wire-compat dumper (mirrors siblings). |

Vendored proto annotation sources live in `proto/` — they're the
cross-port wire contract (extension field numbers in the 50000s).

## Build

```sh
cargo build --workspace
cargo test --workspace
```

## Cross-port wire check

After touching `protowire-pb` or `protowire-envelope`:

```sh
WITH_RUST=1 bash ../protowire/scripts/cross_envelope_check.sh
```

Off by default until Slice 1 (envelope) lands a real `dump-envelope`.

## Status

Slice 0 (scaffolding) only. See `CLAUDE.md` for the slice plan.
