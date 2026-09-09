# Changelog

All notable changes to `protowire-rust` are documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

The version number is kept aligned with the rest of the `protowire-*`
stack — releases bump in lockstep across language ports when the wire
format changes.

## [Unreleased]

### Fixed

- **A dotted string map key is written bare** (draft `-01` § Entries and
  Keys, the Rust leg of
  [protowire#313](https://github.com/trendvidia/protowire/issues/313)).
  `is_valid_ident` (the marshaller) and `needs_quoting` (`format`)
  stopped at `[A-Za-z0-9_]`, while the grammar's *ident-part* — and
  `ident_safe_entry_name`, the test keyed entry names use — admits `.`;
  so a key `a.b` was marshalled `"a.b":` and a quoted `"a.b"` kept its
  quotes through `fmt`, where the text says bare. The decoder always read
  `a.b:` as one identifier token, so only the writers move: both now
  delegate to `ident_safe_entry_name`, one identifier-safe rule for the
  document. `".e"` and `"1.5"` still fail *ident-start* and stay quoted.
  The spec's third fmt pair, `fmt-dotted-keys`, is vendored and pinned.
  A canonical-text change for dotted string keys; no binding or wire
  change.

### Added

- **Keyed repeated fields** (draft `-01` §3.13; [#20](https://github.com/trendvidia/protowire-rust/issues/20),
  [protowire#116](https://github.com/trendvidia/protowire/issues/116)).
  A `repeated <Message>` field carrying `(pxf.key)` may be written as a
  block of named blocks — entry name = key-field value, entry order = list
  order — and the anonymous list form stays valid for the same field. The
  grammar accepts a quoted entry name anywhere (`field_entry = (identifier
  | string) …`); `Assignment` and `Block` record whether it was quoted, and
  the schema-less formatter reproduces it. The decoder reads the keyed
  block in both spellings (`name { }` and `name = { }`), rejects duplicate
  entry names (compared unquoted), the empty key in either form, and a
  disagreeing explicit key-field assignment, and rejects a quoted entry
  name outside a keyed block; the encoder emits the keyed form whenever
  every key is present, non-empty and distinct, unquoted iff
  identifier-safe, and the anonymous form otherwise. `canonicalize_keyed`
  is the schema-aware half of `pxf fmt`: anonymous → keyed where eligible,
  `name = { }` → `name { }`, identifier-safe quoted names unquoted,
  redundant key assignments dropped. The spec repo's `testdata/keyed/`
  corpus is vendored and driven by `tests/keyed.rs`. `Assignment` and
  `Block` gain public fields (breaking for struct-literal construction).
- **A repeated field bound more than once now concatenates** in document
  order, as draft `-01` § Entries and Keys says and the reference does; a
  second `tags = [...]` used to replace the first.

- **The v1.11 bind-time placement checks, over the import closure**
  ([#25](https://github.com/trendvidia/protowire-rust/issues/25)).
  `validate_descriptor` / `validate_file` now enforce, besides the
  reserved-name rule, `(pxf.key)` placement (draft `-01` §3.13.1: a
  repeated message-typed field whose value names a singular string field
  of the element message), `(pxf.default)` placement ("Default
  Placement": never a `repeated`, `map` or group field, nor a message
  type outside `Timestamp`, `Duration`, the nine `*Value` wrappers,
  `pxf.BigInt`, `pxf.Decimal`, `pxf.BigFloat`), and the two oneof rules
  ("Oneof Members": at most one `(pxf.default)` per oneof, and no
  `(pxf.required)` on a member). All four families cover the bound file
  **and its transitive imports** ("Scope of Bind-Time Checks"): a
  violation in an imported `.proto` is reported, attributed to the file
  that declares it, a diamond is checked once, and results are memoized
  per file descriptor at two grains (closure and file) keyed on
  descriptor identity, so the wider check costs less per decode than the
  single-file walk it replaces. `ViolationKind` gains `KeyOption`,
  `DefaultOption`, `RequiredOption`; `Violation` gains `detail`; the
  decode error header is `PXF schema violations:`. The vendored
  `pxf/annotations.proto` gains `key = 1316` and `annotations::get_key`
  reads it. **This narrows accepted schema input**: a schema carrying any
  of these placements bound before and is rejected now, over the whole
  closure — every rejected placement is one no implementation could
  honor, and the diagnostic names the field by fully-qualified name (the
  spec repo's STABILITY.md records the narrowing under v1.11).

- **`pxf.BigFloat` takes its literal form**
  ([#39](https://github.com/trendvidia/protowire-rust/issues/39)), closing
  the last gap in the arbitrary-precision types. A numeric literal on a
  `pxf.BigFloat` field decodes to the reference's bytes and the message
  marshals back to the reference's text, in every position and as a
  `(pxf.default)`. The reference parses with `math/big` at 256 bits and
  that conversion is not correctly rounded — it multiplies or divides by
  a power of five rounded to 320 bits and built by square-and-multiply at
  384 — so `src/bigfloat.rs` mirrors those steps on a hand-rolled big
  integer rather than computing the nearest value, and renders with the
  reference's `%g` at 78 digits. `testdata/bigfloat-oracle.tsv`, produced
  from protowire-go v1.6.0, pins 31 literals byte for byte, including the
  three range errors. No new dependency.
- **Every HARDENING § Mandatory limit is enforced, and configurable per
  call** ([#33](https://github.com/trendvidia/protowire-rust/issues/33)).
  `MaxMessageSize` (64 MiB, the total input to one decode or parse),
  `MaxBytesLiteralLength` and `MaxRepeatedCount` were not enforced at all
  — a 65 MiB document decoded — and `MaxNestingDepth` and
  `MaxNumericLiteralDigits` were fixed constants. New: `protowire_pxf::Limits`
  on `UnmarshalOptions::limits`, `parse_with_limits`,
  `DatasetReader::with_limits` (which also caps the bytes held while
  looking for a row boundary); `protowire_pb::Limits` with
  `unmarshal_with`, `Reader::with_limits`, and the element helpers
  `Reader::push_element` / `check_repeated` / `packed` a `Message` impl
  appends through so a repeated field is refused before the element past
  the bound is added; `protowire_sbe::Limits` with
  `Codec::from_files_with_limits`, refusing a group's `numInGroup` before
  any entry is allocated. The defaults are the constants, exported from
  each crate. Rejections name the limit (`input of N bytes exceeds
  MaxMessageSize=M`), and the PXF decoder now surfaces the lexer's own
  diagnostic at a value position (`invalid duration: 5seconds`, the bytes
  cap) instead of `expected bytes for field`. `check-decode` accepts
  `--limit NAME=VALUE` for the corpus rows that prove a limit with a small
  fixture (protowire#299), links `adversarial.v1.ListHolder`, and
  **decodes SBE for real** — its SBE leg returned "not implemented", which
  the harness reads as a rejection, so every SBE corpus row passed without
  decoding a byte.

- **`pxf.BigInt` and `pxf.Decimal` take their literal forms**
  ([#34](https://github.com/trendvidia/protowire-rust/issues/34)). A bare
  integer on a `pxf.BigInt` field and an integer or decimal literal on a
  `pxf.Decimal` field decode to the wire messages of `pxf/bignum.proto`
  (big-endian magnitude, sign flag, and for `Decimal` the scale exactly as
  written: `1.00` has scale 2), in singular, repeated and map-value
  positions and as a `(pxf.default)`; the encoder writes them back as
  literals. The conversions are hand-rolled in `src/bignum.rs` — no
  bignum dependency — and **`MaxNumericLiteralDigits` (4096) is enforced
  before they run**: a literal of exactly 4096 digits decodes, one more
  is rejected naming the limit. Before this a `pxf.BigInt` field rejected
  every literal with `expected '{'`, which passed the corpus's reject
  rows by accident and failed the accept row protowire#279 added.
  `pxf.BigFloat`'s literal form is still open
  ([#39](https://github.com/trendvidia/protowire-rust/issues/39);
  its block form is unchanged). `check-decode` links `adversarial.v1.BigNumHolder` and
  bounds `pxf.Decimal.scale` on the PB wire on both signs.

### Changed

- **Accepted input narrows at the HARDENING defaults**: a document, PB
  message or SBE buffer over 64 MiB is rejected where it decoded before,
  and the SBE decoder and `View` reject a wire `block_length` below the
  template's (HARDENING § SBE step 2), a zero entry `block_length` with a
  non-zero count (step 4), and a group whose `count × block_length` runs
  past the buffer — `View::group` used to accept the header and let
  `entry()` index out of bounds. STABILITY.md promise 1 calls the size cap
  a narrowing; the spec repo's v1.13 section records why the family takes
  it. The nesting-depth message is now `MaxNestingDepth=N` in every crate.
- **The encoder writes message shorthand for map values** — a
  `Timestamp`, `Duration` or `*Value` wrapper as a map value was written
  as a block (`at: { seconds = … }`) where singular and repeated positions
  and every other port write the literal (`at: 2026-01-01T00:00:00Z`).
  Both forms still read; marshal output for such maps changes.
- **A `pb` map entry always carries both its key and its value, zero-valued
  or not** ([#32](https://github.com/trendvidia/protowire-rust/issues/32);
  decided family-wide in
  [protowire#295](https://github.com/trendvidia/protowire/issues/295)).
  The envelope codec wrote a `metadata` entry the way it writes any
  message, with proto3 zero-skipping inside it, so `"" → ""` went out as
  `22 02 2a 00` where protobuf-go, protoc and C++ protobuf write
  `22 06 2a 04 0a 00 12 00`. Every reader in the family accepted all three
  layouts measured, so the divergence was lossless and invisible until the
  spec repo added a golden-checked wire vector. This **changes pb bytes**
  for entries with an empty key or value (STABILITY.md promise 2; the spec
  repo's v1.13 section records why they move); the reader still takes the
  old omission. `protowire_pb::write_map_entry` and the `MapEntryField`
  trait are new, so a `Message` impl no longer hand-rolls the entry — every
  hand-rolled one had got it wrong the same way. `dump-envelope --vector
  NAME` prints a named vector for the gate, and exits 3 with
  `not-implemented: NAME` for one this port has not built.
- **MSRV raised from 1.82 to 1.85.** `prost` 0.14.4 raised its own MSRV to
  rustc 1.85, and holding the workspace at 1.82 would have meant freezing
  a core decoding dependency off its upstream fix stream. Rust 1.85 shipped
  in February 2025. `[workspace.package].rust-version`, the CI gate, the
  README and CONTRIBUTING all move together.

  This is a compatibility change on five published 1.0.0 crates —
  `protowire`, `protowire-pb`, `protowire-envelope`, `protowire-pxf`,
  `protowire-sbe` — and by the usual Rust convention wants a minor
  release rather than a patch.

- Dependency bumps taken with it: `prost` 0.14.3 → 0.14.4, `prost-types`
  0.14.3 → 0.14.4, `prost-reflect` 0.16.4 → 0.16.5, `bytes` 1.11.1 →
  1.12.1, `thiserror` 2.0.18 → 2.0.20. No wire-format or API change.

### Fixed

- **`format` keeps the spelling of a map key where changing it would
  change what the key denotes**
  ([protowire#306](https://github.com/trendvidia/protowire/issues/306),
  option 2). Since #36 a bare `true` / `false` is a bool key, so
  `"true": "v"` on a `map<string, V>` and `true: "v"` are different
  documents — and the formatter wrote the second for the first, turning a
  document that binds into one that does not (`"null"` → `null`, no key at
  all, likewise). `ast::MapEntry` gains `quoted`, the parser records it,
  and the formatter reproduces it: a bare key stays bare; a quoted key is
  unquoted only when it is identifier-safe and not a value keyword. One
  existing output moves: a bare integer key was written quoted
  (`404:` → `"404":`) and stays bare now, as the marshaller has always
  written it. The spec repo's `fmt-keyword-keys` and `fmt-bare-keys`
  pairs are vendored under `testdata/map-keys/` and pinned as fixed
  points. `MapEntry` gains a public field, which is breaking for
  struct-literal construction.
- **The lexer reads fractional and `µs` duration literals** (`1.5ms`,
  `312.5µs`, `1h30m0.5s`, `2µs`)
  ([#26](https://github.com/trendvidia/protowire-rust/issues/26)). Draft
  `-01` §3.3 admits `duration-segment = 1*DIGIT [ "." 1*DIGIT ] time-unit`
  with `µs` (U+00B5) among the units, and the encoder writes exactly those
  forms for any `google.protobuf.Duration` that is not a whole multiple of
  its largest unit — so this port could not read its own output for a
  measured latency. Two defects inherited from the Go reference lexer
  (fixed there in protowire-go#76): the number path decided *float* on
  seeing `.` before it looked for a unit, and the duration scan was
  ASCII-only. The lexer now consumes an optional fraction first and takes
  the duration branch when a unit (or the `C2 B5` micro sign) follows;
  `1.5` stays a float, `1.5e3ms` stays a float plus an identifier, `1.ms`
  stays `1.` plus an identifier, and U+03BC GREEK SMALL LETTER MU is not a
  unit. The 47-case token table from protowire-go and a marshal → read-back
  property test over every `Duration.String()` branch pin it. An illegal
  non-ASCII character is now reported as one token naming the character,
  not one per UTF-8 byte.
  
- **Bool map keys accept exactly the spellings the grammar admits**
  ([#31](https://github.com/trendvidia/protowire-rust/issues/31)). A
  `map<bool, V>` key was matched against the text `true` / `false` only, so
  the bare integers `0` / `1` — the "bool encoded as 0/1" spelling draft
  `-01` § Entries and Keys names, which Go and Java bind — were a syntax
  error here. A bool key is now the keyword `true` / `false` bare (decided
  in [protowire#284](https://github.com/trendvidia/protowire/issues/284)),
  the bare integers `0` / `1`, or the quoted literals `"true"` / `"false"`;
  every other spelling (`t`, `TRUE`, `yes`, `"1"`, `"0"`, `"TRUE"`, …) is an
  error naming the key. The keyword bare on a `map<string, V>` is rejected
  too — the string is spelled quoted — and the AST parser admits a bool key
  with the `:` tail so `format` and `validate` see the same documents the
  decoder does. The spec repo's `testdata/map-keys/` corpus is vendored
  under `crates/protowire-pxf/testdata/map-keys/` and driven by
  `tests/map_keys.rs`.
  
- **`(pxf.default)` on a oneof member no longer destroys the arm the
  document chose** ([#24](https://github.com/trendvidia/protowire-rust/issues/24)).
  `post_decode` tested presence per field, so a member's default was
  applied over a chosen sibling — and setting a oneof member clears the
  rest, so `a = "written"` decoded as `b = "bbb"` with `a` gone. The
  default now applies only when no member of the oneof is present in the
  document (a member bound to `null` counts as present), per draft `-01`
  §annotation-extensions "Oneof Members". A proto3 `optional` field's
  synthetic oneof is excluded, so its default keeps applying.
  
- **`(pxf.default)` on a `repeated` or `map` field is an error, not a
  one-element list** ([#23](https://github.com/trendvidia/protowire-rust/issues/23)).
  `apply_default` dispatched on the element kind and handed `set_field` a
  scalar for a list field, which prost-reflect accepts and encodes as
  `tags = ["ignored"]` — pb bytes no other port emits. Draft `-01`
  "Default Placement" forbids inventing that semantics; the decode now
  fails with `default values not supported for repeated field "tags"`
  (the message protowire-typescript and protowire-go already produce),
  and the map case names the placement rather than the synthetic
  `…Entry` type. The bind-time half of the rule is
  [#25](https://github.com/trendvidia/protowire-rust/issues/25).
  
- README and CONTRIBUTING both claimed an MSRV of **1.74**, which had not
  matched `Cargo.toml` since the pin moved to 1.82. Both now state 1.85,
  and CONTRIBUTING no longer describes the workspace as depending on
  `prost 0.13`.

## [1.0.0] — 2026-05-13

First major-version cut. Implements the three one-time spec changes
from the [protowire v1.0 freeze line](https://github.com/trendvidia/protowire/releases/tag/v1.0.0)
in lockstep with the other v1.0 ports. **Breaking** — there is no
alias period; v1.0 is itself the major bump.

### v1.0 spec changes

- **`@table` → `@dataset` rename** (draft §3.4.4). Public API
  rename: `ast::TableDirective` → `ast::DatasetDirective`,
  `ast::TableRow` → `ast::DatasetRow`, `TableReader` →
  `DatasetReader`, `Presence::tables()` → `Presence::datasets()`,
  `Presence::add_table()` → `Presence::add_dataset()`,
  `TokenKind::AtTable` → `TokenKind::AtDataset`. Source files
  `src/table_reader.rs` → `src/dataset_reader.rs`;
  `tests/table_reader.rs` → `tests/dataset_reader.rs`. Hard cutover.

- **`@proto` directive added** (draft §3.4.5). New `ast::ProtoDirective`
  + `ast::ProtoShape` enum (`Anonymous`, `Named`, `Source`,
  `Descriptor`). Four body shapes lexically distinguished.
  Exposed via `Document::protos` and `Presence::protos()`.
  Descriptor form is the MUST-support shape; this port supports all
  four.

- **Reserved directive names** expanded from 5 to 13 (draft §3.4.6).
  `schema::is_future_reserved_directive(name)` exported. Parser +
  fast decoder reject `@table`, `@datasource`, `@view`,
  `@procedure`, `@function`, `@permissions` as spec-reserved.

`@dataset`'s row message type is now optional in the AST — binding
to an anonymous `@proto` per draft §3.4.4 Anonymous binding.
`Lexer::reposition_to(target)` and `Lexer::input_bytes()` added so
the parser can skip past an `@proto` brace-body whose interior is
protobuf source rather than PXF.

### Build

- Workspace version `0.75.0` → `1.0.0` (Cargo.toml + inter-crate
  path-dep version pins).

### Tests

- New `crates/protowire-pxf/tests/proto_directive.rs` with 13 cases
  covering all four `@proto` body shapes, anonymous binding,
  multi-`@proto`, nested-brace bodies, three error paths, parametric
  reserved-name rejection, and `ProtoShape::name()` lookup.
- `cargo test --workspace`: all tests pass.

## [0.75.0] — 2026-05-12

First release after the v0.70.0 baseline that closes the v0.72–v0.75
gap with the rest of the `protowire-*` stack (Go, Java, cpp, python,
TypeScript). All four PXF v0.72-series features are now available in
the Rust port, in lockstep with what the sibling ports shipped over
their v0.72 → v0.74 → v0.75 cuts. The Rust port skips intermediate
version numbers and lands the bundled feature set directly on v0.75.0
to match the active wire revision.

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
