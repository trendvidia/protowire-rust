# Changelog

All notable changes to `protowire-rust` are documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

The version number is kept aligned with the rest of the `protowire-*`
stack — releases bump in lockstep across language ports when the wire
format changes.

## [Unreleased]

### Added

- **`TableReader` streaming `@table` consumption + `bind_row`
  per-row binding** (draft §3.4.4). `unmarshal_full` materializes
  every row of an `@table` directive into `Presence::tables`; that
  works for small datasets and breaks for the CSV-replacement
  workload `@table` was designed for. New
  `protowire_pxf::table_reader` module exposes:
  - `TableReader<R: Read>::new(R)` — consumes leading directives
    and the `@table TYPE ( cols )` header from any `io::Read`
    source. Header capped at 64 KiB (`DEFAULT_HEADER_MAX_BYTES`)
    to fail-fast on misuse.
  - `type_name()` / `columns()` / `directives()` / `done()` accessors.
  - Implements [`Iterator`] (`Item = Result<TableRow, PxfError>`)
    so `for row in reader` just works; `next_row()` is the
    non-iterator entry point. Per-row arity and v1 cell-grammar
    checks happen at consume time. Errors are sticky.
  - `scan_one(desc, options)` — `next_row` + `bind_row` in one call;
    returns `Ok(Some(msg))` or `Ok(None)` at EOF. Named
    `scan_one` because `Iterator::scan` would shadow `scan`.
  - `tail()` — returns an `impl Read` that yields the buffered + the
    remaining underlying bytes, so callers can chain a second
    `TableReader` for multi-`@table` documents.
  - `bind_row(desc, columns, row, options)` — exported helper for
    callers iterating `Presence::tables()[i].rows` from the
    materializing path. Strategy is format-and-reparse — render
    cells as a synthetic PXF body and run through `unmarshal`,
    reusing every branch of the existing decoder. Callers in a
    tight scan loop typically set `options.skip_validate = true`.

- **`Presence::directives()` and `Presence::tables()` accessors.** The
  direct decoder now populates the document-root directive list and
  `@table` directive list on `Presence` during `unmarshal_full`, so
  consumers can read them after a decode call.
  - `Presence::directives()` returns the generic
    `@<name> *(prefix) [{ ... }]` blocks in source order, with raw
    body bytes (`Vec<u8>`) preserved verbatim for downstream re-
    parsing (chameleon's `@header T { ... }` reader, etc.). A single
    prefix populates the back-compat `type` field; two or more leave
    it empty and consumers read `prefixes` directly.
  - `Presence::tables()` returns the `@table` directives with full
    column metadata and parsed cell values per row, faithful to the
    three-state cell grammar (absent / present-but-null /
    present-with-value, draft §3.4.4). Cells are
    `Vec<Option<Value>>` — `None` for absent, `Some(Value::Null)`
    for present-but-null.
  - `unmarshal` (vs `unmarshal_full`) still passes no `Presence` and
    walks directives without allocating directive AST nodes — the
    direct path retains its zero-allocation prelude on the hot path.

- **PXF schema reserved-name validator (draft §3.13).** Rejects
  protobuf schemas that declare a message field, oneof, or enum value
  whose name is case-sensitively equal to a PXF value keyword
  (`null` / `true` / `false`) — such a name lexes as the keyword and
  the declared element is unreachable from PXF surface syntax. New
  `protowire_pxf::schema` module exposes:
  - `validate_descriptor(&MessageDescriptor)` /
    `validate_file(&FileDescriptor)` return a sorted
    `Vec<Violation { file, element, name, kind }>`.
  - `ViolationKind::{Field, Oneof, EnumValue}` and a `Display` impl
    that renders one-line human-readable text.
  - `UnmarshalOptions` gains `skip_validate: bool` for consumers that
    validate once at registry-load time and don't want the per-call
    recheck cost.
  - `unmarshal` and `unmarshal_full` invoke the validator before
    decode; violations come back as a `PxfError` with a multi-line
    message (one `Violation::to_string()` line per offender).
  - Synthetic oneofs from proto3 `optional` fields are filtered
    automatically — prost-reflect's `OneofDescriptor::is_synthetic()`
    matches the Go reference's `IsSynthetic()` filter.

- **PXF parser-side `@<name>` / `@entry` / `@table` directive grammar**
  (draft §3.4.2 – §3.4.4). The AST `Document` now carries `directives`
  (generic `@<name> *(prefix) [{ ... }]` entries) and `tables`
  (`@table <type> ( cols ) row*` entries) alongside `type_url` and
  `entries`. `Directive::body` preserves the raw bytes between `{`
  and `}`; `Directive::type` keeps the legacy single-prefix shape
  for v0.72.0-era consumers. `Document::body_offset` marks the byte
  right after the last directive (used by chameleon for hashing the
  schema-typed payload).

  Both the AST parser and the direct decoder consume the new forms;
  runtime semantics (`Presence` accessors, `TableReader` streaming,
  per-row `bind_row`) follow in subsequent PRs of the v0.72-v0.75
  catch-up. The decoder discards directive contents for now and
  enforces the standalone constraint (draft §3.4.4): a document
  containing any `@table` directive MUST NOT also carry `@type` or
  top-level field entries.

  `Position` gains an `offset` field (byte offset into the lexer's
  input) so directive body extraction can slice raw bytes; existing
  callers that read only line / column are unaffected.

## [0.70.0]

Initial public release. The version number aligns this port with the rest
of the `protowire-*` stack, which targets the 0.70.x series for the first
coordinated public release.

### Added

- **crates.io distribution** for the public crates: `protowire`
  (umbrella), `protowire-pb`, `protowire-pxf`, `protowire-sbe`,
  `protowire-envelope`. The `bench-pxf`, `bench-sbe`,
  `dump-envelope`, and `check-decode` workspace members are internal
  test harnesses and stay unpublished.
- **HARDENING.md decoder safety** (M8): bounded recursion depth and
  PB length-prefix overflow rejection in `protowire-pxf` and
  `protowire-pb`. Verified by the `check-decode` adversarial corpus
  reference under `crates/check-decode/`.
- **Comprehensive CI matrix**: build + test on stable/beta/MSRV across
  Linux/macOS/Windows, plus `cargo fmt --check`, `cargo clippy
  --all-targets --all-features -- -D warnings`, and `cargo miri test`
  on the codec crates. Weekly CodeQL SAST.
- **Governance scaffolding**: `LICENSE` (MIT), `CONTRIBUTING.md`,
  `SECURITY.md` (security@trendvidia.com), `GOVERNANCE.md`,
  `CODE_OF_CONDUCT.md`, `.github/CODEOWNERS`, issue + PR templates,
  Dependabot for cargo + GitHub Actions.

### Changed (breaking)

- **PXF parser stricter on key forms**, mirroring the upstream grammar
  tightening in
  [`trendvidia/protowire@8262bbb`](https://github.com/trendvidia/protowire/commit/8262bbb)
  (`docs/grammar.ebnf`, `docs/draft-trendvidia-protowire-00.txt`):
  - `=` (field assignment) and `{ … }` (submessage) now require an
    identifier key. Inputs like `123 = 234` or `child { 123 = 123 }`
    are now parse errors with
    `"field assignment with '=' requires an identifier key, got integer
    (\"123\"); use ':' for map entries"`.
  - `:` (map entry) is rejected at document top level — the document
    represents a proto message, never a `map<K,V>`. Use `=` for
    top-level field assignments. Map literals (`field = { 1: "x" }`)
    still work because `:` remains valid inside `{ … }` blocks.
