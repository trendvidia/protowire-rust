#!/usr/bin/env bash
# Regenerate crates/protowire-pxf/testdata/conformance/tolerant-oracle.tsv by
# running protowire-go's Parse and ParseTolerant over each case of
# tolerant-cases.tsv (#130): the strict verdict, every error ParseTolerant
# reports, and the tree it returns. The reference is read from the sibling
# checkout's REF (default origin/main), exported so its working tree does not
# matter.

set -euo pipefail

repo_root="$(cd "$(dirname "$0")/.." && pwd)"
go_repo="$repo_root/../protowire-go"
ref="${REF:-origin/main}"
sha="$(git -C "$go_repo" rev-parse --short "$ref")"
testdata="$repo_root/crates/protowire-pxf/testdata"

work="$(mktemp -d)"
trap 'rm -rf "$work"' EXIT
git -C "$go_repo" archive "$ref" | tar -x -C "$work"
mkdir -p "$work/scripts/tolerant_oracle"
cp "$repo_root/scripts/tolerant-oracle/main.go" "$work/scripts/tolerant_oracle/main.go"

out="$testdata/conformance/tolerant-oracle.tsv"
{
  echo "# What protowire-go at $sha's Parse and ParseTolerant do with each case of tolerant-cases.tsv."
  echo "# Produced by scripts/gen-tolerant-oracle.sh. Do not edit."
  echo "# Columns: the case, Parse's verdict (OK, or ERR and the error), ParseTolerant's errors"
  echo "# (line:col:offset|msg joined by ' ; ', or NONE), and its tree in the dump format of"
  echo "# scripts/tolerant-oracle/main.go. Tabs, newlines and backslashes are escaped."
  (cd "$work" && GOWORK=off go run ./scripts/tolerant_oracle "$testdata" "$testdata/conformance/tolerant-cases.tsv")
} > "$out.tmp"
mv "$out.tmp" "$out"
echo "wrote $out"
