#!/usr/bin/env bash
# Regenerate crates/protowire-pxf/testdata/conformance/wkt-shape-oracle.tsv by
# running protowire-go over each case of wkt-shape-cases.tsv: a message with a
# well-known type's full name, the right shape or a wrong one (draft -01
# §wkt-shapes, #120). Each case's descriptor set is written into the oracle, as
# the reference compiled it. The reference is read from the sibling checkout's
# REF (default origin/main), exported so its working tree does not matter.

set -euo pipefail

repo_root="$(cd "$(dirname "$0")/.." && pwd)"
go_repo="$repo_root/../protowire-go"
ref="${REF:-origin/main}"
sha="$(git -C "$go_repo" rev-parse --short "$ref")"
conformance="$repo_root/crates/protowire-pxf/testdata/conformance"

work="$(mktemp -d)"
trap 'rm -rf "$work"' EXIT
git -C "$go_repo" archive "$ref" | tar -x -C "$work"
mkdir -p "$work/scripts/wkt_shape_oracle"
cp "$repo_root/scripts/wkt-shape-oracle/main.go" "$work/scripts/wkt_shape_oracle/main.go"

out="$conformance/wkt-shape-oracle.tsv"
{
  echo "# What protowire-go at $sha does with each case of wkt-shape-cases.tsv."
  echo "# Produced by scripts/gen-wkt-shape-oracle.sh. Do not edit."
  echo "# Rows: id, op, document, verdict. fds: the case's descriptor set in hex."
  echo "# validate: the count and the violations, \"[kind] text\" joined by \" | \"."
  echo "# unmarshal, unmarshal-skip, full, full-skip (Unmarshal or UnmarshalFull, with"
  echo "# SkipValidate or not): OK, pb hex, sorted null paths; or ERR or PANIC, the text."
  echo "# marshal: H with each message field holding an empty message; OK and the text,"
  echo "# or ERR or PANIC and the text. Tabs, newlines and backslashes are escaped."
  (cd "$work" && GOWORK=off go run ./scripts/wkt_shape_oracle "$conformance/wkt-shape-cases.tsv")
} > "$out.tmp"
mv "$out.tmp" "$out"
echo "wrote $out"
