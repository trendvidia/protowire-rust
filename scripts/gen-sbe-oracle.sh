#!/usr/bin/env bash
# Regenerate the oracles under crates/protowire-sbe/testdata/conformance/ by
# running protowire-go's SBE codec over each <name>-cases.tsv there, writing
# <name>-oracle.tsv, against the descriptor set the port reads: bounds (#122,
# what the SBE wire can hold). The reference is read from the sibling
# checkout's REF (default origin/main), exported so its working tree does not
# matter.

set -euo pipefail

repo_root="$(cd "$(dirname "$0")/.." && pwd)"
go_repo="$repo_root/../protowire-go"
ref="${REF:-origin/main}"
sha="$(git -C "$go_repo" rev-parse --short "$ref^{commit}")"
testdata="$repo_root/crates/protowire-sbe/testdata"

work="$(mktemp -d)"
trap 'rm -rf "$work"' EXIT
git -C "$go_repo" archive "$ref" | tar -x -C "$work"
mkdir -p "$work/scripts/sbe_oracle"
cp "$repo_root/scripts/sbe-oracle/main.go" "$work/scripts/sbe_oracle/main.go"

for name in ${@:-bounds}; do
  cases="$testdata/conformance/$name-cases.tsv"
  out="$testdata/conformance/$name-oracle.tsv"
  {
    echo "# What protowire-go at $sha does with each case of $name-cases.tsv."
    echo "# Produced by scripts/gen-sbe-oracle.sh. Do not edit."
    echo "# Columns: the case's, then its result (see scripts/sbe-oracle/main.go)."
    (cd "$work" && GOWORK=off go run ./scripts/sbe_oracle "$testdata/$name.binpb" "$cases")
  } > "$out.tmp"
  mv "$out.tmp" "$out"
  echo "wrote $out"
done
