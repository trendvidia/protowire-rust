# Governance

`protowire-rust` is governed under the same constitution as the rest of
the `protowire-*` stack. The machine-readable source of truth lives in
the upstream spec repo at
[`governance.pxf`](https://github.com/trendvidia/protowire/blob/main/governance.pxf);
the human-readable preamble is at
[`GOVERNANCE.md`](https://github.com/trendvidia/protowire/blob/main/GOVERNANCE.md).

This file is a short pointer-doc. If anything below disagrees with the
upstream constitution, the upstream wins.

## Domain ownership

This repo's only domain vector is
[`protowire-rust`](https://github.com/trendvidia/protowire/blob/main/governance.pxf)
under the upstream `port-libraries` umbrella. Approval requirements:

| Path | Reviewer authority |
|---|---|
| `crates/protowire-pb/`, `crates/protowire-pxf/`, `crates/protowire-sbe/`, `crates/protowire-envelope/`, `crates/protowire/` | port maintainers (`@trendvidia/maintainers`); `unsafe` scrutiny on wire-codec paths |
| `proto/` | upstream spec maintainers — these mirror `trendvidia/protowire/proto/` and may not diverge |
| `crates/check-decode/`, `crates/dump-envelope/`, `crates/bench-*/` | port maintainers |
| `Cargo.toml`, `Cargo.lock`, `.cargo/` | port maintainers |
| `.github/workflows/publish.yml` | maintainers only — controls crates.io release surface |
| `.github/` (other) | port maintainers |

## What's enforced today vs (roadmap)

The Steward agent that enforces the constitution programmatically is
**rolling out**. Until it is live:

- Pull requests are reviewed by human maintainers.
- The `0.70.x` release line implements the wire contract documented in
  [`docs/grammar.ebnf`](https://github.com/trendvidia/protowire/blob/main/docs/grammar.ebnf)
  + [`docs/HARDENING.md`](https://github.com/trendvidia/protowire/blob/main/docs/HARDENING.md);
  the `check-decode` adversarial corpus run is the local enforcement
  of the hardening invariants.
- Reputation-weighted voting, automatic escrow for risky changes, and
  the `manifesto.blocked_module_globs` restriction are all `(roadmap)`
  per the upstream `governance.pxf`.

## Stable surfaces

Everything in these public modules is part of the SemVer contract for
the corresponding crate:

- `protowire::pb` (re-export of `protowire_pb`)
- `protowire::pxf` (re-export of `protowire_pxf`)
- `protowire::sbe` (re-export of `protowire_sbe`)
- `protowire::envelope` (re-export of `protowire_envelope`)

Each sub-crate's public API is also covered. Anything in a module
named `internal` or prefixed `_` is not stable.

The wire contract — what bytes a given proto message produces — is
governed by the **upstream** spec, not this port. Bumping the wire
contract requires a coordinated PR landing in every sibling port; see
[`STABILITY.md`](https://github.com/trendvidia/protowire/blob/main/STABILITY.md)
upstream.

## `unsafe` particulars

`unsafe` blocks in the wire-codec hot paths touch a class of bugs the
managed-runtime ports do not (UB, aliasing violations, lifetime
mistakes that escape the borrow checker). The constitution treats those
as a higher-severity tier:

- Any new `unsafe` block needs explicit maintainer approval and a
  `# Safety` comment per the `clippy::undocumented_unsafe_blocks` lint.
- The Miri job on PRs is mandatory for changes that touch any `unsafe`
  block — a Miri failure is identical to an ASan/UBSan hit.
- New uses of `unsafe` outside the wire-codec hot paths
  (`crates/protowire-pb/src/wire/`) need a maintainer conversation
  before the PR.
