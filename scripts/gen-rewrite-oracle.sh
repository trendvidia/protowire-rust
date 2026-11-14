#!/usr/bin/env bash
# Regenerate crates/protowire-pxf/testdata/conformance/rewrite-oracle.tsv by
# running protowire-go's Rewriter over each case of rewrite-cases.txt (#131):
# each operation's verdict and the rewritten document. The reference is read
# from the sibling checkout's REF (default origin/main), exported so its
# working tree does not matter.

set -euo pipefail

repo_root="$(cd "$(dirname "$0")/.." && pwd)"
go_repo="$repo_root/../protowire-go"
ref="${REF:-origin/main}"
sha="$(git -C "$go_repo" rev-parse --short "$ref^{commit}")"
conformance="$repo_root/crates/protowire-pxf/testdata/conformance"

work="$(mktemp -d)"
trap 'rm -rf "$work"' EXIT
git -C "$go_repo" archive "$ref" | tar -x -C "$work"
mkdir -p "$work/scripts/rewrite_oracle"
cp "$repo_root/scripts/rewrite-oracle/main.go" "$work/scripts/rewrite_oracle/main.go"

out="$conformance/rewrite-oracle.tsv"
{
  echo "# What protowire-go at $sha's Rewriter does with each case of rewrite-cases.txt."
  echo "# Produced by scripts/gen-rewrite-oracle.sh. Do not edit."
  echo "# Columns: the case, then its document, its operations and protowire-go's results, each"
  echo "# base64; scripts/rewrite-oracle/main.go says how each is encoded."
  (cd "$work" && GOWORK=off go run ./scripts/rewrite_oracle "$conformance/rewrite-cases.txt")
} > "$out.tmp"
mv "$out.tmp" "$out"
echo "wrote $out"
