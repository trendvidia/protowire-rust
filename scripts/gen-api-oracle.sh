#!/usr/bin/env bash
# Regenerate crates/protowire-pxf/testdata/conformance/api-oracle.tsv by running
# protowire-go's pxf API over each case of api-cases.tsv (#127): decoding with
# and without the post-decode pass, ApplyDefault, IsRequired and Default,
# against the same descriptor sets the port reads. The reference is read from
# the sibling checkout's REF (default origin/main), exported so its working
# tree does not matter.

set -euo pipefail

repo_root="$(cd "$(dirname "$0")/.." && pwd)"
go_repo="$repo_root/../protowire-go"
ref="${REF:-origin/main}"
sha="$(git -C "$go_repo" rev-parse --short "$ref^{commit}")"
testdata="$repo_root/crates/protowire-pxf/testdata"

work="$(mktemp -d)"
trap 'rm -rf "$work"' EXIT
git -C "$go_repo" archive "$ref" | tar -x -C "$work"
mkdir -p "$work/scripts/api_oracle"
cp "$repo_root/scripts/api-oracle/main.go" "$work/scripts/api_oracle/main.go"

out="$testdata/conformance/api-oracle.tsv"
{
  echo "# What protowire-go at $sha's pxf API does with each case of api-cases.tsv."
  echo "# Produced by scripts/gen-api-oracle.sh. Do not edit."
  echo "# Columns: descriptor set, message, op, its arguments, then the verdict:"
  echo "# full / full-skip: OK, pb hex, sorted present paths, sorted null paths; or ERR."
  echo "# marshal: OK and the text, or ERR. apply: OK and the pb hex, or ERR."
  echo "# required: the answer. default: whether there is one, and the literal."
  echo "# Tabs, newlines and backslashes in a verdict are escaped."
  (cd "$work" && GOWORK=off go run ./scripts/api_oracle "$testdata" "$testdata/conformance/api-cases.tsv")
} > "$out.tmp"
mv "$out.tmp" "$out"
echo "wrote $out"
