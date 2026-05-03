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

All 15 slices have landed (one commit each on `main`, mirroring
`protowire4ts/`'s git log). Module dependency order followed:
`envelope + pb` → `pxf` → `cli` → `sbe`.

| Slice | Tests | Status |
|---|---|---|
| 1. envelope | 11 | done |
| 2. pb wire + Message-trait codec + dump-envelope | 28 | done |
| 3. pxf-A tokens + lexer | 46 | done |
| 4. pxf-B AST + parser | 33 | done |
| 5. pxf-C formatter | 22 | done |
| 6. pxf-D1 scalars/nested/repeated/enum/oneof | 30 | done |
| 7. pxf-D2 maps + WKT | 18 | done |
| 8. pxf-D3 Any | 5 | done |
| 9. pxf-D4 annotations + `_null` FieldMask + `unmarshal_full` | 13 | done |
| 10. pxf-E encoder | 26 | done |
| 11. pxf-F CLI | 13 | superseded — shared CLI now lives in protowire/cmd/protowire/ |
| 12. sbe-A annotations + template + codec | 6 | done |
| 13. sbe-B marshal + unmarshal | 9 | done |
| 14. sbe-C View / GroupView | 8 | done |
| 15. sbe-D XML — saxlite + parse_xml_schema + xml_to_proto + proto_to_xml | 26 | done |

Workspace currently runs **304 tests** (one shy of the ~305 target —
one TS view test was inlined into another). `cargo clippy --workspace
--tests` is clean.

## Cross-port wire contracts (don't re-derive)

- `pb`: signed-int fields default to proto3 `int32`/`int64` (plain varint,
  10-byte sign-extension on negatives). Use `sint32`/`sint64` only when
  explicitly tagged. All five ports (Go/C++/TS/Java/Rust) agree per
  `protowire/scripts/cross_envelope_check.sh`. Canonical envelope: 129
  bytes (258 hex chars) starting `08 92 03 1a 04 de ad be ef 22 76 …`
  (the `22 76` is tag 4 length 118, framing a nested `AppError`).
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
3. **PXF lexer/parser/decoder**: hand-rolled. Matches Go and TS
   line-for-line. Rejected `nom`/`chumsky` to avoid semantic drift.
   The decoder is the *fused single-pass* path (mirrors Go's
   `decode_fast.go`), not an AST-walking variant — there's no separate
   slow path to swap in.
4. **XML library**: hand-rolled mini SAX (`crates/protowire-sbe/src/saxlite.rs`).
   The original plan was `quick-xml`, but the SBE schema vocabulary is
   small enough that a ~250-line hand-rolled parser maps line-for-line
   to the TS port and gives full control over error messages. Revisit
   if we ever support the full SBE XML grammar.
5. **Fixture pipeline**: checked-in `.binpb` `FileDescriptorSet` files
   loaded via `prost_reflect::DescriptorPool::decode`. Local fixtures
   build via `buf` against the repo-root `buf.yaml` workspace
   (`crates/protowire-{pxf,sbe}/testdata/*.binpb`); the shared canary
   `test.binpb` still uses `protoc` because its source lives in the
   sibling `protowire/` repo. Regenerate via `scripts/gen-testdata.sh`.
6. **Errors**: `thiserror` per crate.
7. **Annotations**: vendored locally in `proto/`. Read via
   `prost_reflect::DescriptorPool::get_extension_by_name` — when the
   FDS includes the annotations file (via `--include_imports`),
   prost-reflect resolves the extensions as known fields on
   `FieldOptions` / `MessageOptions` / `FileOptions`, so we don't need
   the TS port's raw-unknown-bytes fallback.

## Explicitly deferred

- pxf `BigInt` / `Decimal` / `BigFloat` (`bignum_test.go`) — not in any
  port yet.
- SBE XML round-trip via in-process protoc — Go has it (protocompile),
  TS doesn't, Rust likely won't unless we bind libprotoc.

## Conventions

- One commit per slice (mirror `protowire4ts/` git log). The slice plan
  has finished; further work is on a per-task basis.
- After every `pb` change: `bash ../protowire/scripts/cross_envelope_check.sh`.
  The script defaults to `WITH_RUST=1`; set `WITH_RUST=0` to skip Rust.
  Last verified all five ports (Go/C++/TS/Java/Rust) produce the same
  129-byte envelope.
- Shared canary: `../protowire/testdata/test.proto` + `example.pxf`.
  Don't fork — it's the cross-language contract. Compile to
  `FileDescriptorSet` `.binpb` at fixture-build time; per-port fixtures
  for port-specific tests live under each crate's `testdata/`.

## TS pitfalls that may transfer

- The shared `test.proto` is intentionally NOT modified for port-specific
  tests. Add fixtures under `crates/<crate>/testdata/` and a separate
  generation script.
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
- Raw `FieldOptions` unknown-bytes parsing: Rust uses
  `DescriptorPool::get_extension_by_name` instead. The TS port had to
  hand-decode varints because protobuf-es doesn't surface extensions as
  known fields; prost-reflect does.

## Useful sibling references

- `../protowire/` — Go canonical. Subpackages: `envelope/`, `encoding/pb/`,
  `encoding/pxf/`, `encoding/sbe/`, `cmd/protowire/main.go`, `proto/`,
  `testdata/`, `scripts/cross_envelope_check.sh`.
- `../protowire4ts/` — closest sibling for API shape (full module split
  landed; same descriptor-driven strategy).
- `../protowire4java/`, `../protowire4csharp/` — other standalone ports.
- Skip `../protowire4cpp/` and `../protowire4py/` — wrapper-style, not
  the model to follow.
