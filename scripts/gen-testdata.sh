#!/usr/bin/env bash
# Regenerate FileDescriptorSet fixtures used by per-crate tests.
#
# Outputs land under each crate's testdata/ directory and are checked in.
# Mirrors `protowire4ts/package.json`'s `gen:testdata:*` npm scripts.

set -euo pipefail

repo_root="$(cd "$(dirname "$0")/.." && pwd)"
shared_proto_root="$repo_root/../protowire/testdata"

protoc \
  --include_imports \
  --descriptor_set_out="$repo_root/crates/protowire-pxf/testdata/test.binpb" \
  -I "$shared_proto_root" \
  "$shared_proto_root/test.proto"

echo "wrote crates/protowire-pxf/testdata/test.binpb"

protoc \
  --include_imports \
  --descriptor_set_out="$repo_root/crates/protowire-pxf/testdata/any-test.binpb" \
  -I "$repo_root/crates/protowire-pxf/testdata" \
  "$repo_root/crates/protowire-pxf/testdata/any-test.proto"

echo "wrote crates/protowire-pxf/testdata/any-test.binpb"
