<!--
For changes that touch wire-format behaviour: please open the upstream
PR in trendvidia/protowire FIRST. This port implements the spec; it
shouldn't lead spec changes. See CONTRIBUTING.md.

For changes touching `unsafe` blocks in the wire-codec hot path
(crates/protowire-pb/src/wire/...) or any new `unsafe` anywhere:
include a Miri-clean justification, and add a `# Safety` comment per
the clippy::undocumented_unsafe_blocks lint.
-->

## Summary

What this PR changes, in 1–3 sentences.

## Why

Link to the issue or upstream spec change that motivated this.

## Scope

- [ ] Wire-impacting source (`crates/protowire-{pb,pxf,sbe,envelope,}`)
- [ ] Vendored proto annotations (`proto/`)
- [ ] Test fixtures / harnesses (`crates/check-decode/`, `crates/dump-envelope/`, `crates/bench-*/`)
- [ ] Build / CI / repo plumbing (`Cargo.toml`, `.github/`)
- [ ] Documentation only

## Test plan

- [ ] `cargo build --workspace` clean
- [ ] `cargo test --workspace` clean
- [ ] `cargo fmt --all --check` clean
- [ ] `cargo clippy --all-targets --all-features -- -D warnings` clean
- [ ] If parser/encoder change: `cargo run -p check-decode --` clean against the upstream adversarial corpus
- [ ] If new or modified `unsafe`: `cargo miri test -p <affected-crate>` clean
- [ ] If wire-impacting: matching upstream spec PR linked above
- [ ] If new public symbol: re-exported from the umbrella `protowire`
      crate, not just the sub-crate
