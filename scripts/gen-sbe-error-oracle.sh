#!/usr/bin/env bash
# Regenerate crates/protowire-sbe/testdata/conformance/sbe-error-oracle.tsv:
# what protowire-go's SBE codec says about damaged input (#144), from every
# prefix of a full encoding of each sbe-test.proto message, hand-made damage,
# View accessor reads, SOFH streams and XML schemas. The reference is read
# from the sibling checkout's REF (default origin/main), exported so its
# working tree does not matter.

set -euo pipefail

repo_root="$(cd "$(dirname "$0")/.." && pwd)"
go_repo="$repo_root/../protowire-go"
ref="${REF:-origin/main}"
sha="$(git -C "$go_repo" rev-parse --short "$ref^{commit}")"
testdata="$repo_root/crates/protowire-sbe/testdata"

work="$(mktemp -d)"
trap 'rm -rf "$work"' EXIT
git -C "$go_repo" archive "$ref" | tar -x -C "$work"
mkdir -p "$work/scripts/sbe_error_oracle"
cp "$repo_root/scripts/sbe-error-oracle/main.go" "$work/scripts/sbe_error_oracle/main.go"

out="$testdata/conformance/sbe-error-oracle.tsv"
{
  echo "# What protowire-go at $sha's SBE codec says about each input."
  echo "# Produced by scripts/gen-sbe-error-oracle.sh. Do not edit."
  echo "# Columns: op, subject, argument, input hex, verdict (see scripts/sbe-error-oracle/main.go)."
  (cd "$work" && GOWORK=off go run ./scripts/sbe_error_oracle \
    "$testdata/sbe-test.binpb" "$testdata/go-sbe-stream.bin" "$testdata/convert/fixed-arrays.xml")
} > "$out.tmp"
mv "$out.tmp" "$out"
echo "wrote $out"
