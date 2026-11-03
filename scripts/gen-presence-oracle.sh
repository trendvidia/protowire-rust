#!/usr/bin/env bash
# Regenerate crates/protowire-pxf/testdata/conformance/presence-oracle.tsv by
# running protowire-go over presence-cases.tsv, against the same descriptor
# sets the port reads. The reference is read from the sibling checkout's
# REF (default origin/main), exported so its working tree does not matter.

set -euo pipefail

repo_root="$(cd "$(dirname "$0")/.." && pwd)"
go_repo="$repo_root/../protowire-go"
ref="${REF:-origin/main}"
sha="$(git -C "$go_repo" rev-parse --short "$ref")"
testdata="$repo_root/crates/protowire-pxf/testdata"
out="$testdata/conformance/presence-oracle.tsv"

work="$(mktemp -d)"
trap 'rm -rf "$work"' EXIT
git -C "$go_repo" archive "$ref" | tar -x -C "$work"
mkdir -p "$work/scripts/presence_oracle"
cp "$repo_root/scripts/presence-oracle/main.go" "$work/scripts/presence_oracle/main.go"

{
  echo "# What protowire-go at $sha does with each document of presence-cases.tsv."
  echo "# Produced by scripts/gen-presence-oracle.sh. Do not edit."
  echo "# Columns: descriptor set, message, document, then for UnmarshalFull and for"
  echo "# Unmarshal in turn: OK, pb hex, sorted null paths; or ERR, the error, empty."
  (cd "$work" && go run ./scripts/presence_oracle "$testdata" "$testdata/conformance/presence-cases.tsv")
} > "$out.tmp"
mv "$out.tmp" "$out"
echo "wrote $out"
