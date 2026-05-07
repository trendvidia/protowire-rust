# Security Policy

## Reporting a vulnerability

Email **security@trendvidia.com** with a description, reproduction steps,
and the affected version(s) or commit(s). PGP key on request.

Please do **not** file public GitHub issues for vulnerabilities, and do
**not** post details in pull request comments.

You can expect:

- An acknowledgement within **3 business days**.
- A triage decision (accepted / not-a-vulnerability / needs-more-info)
  within **10 business days**.
- A coordinated fix on the timeline below.

## Scope

This policy covers `protowire-rust` — the Rust port of the `protowire`
stack. Cross-port issues are also accepted here and routed to the
upstream project; you can equivalently file at
[`trendvidia/protowire`](https://github.com/trendvidia/protowire) per
its [`SECURITY.md`](https://github.com/trendvidia/protowire/blob/main/SECURITY.md).

In scope:

- Decoder crashes, panics, infinite loops, unbounded memory, or OOMs
  triggered by adversarial PXF / PB / SBE / envelope input.
- Wire-format divergences from other ports for the same input that
  could be exploited (e.g. authorization bypass via parser
  disagreement).
- Schema-validation bypasses that let invalid messages reach
  application code.
- **`unsafe` blocks** in the wire-codec hot path: any soundness
  violation, undefined behaviour, or aliasing issue. These are the
  highest-severity class of report we receive — please flag even
  theoretical paths.

Out of scope:

- Denial-of-service via legitimately large inputs that respect the
  limits in the upstream
  [`docs/HARDENING.md`](https://github.com/trendvidia/protowire/blob/main/docs/HARDENING.md).
- Issues in `prost` / `prost-reflect` / `quick-xml` themselves — file
  those upstream against the respective crate and CC us.

## Hardening floor

The decoder paths are exercised against the upstream adversarial corpus
([`testdata/adversarial/`](https://github.com/trendvidia/protowire/tree/main/testdata/adversarial))
on every PR via the `check-decode` crate. A regression on the corpus
blocks merge. The workspace also runs under `cargo miri test` for the
codec crates on each PR; a Miri failure is treated identically to an
ASan/UBSan hit.

## Coordinated disclosure

For vulnerabilities affecting **more than one port**, a **30-day
embargo** applies from the date we acknowledge your report (per the
upstream project's policy), extendable by mutual agreement when a fix
needs more time.

Single-port issues follow this port's own disclosure timeline,
typically 7–14 days, but always at least long enough for a fix to be
released.

## Hall of fame

Reporters who follow coordinated disclosure are credited in
`SECURITY-ADVISORY-*.md` advisories on the upstream repo and (with
permission) in the release notes. We do not currently run a paid
bug-bounty program.
