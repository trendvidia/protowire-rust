# Changelog

All notable changes to `protowire-rust` are documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

The version number is kept aligned with the rest of the `protowire-*`
stack — releases bump in lockstep across language ports when the wire
format changes.

## [Unreleased]

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
