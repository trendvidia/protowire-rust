#!/usr/bin/env bash
# Regenerate crates/protowire-pxf/testdata/conformance/keyed-oracle.tsv by running
# protowire-go over each case of keyed-cases.tsv (#130): the typed KeyedError
# of Unmarshal, UnmarshalFull and a @dataset scan, and KeyedDiagnostics over
# the parsed document, against the same descriptor sets the port reads. The
# reference is read from the sibling checkout's REF (default origin/main),
# exported so its working tree does not matter.

set -euo pipefail

repo_root="$(cd "$(dirname "$0")/.." && pwd)"
go_repo="$repo_root/../protowire-go"
ref="${REF:-origin/main}"
sha="$(git -C "$go_repo" rev-parse --short "$ref^{commit}")"
testdata="$repo_root/crates/protowire-pxf/testdata"

work="$(mktemp -d)"
trap 'rm -rf "$work"' EXIT
git -C "$go_repo" archive "$ref" | tar -x -C "$work"
mkdir -p "$work/scripts/keyed_oracle"
cp "$repo_root/scripts/keyed-oracle/main.go" "$work/scripts/keyed_oracle/main.go"

out="$testdata/conformance/keyed-oracle.tsv"
{
  echo "# What protowire-go at $sha's pxf API does with each case of keyed-cases.tsv."
  echo "# Produced by scripts/gen-keyed-oracle.sh. Do not edit."
  echo "# Columns: descriptor set, message, op, document, then three verdicts."
  echo "# doc: Unmarshal, UnmarshalFull, KeyedDiagnostics of Parse (NONE, or PARSE and its error)."
  echo "# scan: NewDatasetReader and Scan of every row (OK n, NEW ..., ROW n ...), then - -."
  echo "# A verdict is OK, KEYED kind|field|key|line:col|offset|msg and the error text, or ERR and the text."
  echo "# A diagnostic is kind|field|key|line:col|offset|msg; several are joined by ' ; '."
  echo "# Tabs, newlines and backslashes in a verdict are escaped."
  (cd "$work" && GOWORK=off go run ./scripts/keyed_oracle "$testdata" "$testdata/conformance/keyed-cases.tsv")
} > "$out.tmp"
mv "$out.tmp" "$out"
echo "wrote $out"
