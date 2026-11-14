#!/usr/bin/env bash
# Regenerate crates/protowire/testdata/conformance/pb-error-oracle.tsv: what
# protowire-go's pb.Unmarshal says about damaged input (#144), from every
# prefix of a full encoding and a list of hand-made inputs. The reference is
# read from the sibling checkout's REF (default origin/main), exported so its
# working tree does not matter.

set -euo pipefail

repo_root="$(cd "$(dirname "$0")/.." && pwd)"
go_repo="$repo_root/../protowire-go"
ref="${REF:-origin/main}"
sha="$(git -C "$go_repo" rev-parse --short "$ref^{commit}")"

work="$(mktemp -d)"
trap 'rm -rf "$work"' EXIT
git -C "$go_repo" archive "$ref" | tar -x -C "$work"
mkdir -p "$work/scripts/pb_error_oracle"
cp "$repo_root/scripts/pb-error-oracle/main.go" "$work/scripts/pb_error_oracle/main.go"

out="$repo_root/crates/protowire/testdata/conformance/pb-error-oracle.tsv"
{
  echo "# What protowire-go at $sha's pb.Unmarshal says about each input."
  echo "# Produced by scripts/gen-pb-error-oracle.sh. Do not edit."
  echo "# Columns: message, limits, input hex, then OK, or ERR and the error."
  (cd "$work" && GOWORK=off go run ./scripts/pb_error_oracle)
} > "$out.tmp"
mv "$out.tmp" "$out"
echo "wrote $out"
