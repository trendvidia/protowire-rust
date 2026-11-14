#!/usr/bin/env bash
# Regenerate the SBE XML converter goldens under
# crates/protowire-sbe/testdata/convert/ with protowire-go's converter
# (#104), which tests/convert_as_go.rs holds this port's to. The reference is
# read from the sibling checkout's REF (default origin/main), exported so its
# working tree does not matter. Needs buf for the one compile step.
#
#   fixed-arrays.xml         input: protowire-go's fixedArraySchema
#   fixed-arrays.proto       XMLToProto of it, compiled into convert.binpb
#   fixed-arrays.export.xml  ProtoToXML of that
#   fixed-arrays.again.txt   XMLToProto of the export, as text: it is never
#                            compiled, and its package is the first one's
#   bytes-one.proto          input: a bytes field of length 1, in convert.binpb
#   bytes-one.export.xml, bytes-one.again.txt  as above
#   wide-*.xml, wide-*.err   inputs XMLToProto refuses, and its error
#   sbe-test.export.xml, char-encoding.export.xml  ProtoToXML of fixtures
#                                                  next door

set -euo pipefail

repo_root="$(cd "$(dirname "$0")/.." && pwd)"
go_repo="$repo_root/../protowire-go"
ref="${REF:-origin/main}"
testdata="$repo_root/crates/protowire-sbe/testdata"
dir="$testdata/convert"

work="$(mktemp -d)"
trap 'rm -rf "$work"' EXIT
git -C "$go_repo" archive "$ref" | tar -x -C "$work"
mkdir -p "$work/scripts/sbe_convert"
cp "$repo_root/scripts/sbe-convert/main.go" "$work/scripts/sbe_convert/main.go"
(cd "$work" && GOWORK=off go build -o "$work/sbe-convert" ./scripts/sbe_convert)
conv="$work/sbe-convert"

"$conv" xml2proto "$dir/fixed-arrays.xml" > "$dir/fixed-arrays.proto"
cd "$repo_root"
buf build --as-file-descriptor-set --exclude-source-info \
  --path crates/protowire-sbe/testdata/convert/fixed-arrays.proto \
  --path crates/protowire-sbe/testdata/convert/bytes-one.proto \
  -o "$dir/convert.binpb"
for name in fixed-arrays bytes-one; do
  "$conv" proto2xml "$dir/convert.binpb" "convert/$name.proto" > "$dir/$name.export.xml"
  "$conv" xml2proto "$dir/$name.export.xml" > "$dir/$name.again.txt"
done
for f in "$dir"/wide-*.xml; do
  "$conv" xml2proto "$f" > "${f%.xml}.err"
done
"$conv" proto2xml "$testdata/sbe-test.binpb" sbe-test.proto > "$dir/sbe-test.export.xml"
"$conv" proto2xml "$testdata/char-encoding.binpb" char-encoding.proto > "$dir/char-encoding.export.xml"
echo "wrote $dir from protowire-go at $(git -C "$go_repo" rev-parse --short "$ref^{commit}")"
