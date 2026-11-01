# Annotation carrier fixtures

The `@required` / `@default(v)` spelling lowers to one `AnnotationList` at
extension 1327 on the field's options (RFC-001 §8.1), which only the
reference compiler writes: neither `buf` nor `protoc` parses the `@` sigil,
so these files are not built by `scripts/gen-testdata.sh`.

`generate_test.go.txt` built them. Copy it to
`encoding/pxf/carrier_gen_test.go` in protowire-go, whose `compileV12` it
calls, and run it with `CARRIER_DIR` set to this directory:

    CARRIER_DIR=$PWD go test -count=1 -run TestGenCarrier ./encoding/pxf

Each `<name>.proto` gives two descriptor sets, because a reader meets the
carrier in two shapes:

- `<name>.binpb` has the schema files, and imports `descriptor.proto`, which
  declares 1327, so it is a known extension. `annotations.proto` alone doesn't.
- `<name>.bare.binpb` drops them, so 1327 is an unknown field, as it is for a
  consumer whose descriptors don't import them.

The compiler refuses `@required` on a oneof member, and with `@default`, and
`@default` on a repeated field. A descriptor can still say so, and the
bind-time checks are what catch it, so for `oneof_required`,
`required_default` and `repeated_default` the generator compiles a `donor`
field apart and merges its options onto `target`.
