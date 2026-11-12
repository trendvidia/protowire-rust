#!/usr/bin/env bash
# Regenerate crates/protowire/testdata/conformance/retired-oracle.tsv by
# running protowire-go over each case of retired-cases.tsv: a descriptor whose
# options sit at the extension numbers retired in protowire v1.12.0
# (STABILITY.md promise 3, #133). Each case's descriptor set, its imports
# compiled from this repository's proto/, is written into the oracle. The
# reference is read from the sibling checkout's REF (default origin/main),
# exported so its working tree does not matter.

set -euo pipefail

repo_root="$(cd "$(dirname "$0")/.." && pwd)"
go_repo="$repo_root/../protowire-go"
ref="${REF:-origin/main}"
sha="$(git -C "$go_repo" rev-parse --short "$ref")"
conformance="$repo_root/crates/protowire/testdata/conformance"

work="$(mktemp -d)"
trap 'rm -rf "$work"' EXIT
git -C "$go_repo" archive "$ref" | tar -x -C "$work"
mkdir -p "$work/scripts/retired_oracle"
cp "$repo_root/scripts/retired-oracle/main.go" "$work/scripts/retired_oracle/main.go"

out="$conformance/retired-oracle.tsv"
{
  echo "# What protowire-go at $sha does with each case of retired-cases.tsv."
  echo "# Produced by scripts/gen-wkt-shape-oracle.sh. Do not edit."
  echo "# Rows: id, op, then the verdict. fds: the case's descriptor set in hex."
  echo "# validate: ValidateFile's count and violations, \"[kind] text\" joined by \" | \"."
  echo "# unmarshal: OK and the pb hex, or ERR and the error. sbe-codec: NewCodec's"
  echo "# OK, or ERR and the error. sbe-xml: ProtoToXML's OK and the XML, or ERR."
  echo "# Tabs, newlines and backslashes are escaped."
  (cd "$work" && GOWORK=off go run ./scripts/retired_oracle "$repo_root/proto" "$conformance/retired-cases.tsv")
} > "$out.tmp"
mv "$out.tmp" "$out"
echo "wrote $out"
