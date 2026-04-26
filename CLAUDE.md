# CLAUDE.md

Notes for future Claude sessions working on this Rust port.

## What this is

Standalone Rust port of `github.com/trendvidia/protowire`. Mirrors the
TypeScript port (`protowire4ts/`) in scope and slice plan. Both are
descriptor-driven (Go protoreflect → prost-reflect / protobuf-es), not
codegen-bound.

The Go module at `../protowire/` is the canonical reference; line-for-line
ports of `encoding/pxf/*.go`, `encoding/sbe/*.go`, etc. are expected.

## Layout

Cargo workspace. Sub-crates under `crates/`. Vendored proto annotation
sources under `proto/`.

## Slice plan

Module dependency order: `envelope + pb` → `sbe` → `pxf` → `cli`.

Slice targets (from the TS port; aim for parity):

| Slice | Tests |
|---|---|
| envelope | 11 |
| pb | 28 |
| pxf-A tokens + lexer | 46 |
| pxf-B AST + parser | 33 |
| pxf-C formatter | 22 |
| pxf-D1 scalars/nested/repeated/enum/oneof | 30 |
| pxf-D2 maps + WKT | 18 |
| pxf-D3 Any | 5 |
| pxf-D4 annotations + `_null` FieldMask + `unmarshal_full` | 13 |
| pxf-E encoder | 26 |
| pxf-F CLI | 13 |
| sbe-A annotations + template + codec | 6 |
| sbe-B marshal + unmarshal | 9 |
| sbe-C View / GroupView | 8 |
| sbe-D XML — D1 reader, D2 ParseXMLSchema, D3 XMLToProto, D4 ProtoToXML | 26 |

Total target: ~305 tests across ~12,000 LOC.

## Cross-port wire contracts (don't re-derive)

- `pb`: signed-int fields default to proto3 `int32`/`int64` (plain varint,
  10-byte sign-extension on negatives). Use `sint32`/`sint64` only when
  explicitly tagged. All four ports agree per
  `protowire/scripts/cross_envelope_check.sh`. Canonical envelope: 138
  bytes starting `08 92 03 1a 04 de ad be ef 22 76 …`.
- `pxf` annotations: `pxf.required` = 50000, `pxf.default` = 50001;
  `_null` field of type `google.protobuf.FieldMask` carries null-survival
  across binary.
- `sbe` annotations: `sbe.schema_id` = 50100, `version` = 50101,
  `template_id` = 50200, `length` = 50300, `encoding` = 50301.
- `sbe` wire: 8-byte LE message header + 4-byte LE group header.

## Design calls (settled)

1. **Workspace**: Cargo workspace, sub-crates `protowire-{pb,pxf,sbe,envelope,cli}`,
   plus umbrella `protowire`, plus `dump-envelope` for cross-port check.
2. **Protobuf**: `prost-reflect` for descriptor / `DynamicMessage` (Go
   protoreflect / TS protobuf-es analog). `prost` for codegen of WKT
   types and annotation extension messages. Pure `prost` codegen would
   force concrete types and break the descriptor-driven decoder.
3. **PXF lexer/parser**: hand-rolled. Matches Go and TS line-for-line.
   Rejected `nom`/`chumsky` to avoid semantic drift.
4. **XML library**: `quick-xml`. TS hand-rolled SAX only because of
   npm dep aversion; we have no such constraint.
5. **Fixture pipeline**: `protoc --include_imports --descriptor_set_out=…`
   checked-in `.binpb` files, loaded via
   `prost_reflect::DescriptorPool::decode_file_descriptor_set`.
6. **Errors**: `thiserror` per crate.
7. **Annotations**: vendored locally in `proto/`. The decoder reads
   extension field numbers from raw `FieldOptions` unknown bytes (same
   approach as the TS port) — no global extension registration.

## Explicitly deferred

- `decode_fast.go` (~925 LOC fused single-pass decoder) — use the AST
  path for clarity; fast path lands when benchmarks demand it.
- pxf `BigInt` / `Decimal` / `BigFloat` (`bignum_test.go`) — not in any
  port yet.
- SBE XML round-trip via in-process protoc — Go has it (protocompile),
  TS doesn't, Rust likely won't unless we bind libprotoc.

## Conventions

- One commit per slice (mirror `protowire4ts/` git log).
- After every `pb` change: `WITH_RUST=1 bash ../protowire/scripts/cross_envelope_check.sh`.
- The cross-port script is gated on `WITH_RUST=1` until Slice 1 lands —
  flip the default in Slice 1's commit.
- Shared canary: `../protowire/testdata/test.proto` + `example.pxf`.
  Don't fork — it's the cross-language contract. Compile to
  `FileDescriptorSet` `.binpb` at fixture-build time; per-port fixtures
  for port-specific tests live under each crate's `testdata/`.

## TS pitfalls that may transfer

- The shared `test.proto` is intentionally NOT modified for port-specific
  tests. Add fixtures under `crates/<crate>/testdata/` and a separate
  generation script.
- `pxf.required` / `pxf.default` extensions aren't registered globally.
  Read from `FieldOptions` unknown bytes for field numbers 50000 (varint
  bool) and 50001 (length-delimited string). Length-delimited unknown
  payloads are recorded *with* the length prefix — strip it.
- "Null suppresses default": a field set to `null` in PXF is *present*
  (intentionally null), so post-decode must not overwrite it with a
  default. Required-validation skips null too.
- Map keys are sorted lexicographically by their formatted-string form,
  not numerically (`int_map = { 10: …, 2: … }` keeps that order).
- `1.5s` cannot be a single DURATION token — the `.` forces float lexing
  with no trailing-unit recheck. For sub-second durations, write `1500ms`.
- Encoder always skips message-typed fields when not set, even with
  `emit_defaults`. Scalars/lists/maps respect the flag. Tests for
  zero-value WKT must explicitly set the WKT to its zero (`dur_field = 0s`).

## TS pitfalls that DO NOT transfer

- BigInt: Rust has native `i64`/`u64`.
- No in-process protoc: same constraint in Rust unless we bind libprotoc;
  keep checked-in `.binpb` fixtures.
- npm dep aversion: doesn't apply — use `quick-xml`, `thiserror`, etc.

## Useful sibling references

- `../protowire/` — Go canonical. Subpackages: `envelope/`, `encoding/pb/`,
  `encoding/pxf/`, `encoding/sbe/`, `cmd/protowire/main.go`, `proto/`,
  `testdata/`, `scripts/cross_envelope_check.sh`.
- `../protowire4ts/` — closest sibling for API shape (full module split
  landed; same descriptor-driven strategy).
- `../protowire4java/`, `../protowire4csharp/` — other standalone ports.
- Skip `../protowire4cpp/` and `../protowire4py/` — wrapper-style, not
  the model to follow.
