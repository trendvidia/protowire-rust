#!/usr/bin/env bash
# Regenerate FileDescriptorSet fixtures used by per-crate tests.
#
# Outputs land under each crate's testdata/ directory and are checked in.
# Mirrors `protowire4ts/package.json`'s `gen:testdata:*` npm scripts:
#  - shared `test.binpb` is built via `protoc` from the canonical
#    `../protowire/testdata/test.proto` (out-of-tree source);
#  - local fixtures (any/d4/sbe) are built via `buf` against this repo's
#    `buf.yaml` workspace, which already knows about vendored annotations.

set -euo pipefail

repo_root="$(cd "$(dirname "$0")/.." && pwd)"
shared_proto_root="$repo_root/../protowire/testdata"

# Shared canonical fixture lives in a sibling repo, outside the buf workspace —
# keep using protoc for it.
protoc \
  --include_imports \
  --descriptor_set_out="$repo_root/crates/protowire-pxf/testdata/test.binpb" \
  -I "$shared_proto_root" \
  "$shared_proto_root/test.proto"

echo "wrote crates/protowire-pxf/testdata/test.binpb (protoc)"

cd "$repo_root"

buf build --as-file-descriptor-set --exclude-source-info \
  --path crates/protowire-pxf/testdata/any-test.proto \
  -o crates/protowire-pxf/testdata/any-test.binpb

echo "wrote crates/protowire-pxf/testdata/any-test.binpb (buf)"

buf build --as-file-descriptor-set --exclude-source-info \
  --path crates/protowire-pxf/testdata/d4-test.proto \
  -o crates/protowire-pxf/testdata/d4-test.binpb

echo "wrote crates/protowire-pxf/testdata/d4-test.binpb (buf)"

buf build --as-file-descriptor-set --exclude-source-info \
  --path crates/protowire-pxf/testdata/hardening-test.proto \
  -o crates/protowire-pxf/testdata/hardening-test.binpb

echo "wrote crates/protowire-pxf/testdata/hardening-test.binpb (buf)"

buf build --as-file-descriptor-set --exclude-source-info \
  --path crates/protowire-sbe/testdata/sbe-test.proto \
  -o crates/protowire-sbe/testdata/sbe-test.binpb

echo "wrote crates/protowire-sbe/testdata/sbe-test.binpb (buf)"

buf build --as-file-descriptor-set --exclude-source-info \
  --path crates/protowire-pxf/testdata/bench-test.proto \
  -o crates/protowire-pxf/testdata/bench-test.binpb

echo "wrote crates/protowire-pxf/testdata/bench-test.binpb (buf)"
