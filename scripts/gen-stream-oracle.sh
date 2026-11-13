#!/usr/bin/env bash
# Regenerate the message streams protowire-go's stream encoders write (#128):
# crates/protowire-pb/testdata/go-pb-stream.bin,
# crates/protowire-envelope/testdata/go-envelope-stream.bin and
# crates/protowire-sbe/testdata/go-sbe-stream.bin. The ports' tests read them
# and write the same messages byte for byte. The reference is read from the
# sibling checkout's REF (default origin/main), exported so its working tree
# does not matter.

set -euo pipefail

repo_root="$(cd "$(dirname "$0")/.." && pwd)"
go_repo="$repo_root/../protowire-go"
ref="${REF:-origin/main}"
sha="$(git -C "$go_repo" rev-parse --short "$ref")"
crates="$repo_root/crates"

work="$(mktemp -d)"
trap 'rm -rf "$work"' EXIT
git -C "$go_repo" archive "$ref" | tar -x -C "$work"
mkdir -p "$work/scripts/stream_oracle" "$crates/protowire-envelope/testdata"
cp "$repo_root/scripts/stream-oracle/main.go" "$work/scripts/stream_oracle/main.go"
(cd "$work" && GOWORK=off go run ./scripts/stream_oracle \
  "$crates/protowire-sbe/testdata/sbe-test.binpb" \
  "$crates/protowire-pb/testdata/go-pb-stream.bin" \
  "$crates/protowire-envelope/testdata/go-envelope-stream.bin" \
  "$crates/protowire-sbe/testdata/go-sbe-stream.bin")
echo "wrote the streams protowire-go at $sha writes"
