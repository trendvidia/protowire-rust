#!/usr/bin/env bash
# Regenerate crates/protowire-pxf/testdata/conformance/dataset-write-oracle.tsv
# by running protowire-go's @dataset writer over each case of
# dataset-write-cases.tsv (#129): AppendDatasetHeader, AppendDatasetRow, what
# its reader reads back from the table, and Marshal, against the same
# descriptor sets the port reads. The reference is read from the sibling
# checkout's REF (default origin/main), exported so its working tree does not
# matter.

set -euo pipefail

repo_root="$(cd "$(dirname "$0")/.." && pwd)"
go_repo="$repo_root/../protowire-go"
ref="${REF:-origin/main}"
sha="$(git -C "$go_repo" rev-parse --short "$ref^{commit}")"
testdata="$repo_root/crates/protowire-pxf/testdata"

work="$(mktemp -d)"
trap 'rm -rf "$work"' EXIT
git -C "$go_repo" archive "$ref" | tar -x -C "$work"
mkdir -p "$work/scripts/dataset_write_oracle"
cp "$repo_root/scripts/dataset-write-oracle/main.go" "$work/scripts/dataset_write_oracle/main.go"

out="$testdata/conformance/dataset-write-oracle.tsv"
{
  echo "# What protowire-go at $sha's @dataset writer does with each case of dataset-write-cases.tsv."
  echo "# Produced by scripts/gen-dataset-write-oracle.sh. Do not edit."
  echo "# Columns: descriptor set, message, columns, build, the row's index, its pb hex, then"
  echo "# OK and the text or ERR and the error for: the header, the row, the read-back"
  echo "# (its pb hex, \"=\" when it is the row's own) and Marshal. \"-\" where there is none."
  echo "# Tabs, newlines and backslashes in a verdict are escaped."
  (cd "$work" && GOWORK=off go run ./scripts/dataset_write_oracle "$testdata" "$testdata/conformance/dataset-write-cases.tsv")
} > "$out.tmp"
mv "$out.tmp" "$out"
echo "wrote $out"
