#!/usr/bin/env bash
# Regenerate crates/protowire-pxf/src/go_print.rs: the code points Go's
# strconv.IsPrint calls printable, from the Go toolchain on PATH. Use the
# toolchain protowire-go is built with, since the set follows its Unicode
# version.

set -euo pipefail

repo_root="$(cd "$(dirname "$0")/.." && pwd)"
out="$repo_root/crates/protowire-pxf/src/go_print.rs"
(cd "$repo_root/scripts/go-print-ranges" && GOWORK=off GO111MODULE=off go run main.go) > "$out.tmp"
mv "$out.tmp" "$out"
echo "wrote $out"
