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
  --path crates/protowire-pxf/testdata/default-placement-test.proto \
  -o crates/protowire-pxf/testdata/default-placement-test.binpb

echo "wrote crates/protowire-pxf/testdata/default-placement-test.binpb (buf)"

buf build --as-file-descriptor-set --exclude-source-info \
  --path crates/protowire-pxf/testdata/secret-literal-test.proto \
  -o crates/protowire-pxf/testdata/secret-literal-test.binpb

echo "wrote crates/protowire-pxf/testdata/secret-literal-test.binpb (buf)"

buf build --as-file-descriptor-set --exclude-source-info \
  --path crates/protowire-pxf/testdata/group-placement-test.proto \
  -o crates/protowire-pxf/testdata/group-placement-test.binpb

echo "wrote crates/protowire-pxf/testdata/group-placement-test.binpb (buf)"

buf build --as-file-descriptor-set --exclude-source-info \
  --path crates/protowire-pxf/testdata/duration-block-test.proto \
  -o crates/protowire-pxf/testdata/duration-block-test.binpb

echo "wrote crates/protowire-pxf/testdata/duration-block-test.binpb (buf)"

buf build --as-file-descriptor-set --exclude-source-info \
  --path crates/protowire-pxf/testdata/placement-accepted-test.proto \
  -o crates/protowire-pxf/testdata/placement-accepted-test.binpb

echo "wrote crates/protowire-pxf/testdata/placement-accepted-test.binpb (buf)"

buf build --as-file-descriptor-set --exclude-source-info \
  --path crates/protowire-pxf/testdata/key-placement-test.proto \
  -o crates/protowire-pxf/testdata/key-placement-test.binpb

echo "wrote crates/protowire-pxf/testdata/key-placement-test.binpb (buf)"

buf build --as-file-descriptor-set --exclude-source-info \
  --path crates/protowire-pxf/testdata/oneof-two-defaults-test.proto \
  -o crates/protowire-pxf/testdata/oneof-two-defaults-test.binpb

echo "wrote crates/protowire-pxf/testdata/oneof-two-defaults-test.binpb (buf)"

buf build --as-file-descriptor-set --exclude-source-info \
  --path crates/protowire-pxf/testdata/oneof-required-test.proto \
  -o crates/protowire-pxf/testdata/oneof-required-test.binpb

echo "wrote crates/protowire-pxf/testdata/oneof-required-test.binpb (buf)"

buf build --as-file-descriptor-set --exclude-source-info \
  --path crates/protowire-pxf/testdata/required-default-test.proto \
  -o crates/protowire-pxf/testdata/required-default-test.binpb

echo "wrote crates/protowire-pxf/testdata/required-default-test.binpb (buf)"

buf build --as-file-descriptor-set --exclude-source-info \
  --path crates/protowire-pxf/testdata/closure-depth/a.proto \
  -o crates/protowire-pxf/testdata/closure-depth-a.binpb

echo "wrote crates/protowire-pxf/testdata/closure-depth-a.binpb (buf)"

buf build --as-file-descriptor-set --exclude-source-info \
  --path crates/protowire-pxf/testdata/closure-diamond/top.proto \
  -o crates/protowire-pxf/testdata/closure-diamond-top.binpb

echo "wrote crates/protowire-pxf/testdata/closure-diamond-top.binpb (buf)"

buf build --as-file-descriptor-set --exclude-source-info \
  --path crates/protowire-pxf/testdata/closure-sort/zz_root.proto \
  -o crates/protowire-pxf/testdata/closure-sort-zz_root.binpb

echo "wrote crates/protowire-pxf/testdata/closure-sort-zz_root.binpb (buf)"

buf build --as-file-descriptor-set --exclude-source-info \
  --path crates/protowire-pxf/testdata/closure-reserved/root.proto \
  -o crates/protowire-pxf/testdata/closure-reserved-root.binpb

echo "wrote crates/protowire-pxf/testdata/closure-reserved-root.binpb (buf)"

buf build --as-file-descriptor-set --exclude-source-info \
  --path crates/protowire-pxf/testdata/closure-required-default/root.proto \
  -o crates/protowire-pxf/testdata/closure-required-default-root.binpb

echo "wrote crates/protowire-pxf/testdata/closure-required-default-root.binpb (buf)"

buf build --as-file-descriptor-set --exclude-source-info \
  --path crates/protowire-pxf/testdata/hardening-test.proto \
  -o crates/protowire-pxf/testdata/hardening-test.binpb

echo "wrote crates/protowire-pxf/testdata/hardening-test.binpb (buf)"

buf build --as-file-descriptor-set --exclude-source-info \
  --path crates/protowire-pxf/testdata/schema-test.proto \
  -o crates/protowire-pxf/testdata/schema-test.binpb

echo "wrote crates/protowire-pxf/testdata/schema-test.binpb (buf)"

buf build --as-file-descriptor-set --exclude-source-info \
  --path crates/protowire-pxf/testdata/keyed-entry-name-test.proto \
  -o crates/protowire-pxf/testdata/keyed-entry-name-test.binpb

echo "wrote crates/protowire-pxf/testdata/keyed-entry-name-test.binpb (buf)"

buf build --as-file-descriptor-set --exclude-source-info \
  --path crates/protowire-pxf/testdata/marshal-depth-test.proto \
  -o crates/protowire-pxf/testdata/marshal-depth-test.binpb

echo "wrote crates/protowire-pxf/testdata/marshal-depth-test.binpb (buf)"

buf build --as-file-descriptor-set --exclude-source-info \
  --path crates/protowire-pxf/testdata/any-readback-test.proto \
  -o crates/protowire-pxf/testdata/any-readback-test.binpb

echo "wrote crates/protowire-pxf/testdata/any-readback-test.binpb (buf)"

buf build --as-file-descriptor-set --exclude-source-info \
  --path crates/protowire-pxf/testdata/unset-annotated-test.proto \
  -o crates/protowire-pxf/testdata/unset-annotated-test.binpb

echo "wrote crates/protowire-pxf/testdata/unset-annotated-test.binpb (buf)"

# Vendored verbatim from the spec repo's testdata/map-keys/ (protowire#284);
# keep in sync when the spec repo adds fixtures.
buf build --as-file-descriptor-set --exclude-source-info \
  --path crates/protowire-pxf/testdata/map-keys/bool-keys.proto \
  -o crates/protowire-pxf/testdata/map-keys/bool-keys.binpb

echo "wrote crates/protowire-pxf/testdata/map-keys/bool-keys.binpb (buf)"
echo "wrote crates/protowire-pxf/testdata/map-keys/bool-keys.binpb (buf)"

buf build --as-file-descriptor-set --exclude-source-info \
  --path crates/protowire-pxf/testdata/element-defaults-test.proto \
  -o crates/protowire-pxf/testdata/element-defaults-test.binpb

echo "wrote crates/protowire-pxf/testdata/element-defaults-test.binpb (buf)"

# Vendored verbatim from the spec repo's testdata/keyed/ (protowire#116);
# keep in sync when the spec repo adds fixtures.
buf build --as-file-descriptor-set --exclude-source-info \
  --path crates/protowire-pxf/testdata/keyed/keyed.proto \
  -o crates/protowire-pxf/testdata/keyed/keyed.binpb

echo "wrote crates/protowire-pxf/testdata/keyed/keyed.binpb (buf)"

buf build --as-file-descriptor-set --exclude-source-info \
  --path crates/protowire-pxf/testdata/bignum-test.proto \
  -o crates/protowire-pxf/testdata/bignum-test.binpb

echo "wrote crates/protowire-pxf/testdata/bignum-test.binpb (buf)"

buf build --as-file-descriptor-set --exclude-source-info \
  --path crates/protowire-pxf/testdata/layered-test.proto \
  -o crates/protowire-pxf/testdata/layered-test.binpb

echo "wrote crates/protowire-pxf/testdata/layered-test.binpb (buf)"

buf build --as-file-descriptor-set --exclude-source-info \
  --path crates/protowire-pxf/testdata/secret-hook-test.proto \
  -o crates/protowire-pxf/testdata/secret-hook-test.binpb

echo "wrote crates/protowire-pxf/testdata/secret-hook-test.binpb (buf)"

buf build --as-file-descriptor-set --exclude-source-info \
  --path crates/protowire-sbe/testdata/sbe-test.proto \
  -o crates/protowire-sbe/testdata/sbe-test.binpb

echo "wrote crates/protowire-sbe/testdata/sbe-test.binpb (buf)"

# char-encoding.proto is xml_to_proto's output for char-encoding.xml, kept
# so it can be compiled here; tests/char_encoding.rs fails if the two drift.
buf build --as-file-descriptor-set --exclude-source-info \
  --path crates/protowire-sbe/testdata/char-encoding.proto \
  -o crates/protowire-sbe/testdata/char-encoding.binpb

echo "wrote crates/protowire-sbe/testdata/char-encoding.binpb (buf)"

# One file per schema for the template-id rule (#121): each file is passed to
# Codec::from_files on its own, so they share one descriptor set.
template_id_paths=()
for f in crates/protowire-sbe/testdata/template-ids-*.proto; do
  template_id_paths+=(--path "$f")
done
buf build --as-file-descriptor-set --exclude-source-info \
  "${template_id_paths[@]}" \
  -o crates/protowire-sbe/testdata/template-ids.binpb

echo "wrote crates/protowire-sbe/testdata/template-ids.binpb (buf)"

# What the SBE wire can hold (#122): one schema per file, since a codec is
# built from one file and a refused message refuses its whole file. After
# changing them, rerun scripts/gen-sbe-oracle.sh for bounds-oracle.tsv.
bounds_paths=()
for f in crates/protowire-sbe/testdata/bounds/*.proto; do
  bounds_paths+=(--path "$f")
done
buf build --as-file-descriptor-set --exclude-source-info \
  "${bounds_paths[@]}" \
  -o crates/protowire-sbe/testdata/bounds.binpb

echo "wrote crates/protowire-sbe/testdata/bounds.binpb (buf)"

buf build --as-file-descriptor-set --exclude-source-info \
  --path crates/protowire-pxf/testdata/bench-test.proto \
  -o crates/protowire-pxf/testdata/bench-test.binpb

echo "wrote crates/protowire-pxf/testdata/bench-test.binpb (buf)"

# Vendored verbatim from the spec repo's testdata/blocks/ (protowire#312);
# keep in sync when the spec repo adds fixtures.
buf build --as-file-descriptor-set --exclude-source-info \
  --path crates/protowire-pxf/testdata/blocks/blocks.proto \
  -o crates/protowire-pxf/testdata/blocks/blocks.binpb

echo "wrote crates/protowire-pxf/testdata/blocks/blocks.binpb (buf)"

# Vendored verbatim from the spec repo's testdata/{nulls,strings,dataset}/
# (protowire#339, #350, #341, #342); keep in sync when the spec repo adds
# fixtures.
for f in nulls/nulls nulls/nulls_proto2 nulls/nulls_editions strings/strings dataset/paths; do
  buf build --as-file-descriptor-set --exclude-source-info \
    --path "crates/protowire-pxf/testdata/$f.proto" \
    -o "crates/protowire-pxf/testdata/$f.binpb"
  echo "wrote crates/protowire-pxf/testdata/$f.binpb (buf)"
done

# Vendored verbatim from the spec repo's testdata/duplicates/ (protowire#347,
# #396); keep in sync when the spec repo adds fixtures.
buf build --as-file-descriptor-set --exclude-source-info \
  --path crates/protowire-pxf/testdata/duplicates/duplicates.proto \
  -o crates/protowire-pxf/testdata/duplicates/duplicates.binpb

echo "wrote crates/protowire-pxf/testdata/duplicates/duplicates.binpb (buf)"

# Vendored verbatim from the spec repo's testdata/any/ (protowire#354); keep
# in sync when the spec repo adds fixtures.
buf build --as-file-descriptor-set --exclude-source-info \
  --path crates/protowire-pxf/testdata/any/any.proto \
  -o crates/protowire-pxf/testdata/any/any.binpb

echo "wrote crates/protowire-pxf/testdata/any/any.binpb (buf)"

buf build --as-file-descriptor-set --exclude-source-info \
  --path crates/protowire-pxf/testdata/float-literals-test.proto \
  -o crates/protowire-pxf/testdata/float-literals-test.binpb

echo "wrote crates/protowire-pxf/testdata/float-literals-test.binpb (buf)"

# Vendored verbatim from the spec repo's testdata/bigfloat/ (protowire#406);
# keep in sync when the spec repo adds fixtures. Its .binpb files are PB
# messages, so the schema's descriptor set is bigfloat.fds.
buf build --as-file-descriptor-set --exclude-source-info \
  --path crates/protowire-pxf/testdata/bigfloat/bigfloat.proto \
  -o crates/protowire-pxf/testdata/bigfloat/bigfloat.fds

echo "wrote crates/protowire-pxf/testdata/bigfloat/bigfloat.fds (buf)"
buf build --as-file-descriptor-set --exclude-source-info \
  --path crates/protowire-pxf/testdata/scan-options-test.proto \
  -o crates/protowire-pxf/testdata/scan-options-test.binpb
echo "wrote crates/protowire-pxf/testdata/scan-options-test.binpb (buf)"
buf build --as-file-descriptor-set --exclude-source-info \
  --path crates/protowire-pxf/testdata/scan-options-bad-test.proto \
  -o crates/protowire-pxf/testdata/scan-options-bad-test.binpb
echo "wrote crates/protowire-pxf/testdata/scan-options-bad-test.binpb (buf)"
# The schemas of protowire-go's dataset_write tests.
for f in dataset-write-null-test; do
  buf build --as-file-descriptor-set --exclude-source-info \
    --path "crates/protowire-pxf/testdata/$f.proto" \
    -o "crates/protowire-pxf/testdata/$f.binpb"
  echo "wrote crates/protowire-pxf/testdata/$f.binpb (buf)"
done
